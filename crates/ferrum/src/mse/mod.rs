/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Media Source Extensions (MSE) para Servo, que aún no las tiene. Sin ellas,
//! YouTube y casi todas las plataformas de vídeo se quedan en su formato básico.
//!
//! Experimental (`--mse`). Va en tres piezas:
//!
//! 1. En la página, `resources/mse.js` da `MediaSource` y `SourceBuffer`: guarda
//!    lo que la web añade (vídeo y audio por separado, en MP4 fragmentado) y
//!    pone en el `<video>` una dirección interna, `https://ferrum.invalid/mse/…`.
//! 2. Ferrum recoge esos trozos de la página (este módulo) y une las pistas en
//!    un único MP4 fragmentado, ordenado por tiempo ([`mp4`]).
//! 3. Ferrum responde a esa dirección con el MP4 a medida que se forma, y el
//!    reproductor de Servo lo reproduce como cualquier vídeo.
//!
//! Ferrum guarda lo que la página añade, como haría el navegador: la página
//! lo da por cargado y no lo vuelve a mandar. Lo quita solo cuando la página
//! lo pide (`remove()`) o si ocupa demasiado (y entonces se lo dice). El
//! reproductor de Servo no puede saltar dentro de un flujo que no termina, así
//! que un salto (`currentTime`) abre un flujo nuevo que empieza en el
//! fotograma clave anterior a ese punto, con lo que ya había, y una lista de
//! edición para que se vea desde el punto exacto. Cada flujo empieza en el
//! segundo 0 (si no, GStreamer esperaría todo ese tiempo antes de avanzar) y
//! la página suma el desfase, que Ferrum le dice al recoger.

pub mod mp4;

use std::collections::{BTreeMap, HashMap};

use base64::Engine;
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderMap, HeaderValue, StatusCode};
use serde_json::{Value, json};
use servo::{WebResourceLoad, WebResourceResponse, WebViewId};
use url::Url;

use self::mp4::{BoxRef, Piece, Track};

/// Duración máxima de cada trozo que se manda al reproductor (ver
/// [`mp4::split_fragment`]).
const PIECE_SECONDS: f64 = 0.5;

/// Hueco entre dos trozos que aún se considera seguido (en segundos). Mayor
/// que el que `mse.js` da por continuo (0,1 s), para que lo que la página cree
/// tener seguido Ferrum también lo tenga.
const GAP_SECONDS: f64 = 0.15;

/// Lo que se guarda de cada flujo, todas sus pistas, para saltar hacia atrás
/// sin volver a descargar. Al pasarse, se tira lo más viejo ya enviado.
const STORE_BYTES: usize = 128 * 1024 * 1024;

/// Más de esto (sin nada que se pueda tirar) es una web que abusa: se corta.
const MAX_BYTES: usize = 256 * 1024 * 1024;

/// `https://ferrum.invalid/mse/<token>` → `token`.
pub fn stream_token(url: &Url) -> Option<&str> {
    if url.host_str() != Some("ferrum.invalid") {
        return None;
    }
    let token = url.path().strip_prefix("/mse/")?;
    valid_token(token).then_some(token)
}

/// Los tokens los genera `mse.js`: 24 caracteres hexadecimales al azar.
fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= 64 && token.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Respuesta abierta al `<video>`: recibe datos (`Some`) o el final (`None`).
type Sink = Box<dyn FnMut(Option<Vec<u8>>)>;

/// Todos los flujos MSE abiertos.
#[derive(Default)]
pub struct Streams {
    streams: HashMap<String, Stream>,
    /// Avisos para cada página: tramos que Ferrum tiró por memoria y en qué
    /// segundo empieza cada flujo.
    notes: Vec<(WebViewId, Value)>,
}

struct Stream {
    webview: WebViewId,
    response: Option<Sink>,
    /// Lo ya montado que aún no ha podido salir (el `<video>` no lo ha pedido).
    outbox: Vec<u8>,
    buffers: BTreeMap<u32, Buffer>,
    duration: Option<f64>,
    header_sent: bool,
    sequence: u32,
    ended: bool,
    /// Ya se mandó todo y se cerró la respuesta. Se guarda por si la página
    /// salta hacia atrás (se libera con `close` o al cambiar de página).
    closed: bool,
    /// Segundo desde el que empieza (tras un salto); `None`, desde el principio.
    target: Option<f64>,
    /// Ya se eligió por dónde empieza cada pista…
    started: bool,
    /// …el segundo en que empieza el flujo, que se le resta a todo…
    offset: f64,
    /// …y lo que se salta desde ahí hasta el punto pedido (lista de edición).
    skip: f64,
    /// Falta decirle a la página ese segundo.
    announce: bool,
}

#[derive(Default)]
struct Buffer {
    /// Bytes que aún no forman cajas completas.
    pending: Vec<u8>,
    ftyp: Option<Vec<u8>>,
    track: Option<Track>,
    /// Todo lo que la página añadió y sigue teniendo, en orden de tiempo.
    pieces: Vec<Piece>,
    /// En el flujo actual: inicio del primer trozo (el corte)…
    cut: Option<f64>,
    /// …y, cuando ya se envió algo, inicio y final del último.
    last: Option<(f64, f64)>,
}

/// `b` sigue a `a` sin hueco.
fn follows(a: &Piece, b: &Piece) -> bool {
    b.start >= a.start && b.start <= a.end + GAP_SECONDS
}

/// Inicio del trozo con el fotograma clave desde el que se llega a `from` sin
/// huecos, o `None` si ese punto no está.
fn keyframe_before(pieces: &[Piece], from: f64) -> Option<f64> {
    // Si `from` cae justo antes de un tramo que empieza con fotograma clave
    // (YouTube salta a 60,001 y su tramo empieza en 60,03), desde ahí.
    let ahead = pieces
        .iter()
        .find(|piece| piece.start > from)
        .filter(|piece| piece.sync && piece.start <= from + 0.5)
        .map(|piece| piece.start);
    let Some(at) = pieces.iter().rposition(|piece| piece.start <= from + 0.05) else {
        return ahead;
    };
    if from > pieces[at].end + GAP_SECONDS {
        return ahead; // `from` cae en un hueco
    }
    let mut cut = at;
    while !pieces[cut].sync {
        if cut == 0 || !follows(&pieces[cut - 1], &pieces[cut]) {
            return None;
        }
        cut -= 1;
    }
    Some(pieces[cut].start)
}

impl Buffer {
    /// Añade un trozo. Lo nuevo sustituye a lo que hubiera en ese tramo.
    fn insert(&mut self, piece: Piece) {
        self.pieces
            .retain(|old| old.start < piece.start - 0.001 || old.start >= piece.end - 0.001);
        let at = self.pieces.partition_point(|old| old.start < piece.start);
        self.pieces.insert(at, piece);
    }

    /// `SourceBuffer.remove()`: quita lo que cae entero en `[start, end)`.
    fn remove(&mut self, start: f64, end: f64) {
        self.pieces
            .retain(|piece| piece.start < start - 0.001 || piece.end > end + 0.001);
    }

    /// Siguiente trozo para el flujo actual, si ya está y sigue sin hueco.
    fn next(&self) -> Option<usize> {
        match (self.last, self.cut) {
            (Some((start, end)), _) => {
                let at = self
                    .pieces
                    .partition_point(|piece| piece.start <= start + 0.001);
                self.pieces
                    .get(at)
                    .filter(|piece| piece.start <= end + GAP_SECONDS)
                    .map(|_| at)
            },
            (None, Some(cut)) => {
                let at = self
                    .pieces
                    .partition_point(|piece| piece.start < cut - 0.001);
                (at < self.pieces.len()).then_some(at)
            },
            (None, None) => None,
        }
    }

    fn bytes(&self) -> usize {
        self.pending.len()
            + self
                .pieces
                .iter()
                .map(|piece| piece.data.len())
                .sum::<usize>()
    }
}

impl Streams {
    /// Pestañas con algún flujo abierto.
    pub fn webviews(&self) -> Vec<WebViewId> {
        let mut ids: Vec<WebViewId> = Vec::new();
        for stream in self.streams.values() {
            if !ids.contains(&stream.webview) {
                ids.push(stream.webview);
            }
        }
        ids
    }

    /// Script que recoge lo que la página tiene pendiente para Ferrum y le pasa
    /// sus avisos (ver `notes`).
    pub fn drain_script(&mut self, webview: WebViewId) -> String {
        let mut notes = Vec::new();
        self.notes.retain(|(id, item)| {
            if *id == webview {
                notes.push(item.clone());
                false
            } else {
                true
            }
        });
        format!(
            "(window[Symbol.for('ferrum.mse')] || {{ drain: () => '[]' }}).drain({})",
            Value::Array(notes)
        )
    }

    /// El `<video>` de una página pide su flujo: se responde y se deja abierto.
    /// Solo la pestaña que lo creó puede recibirlo.
    pub fn attach(&mut self, token: &str, webview: WebViewId, load: WebResourceLoad) {
        let url = load.request().url.as_url().clone();
        let stream = self
            .streams
            .entry(token.to_owned())
            .or_insert_with(|| Stream::new(webview));
        if stream.webview != webview || stream.response.is_some() || stream.closed {
            log::warn!("Flujo MSE pedido desde otra pestaña o dos veces: se rechaza");
            load.intercept(WebResourceResponse::new(url).status_code(StatusCode::FORBIDDEN))
                .cancel();
            return;
        }
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("video/mp4"));
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
        let response = WebResourceResponse::new(url)
            .headers(headers)
            .status_code(StatusCode::OK);
        // Servo no exporta el tipo de la respuesta interceptada: se guarda dentro
        // de una función que recibe los datos (`Some`) o el final (`None`).
        log::info!("Flujo MSE abierto");
        let mut open = Some(load.intercept(response));
        stream.response = Some(Box::new(move |chunk: Option<Vec<u8>>| match chunk {
            Some(data) => {
                if let Some(load) = open.as_mut() {
                    load.send_body_data(data);
                }
            },
            None => {
                if let Some(load) = open.take() {
                    load.finish();
                }
            },
        }));
        stream.flush();
    }

    /// Mensajes que la página dejó para Ferrum (ver [`Streams::drain_script`]).
    pub fn receive(&mut self, webview: WebViewId, json: &str) {
        let Ok(Value::Array(messages)) = serde_json::from_str::<Value>(json) else {
            return;
        };
        for message in messages {
            let Some(token) = message.get("s").and_then(Value::as_str) else {
                continue;
            };
            if !valid_token(token) {
                continue;
            }
            let number = |key: &str| message.get(key).and_then(Value::as_f64);
            // Salto: la página sigue desde `v` segundos en un flujo nuevo, `n`.
            if message.get("t").and_then(Value::as_str) == Some("restart") {
                let next = message.get("n").and_then(Value::as_str).unwrap_or_default();
                self.restart(webview, token, next, number("v").unwrap_or(0.0));
                continue;
            }
            let stream = self
                .streams
                .entry(token.to_owned())
                .or_insert_with(|| Stream::new(webview));
            if stream.webview != webview {
                continue;
            }
            let index = message
                .get("i")
                .and_then(Value::as_u64)
                .and_then(|i| u32::try_from(i).ok());
            match message.get("t").and_then(Value::as_str) {
                Some("add") => {
                    if let Some(index) = index {
                        stream.buffers.entry(index).or_default();
                    }
                },
                Some("data") => {
                    let data = message.get("d").and_then(Value::as_str).and_then(|text| {
                        base64::engine::general_purpose::STANDARD.decode(text).ok()
                    });
                    if let (Some(index), Some(data)) = (index, data) {
                        // Añadir tras `endOfStream` vuelve a abrir el flujo.
                        stream.ended = false;
                        stream.append(index, &data);
                        for (index, start, end) in stream.evict(STORE_BYTES) {
                            self.notes.push((
                                webview,
                                json!({ "t": "evict", "s": token, "i": index, "a": start, "b": end }),
                            ));
                        }
                    }
                },
                Some("remove") => {
                    let buffer = index.and_then(|index| stream.buffers.get_mut(&index));
                    if let (Some(buffer), Some(start), Some(end)) =
                        (buffer, number("a"), number("b"))
                    {
                        buffer.remove(start, end);
                    }
                },
                // `abort()`: lo que quedaba a medias se tira.
                Some("reset") => {
                    if let Some(buffer) = index.and_then(|index| stream.buffers.get_mut(&index)) {
                        buffer.pending.clear();
                    }
                },
                Some("duration") => stream.duration = number("v"),
                Some("end") => stream.ended = true,
                Some("close") => {
                    if let Some(mut stream) = self.streams.remove(token) {
                        stream.finish();
                    }
                },
                _ => {},
            }
        }
        let notes = &mut self.notes;
        self.streams.retain(|token, stream| {
            stream.mux();
            stream.flush();
            if std::mem::take(&mut stream.announce) {
                notes.push((
                    stream.webview,
                    json!({ "t": "offset", "s": token, "o": stream.offset + stream.skip }),
                ));
            }
            if stream.too_big() {
                log::warn!("Flujo MSE demasiado grande: se corta");
                stream.finish();
                return false;
            }
            if stream.done() {
                stream.finish();
                stream.closed = true;
            }
            true
        });
    }

    /// El flujo `token` sigue como `next`, desde `from` segundos.
    fn restart(&mut self, webview: WebViewId, token: &str, next: &str, from: f64) {
        let owned = self
            .streams
            .get(token)
            .is_some_and(|stream| stream.webview == webview);
        // El `<video>` puede pedir el flujo nuevo antes de que llegue este
        // mensaje: entonces ya hay uno vacío esperando, con su respuesta.
        let free = match self.streams.get(next) {
            None => true,
            Some(waiting) => {
                waiting.webview == webview
                    && waiting.buffers.is_empty()
                    && !waiting.header_sent
                    && !waiting.closed
            },
        };
        if !owned || token == next || !valid_token(next) || !free {
            return;
        }
        if let Some(mut old) = self.streams.remove(token) {
            let mut restarted = old.restart(from);
            old.finish();
            if let Some(mut waiting) = self.streams.remove(next) {
                restarted.response = waiting.response.take();
            }
            self.streams.insert(next.to_owned(), restarted);
        }
    }

    /// Una pestaña se cerró o cambió de página.
    pub fn remove_webview(&mut self, webview: WebViewId) {
        self.streams.retain(|_, stream| {
            if stream.webview == webview {
                stream.finish();
                false
            } else {
                true
            }
        });
        self.notes.retain(|(id, _)| *id != webview);
    }
}

impl Stream {
    fn new(webview: WebViewId) -> Self {
        Stream {
            webview,
            response: None,
            outbox: Vec::new(),
            buffers: BTreeMap::new(),
            duration: None,
            header_sent: false,
            sequence: 1,
            ended: false,
            closed: false,
            target: None,
            started: false,
            offset: 0.0,
            skip: 0.0,
            announce: false,
        }
    }

    /// Flujo nuevo, para la misma página, que empieza en `from` segundos y se
    /// queda con todo lo que tenía este.
    fn restart(&mut self, from: f64) -> Stream {
        let mut stream = Stream::new(self.webview);
        stream.duration = self.duration;
        stream.ended = self.ended;
        stream.target = Some(from);
        stream.buffers = std::mem::take(&mut self.buffers);
        for buffer in stream.buffers.values_mut() {
            buffer.cut = None;
            buffer.last = None;
        }
        stream
    }

    /// Elige por dónde empieza cada pista: el vídeo, en el fotograma clave
    /// anterior al punto pedido; el audio, en el trozo que cubre ese fotograma
    /// (si el audio fuera muy por delante, GStreamer se atasca esperando el
    /// vídeo). Espera a que todas las pistas tengan ese punto.
    fn start(&mut self) -> bool {
        if self.started {
            return true;
        }
        // Hace falta saber qué pista es el vídeo.
        if self.buffers.values().any(|buffer| buffer.track.is_none()) {
            return false;
        }
        let target = match self.target {
            Some(target) => target,
            // Desde el principio: donde ya tengan algo todas las pistas.
            None => {
                let firsts: Option<Vec<f64>> = self
                    .buffers
                    .values()
                    .map(|buffer| buffer.pieces.first().map(|piece| piece.start))
                    .collect();
                match firsts {
                    Some(firsts) if !firsts.is_empty() => {
                        firsts.into_iter().fold(f64::MIN, f64::max)
                    },
                    _ => return false,
                }
            },
        };
        let cuts: Option<Vec<f64>> = self
            .buffers
            .values()
            .map(|buffer| keyframe_before(&buffer.pieces, target))
            .collect();
        let Some(cuts) = cuts.filter(|cuts| !cuts.is_empty()) else {
            return false;
        };
        let is_video = |buffer: &Buffer| {
            buffer
                .track
                .as_ref()
                .is_some_and(|track| &track.handler == b"vide")
        };
        let anchor = self
            .buffers
            .values()
            .zip(&cuts)
            .filter(|(buffer, _)| is_video(buffer))
            .map(|(_, cut)| *cut)
            .reduce(f64::min)
            .unwrap_or_else(|| cuts.iter().copied().fold(target, f64::min));
        let mut offset = f64::MAX;
        for (buffer, cut) in self.buffers.values_mut().zip(cuts) {
            let cut = if is_video(buffer) {
                cut
            } else {
                keyframe_before(&buffer.pieces, anchor).unwrap_or(cut)
            };
            offset = offset.min(cut);
            buffer.cut = Some(cut);
        }
        self.offset = offset.max(0.0);
        // Al principio no se salta nada: se ve desde el primer fotograma.
        self.skip = self
            .target
            .map_or(0.0, |target| (target - self.offset).max(0.0));
        self.started = true;
        self.announce = true;
        if log::log_enabled!(log::Level::Debug) {
            for (index, buffer) in &self.buffers {
                let cut = buffer.cut.unwrap_or_default();
                let piece = buffer
                    .pieces
                    .iter()
                    .find(|piece| piece.start >= cut - 0.001);
                log::debug!(
                    "MSE: pista {index}: corte en {cut:.2} s, trozo de {:.2} s, {} trozos",
                    piece.map_or(0.0, |piece| piece.end - piece.start),
                    buffer.pieces.len()
                );
            }
        }
        log::info!(
            "MSE: flujo desde {:.2} s, se ve desde {:.2} s",
            self.offset,
            self.offset + self.skip
        );
        true
    }

    fn too_big(&self) -> bool {
        let stored: usize = self.buffers.values().map(Buffer::bytes).sum();
        stored + self.outbox.len() > MAX_BYTES
    }

    /// Si lo guardado pasa de `limit`, tira lo más viejo que ya quedó por
    /// detrás de lo enviado. Devuelve lo tirado: (pista, inicio, final).
    fn evict(&mut self, limit: usize) -> Vec<(u32, f64, f64)> {
        let mut total: usize = self.buffers.values().map(Buffer::bytes).sum();
        let mut evicted = Vec::new();
        while total > limit {
            let oldest = self
                .buffers
                .iter()
                .filter_map(|(index, buffer)| {
                    let first = buffer.pieces.first()?;
                    let behind = buffer
                        .last
                        .is_some_and(|(start, _)| first.start < start - 0.001);
                    behind.then_some((*index, first.start))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1));
            let Some(buffer) = oldest.and_then(|(index, _)| self.buffers.get_mut(&index)) else {
                break;
            };
            let piece = buffer.pieces.remove(0);
            total -= piece.data.len();
            if let Some((index, _)) = oldest {
                evicted.push((index, piece.start, piece.end));
            }
        }
        evicted
    }

    /// Añade bytes de un `SourceBuffer` y separa las cajas completas.
    fn append(&mut self, index: u32, data: &[u8]) {
        let buffer = self.buffers.entry(index).or_default();
        buffer.pending.extend_from_slice(data);
        let pending = std::mem::take(&mut buffer.pending);
        let (boxes, _) = mp4::boxes_in(&pending, 0, pending.len());
        let mut used = 0;
        let mut init: Vec<u8> = Vec::new();
        let mut at = 0;
        while at < boxes.len() {
            let item: BoxRef = boxes[at];
            match &item.kind {
                b"ftyp" => {
                    buffer.ftyp = Some(pending[item.start..item.end].to_vec());
                    used = item.end;
                },
                b"moov" => {
                    init.clear();
                    if let Some(ftyp) = &buffer.ftyp {
                        init.extend_from_slice(ftyp);
                    }
                    init.extend_from_slice(&pending[item.start..item.end]);
                    match mp4::parse_init(&init) {
                        Some(parsed) => buffer.track = parsed.tracks.into_iter().next(),
                        None => log::warn!("Segmento de inicialización MSE no válido"),
                    }
                    used = item.end;
                },
                // Un fragmento es `moof` + `mdat`: si el `mdat` no ha llegado, se
                // espera; un `moof` suelto se descarta.
                b"moof" => match boxes.get(at + 1) {
                    None => break,
                    Some(next) if &next.kind == b"mdat" => {
                        let fragment = &pending[item.start..next.end];
                        let pieces = buffer
                            .track
                            .as_ref()
                            .and_then(|track| mp4::split_fragment(fragment, track, PIECE_SECONDS));
                        match pieces {
                            Some(pieces) => {
                                for piece in pieces {
                                    buffer.insert(piece);
                                }
                            },
                            // Sin partir (formato raro): entero.
                            None => {
                                log::debug!("Fragmento MSE sin partir");
                                let (start, end) = buffer
                                    .track
                                    .as_ref()
                                    .and_then(|track| {
                                        mp4::fragment_span(&pending[item.start..item.end], track)
                                    })
                                    .unwrap_or((0.0, 0.0));
                                buffer.insert(Piece {
                                    start,
                                    end,
                                    sync: true,
                                    data: fragment.to_vec(),
                                });
                            },
                        }
                        used = next.end;
                        at += 1;
                    },
                    Some(_) => used = item.end,
                },
                // styp, sidx, emsg, prft, free…: no hacen falta.
                _ => used = item.end,
            }
            at += 1;
        }
        buffer.pending = pending[used..].to_vec();
        log::debug!(
            "MSE: búfer {index}: {} trozos, {} bytes a medias, inicio: {}",
            buffer.pieces.len(),
            buffer.pending.len(),
            buffer.track.is_some()
        );
    }

    /// Monta lo que se pueda: primero el `moov` con todas las pistas, después
    /// los trozos de todas en orden de tiempo, sin saltarse huecos.
    fn mux(&mut self) {
        // La cabecera depende de dónde empieza el flujo (lista de edición).
        if self.buffers.is_empty() || !self.start() {
            return;
        }
        if !self.header_sent {
            let tracks: Option<Vec<&Track>> = self
                .buffers
                .values()
                .map(|buffer| buffer.track.as_ref())
                .collect();
            let Some(tracks) = tracks else {
                return; // falta el inicio de alguna pista
            };
            let ftyp = self
                .buffers
                .values()
                .find_map(|buffer| buffer.ftyp.as_deref());
            let duration = self.duration.map(|duration| duration - self.offset);
            match mp4::build_init(&tracks, ftyp, duration, self.skip) {
                Some(init) => {
                    log::info!("MSE: {} pistas unidas", tracks.len());
                    self.outbox.extend(init);
                    self.header_sent = true;
                },
                None => return,
            }
        }
        loop {
            // Mientras no termine el flujo, se espera a tener lo siguiente de
            // todas las pistas para no adelantar una a otra.
            let next: Vec<Option<usize>> = self.buffers.values().map(Buffer::next).collect();
            if next.iter().any(Option::is_none) && !self.ended {
                break;
            }
            let pick = self
                .buffers
                .values()
                .zip(&next)
                .enumerate()
                .filter_map(|(position, (buffer, at))| {
                    at.map(|at| (position, at, buffer.pieces[at].start))
                })
                .min_by(|a, b| a.2.total_cmp(&b.2));
            let Some((position, at, _)) = pick else {
                break;
            };
            let Some(buffer) = self.buffers.values_mut().nth(position) else {
                break;
            };
            let piece = &buffer.pieces[at];
            buffer.last = Some((piece.start, piece.end));
            let track_id = u32::try_from(position + 1).unwrap_or(1);
            let timescale = buffer.track.as_ref().map_or(1, |track| track.timescale);
            let shift = (self.offset * f64::from(timescale)).round() as u64;
            match mp4::renumber_fragment(&piece.data, track_id, self.sequence, shift) {
                Some(bytes) => {
                    self.outbox.extend(bytes);
                    self.sequence = self.sequence.wrapping_add(1);
                },
                None => log::warn!("Fragmento MSE que no se puede mover: se omite"),
            }
        }
    }

    fn flush(&mut self) {
        if self.outbox.is_empty() {
            return;
        }
        if let Some(response) = self.response.as_mut() {
            log::debug!("MSE: {} bytes al reproductor", self.outbox.len());
            response(Some(std::mem::take(&mut self.outbox)));
        }
    }

    fn done(&self) -> bool {
        !self.closed
            && self.ended
            && self.started
            && self.header_sent
            && self.response.is_some()
            && self.outbox.is_empty()
            && self.buffers.values().all(|buffer| buffer.next().is_none())
    }

    fn finish(&mut self) {
        self.mux();
        self.flush();
        if let Some(mut response) = self.response.take() {
            response(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(start: f64, sync: bool) -> Piece {
        Piece {
            start,
            end: start + 0.5,
            sync,
            data: vec![0; 10],
        }
    }

    fn buffer_with(pieces: &[(f64, bool)]) -> Buffer {
        let mut buffer = Buffer::default();
        for (start, sync) in pieces {
            buffer.insert(piece(*start, *sync));
        }
        buffer
    }

    /// Búfer con su pista (`vide` o `soun`) y unos trozos.
    fn track_with(handler: &[u8; 4], pieces: &[(f64, bool)]) -> Buffer {
        let mut buffer = buffer_with(pieces);
        let init = mp4::tests::init_segment(1, 1000, handler);
        buffer.track = mp4::parse_init(&init).map(|init| init.tracks[0].clone());
        buffer
    }

    fn starts(buffer: &Buffer) -> Vec<f64> {
        buffer.pieces.iter().map(|piece| piece.start).collect()
    }

    #[test]
    fn restarts_from_the_last_keyframe_before_the_jump() {
        let buffer = buffer_with(&[
            (0.0, true),
            (0.5, false),
            (1.0, true),
            (1.5, false),
            (2.0, false),
        ]);
        assert_eq!(keyframe_before(&buffer.pieces, 1.7), Some(1.0));
        assert_eq!(keyframe_before(&buffer.pieces, 0.2), Some(0.0));
        assert_eq!(keyframe_before(&buffer.pieces, 2.4), Some(1.0));
        // Más allá de lo que hay: nada (la página mandará lo nuevo).
        assert_eq!(keyframe_before(&buffer.pieces, 30.0), None);
    }

    #[test]
    fn needs_a_keyframe_without_gaps() {
        // 0–2 s, hueco, 10–11 s (sin fotograma clave al principio).
        let buffer = buffer_with(&[
            (0.0, true),
            (0.5, false),
            (1.0, false),
            (1.5, false),
            (10.0, false),
            (10.5, false),
        ]);
        assert_eq!(keyframe_before(&buffer.pieces, 1.2), Some(0.0));
        assert_eq!(keyframe_before(&buffer.pieces, 5.0), None);
        assert_eq!(keyframe_before(&buffer.pieces, 10.2), None);
        // Justo antes de un tramo que empieza con fotograma clave: desde él.
        let buffer = buffer_with(&[(0.0, true), (0.5, false), (60.03, true), (60.53, false)]);
        assert_eq!(keyframe_before(&buffer.pieces, 60.001), Some(60.03));
        assert_eq!(keyframe_before(&buffer.pieces, 59.0), None);
    }

    #[test]
    fn new_pieces_replace_the_same_stretch_and_keep_the_rest() {
        let mut buffer = buffer_with(&[(0.0, true), (0.5, true), (1.0, true), (1.5, true)]);
        // Se vuelve a mandar 1,0–1,5 y llega algo anterior: no se pierde nada.
        buffer.insert(piece(1.0, true));
        buffer.insert(piece(-0.5, true));
        assert_eq!(starts(&buffer), vec![-0.5, 0.0, 0.5, 1.0, 1.5]);
        buffer.remove(0.0, 1.0);
        assert_eq!(starts(&buffer), vec![-0.5, 1.0, 1.5]);
    }

    #[test]
    fn sends_in_order_and_stops_at_gaps() {
        let mut buffer = buffer_with(&[(0.0, true), (0.5, false), (2.0, false)]);
        buffer.cut = Some(0.0);
        let mut sent = Vec::new();
        while let Some(at) = buffer.next() {
            let piece = &buffer.pieces[at];
            sent.push(piece.start);
            buffer.last = Some((piece.start, piece.end));
        }
        assert_eq!(sent, vec![0.0, 0.5]);
        // La página rellena el hueco: sigue.
        buffer.insert(piece(1.0, false));
        buffer.insert(piece(1.5, false));
        while let Some(at) = buffer.next() {
            let piece = &buffer.pieces[at];
            sent.push(piece.start);
            buffer.last = Some((piece.start, piece.end));
        }
        assert_eq!(sent, vec![0.0, 0.5, 1.0, 1.5, 2.0]);
    }

    #[test]
    fn audio_starts_with_the_video_keyframe() {
        let mut stream = Stream::new(servo_base::id::TEST_WEBVIEW_ID);
        stream.buffers.insert(
            0,
            track_with(
                b"vide",
                &[(0.0, true), (0.5, false), (1.0, false), (1.5, false)],
            ),
        );
        stream.buffers.insert(
            1,
            track_with(
                b"soun",
                &[(0.0, true), (0.5, true), (1.0, true), (1.5, true)],
            ),
        );
        let mut restarted = stream.restart(1.2);
        assert!(restarted.start());
        // El vídeo, desde su fotograma clave; el audio, desde el mismo punto; y
        // se ve desde 1,2 s (lista de edición).
        assert_eq!(restarted.buffers[&0].cut, Some(0.0));
        assert_eq!(restarted.buffers[&1].cut, Some(0.0));
        assert_eq!(restarted.offset, 0.0);
        assert!((restarted.skip - 1.2).abs() < 1e-9);
        // Si a una pista le falta el punto, se espera a que la página lo mande.
        let mut waiting = restarted.restart(30.0);
        assert!(!waiting.start());
        waiting
            .buffers
            .get_mut(&0)
            .expect("vídeo")
            .insert(piece(29.8, true));
        waiting
            .buffers
            .get_mut(&1)
            .expect("audio")
            .insert(piece(29.9, true));
        assert!(waiting.start());
        assert_eq!(waiting.buffers[&0].cut, Some(29.8));
        assert_eq!(waiting.offset, 29.8);
    }

    #[test]
    fn evicts_only_what_was_already_sent() {
        let mut stream = Stream::new(servo_base::id::TEST_WEBVIEW_ID);
        let mut buffer = buffer_with(&[(0.0, true), (0.5, true), (1.0, true), (1.5, true)]);
        buffer.last = Some((1.0, 1.5));
        stream.buffers.insert(0, buffer);
        // 40 bytes guardados; con un límite de 15, se tiran los dos primeros
        // (el tercero es el último enviado: se queda).
        let evicted = stream.evict(15);
        assert_eq!(evicted, vec![(0, 0.0, 0.5), (0, 0.5, 1.0)]);
        assert_eq!(starts(&stream.buffers[&0]), vec![1.0, 1.5]);
    }

    #[test]
    fn tokens_are_strict() {
        let ok = Url::parse("https://ferrum.invalid/mse/ab12CD").expect("url");
        assert_eq!(stream_token(&ok), Some("ab12CD"));
        for bad in [
            "https://ferrum.invalid/mse/",
            "https://ferrum.invalid/mse/a/b",
            "https://ferrum.invalid/otra/abc",
            "https://evil.example/mse/abc",
        ] {
            assert_eq!(stream_token(&Url::parse(bad).expect("url")), None, "{bad}");
        }
    }
}
