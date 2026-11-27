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

pub mod mp4;

use std::collections::{BTreeMap, HashMap, VecDeque};

use base64::Engine;
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderMap, HeaderValue, StatusCode};
use serde_json::Value;
use servo::{WebResourceLoad, WebResourceResponse, WebViewId};
use url::Url;

use self::mp4::{BoxRef, Track};

/// Script que recoge lo que la página tiene pendiente para Ferrum.
pub const DRAIN: &str = "(window[Symbol.for('ferrum.mse')] || { drain: () => '[]' }).drain()";

/// Duración máxima de cada trozo que se manda al reproductor (ver
/// [`mp4::split_fragment`]).
const PIECE_SECONDS: f64 = 0.5;

/// Más de esto pendiente en un flujo es una web que abusa: se corta.
const MAX_PENDING_BYTES: usize = 256 * 1024 * 1024;

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
}

#[derive(Default)]
struct Buffer {
    /// Bytes que aún no forman cajas completas.
    pending: Vec<u8>,
    ftyp: Option<Vec<u8>>,
    track: Option<Track>,
    fragments: VecDeque<Fragment>,
}

struct Fragment {
    start: f64,
    /// `moof` + `mdat`.
    data: Vec<u8>,
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

    /// El `<video>` de una página pide su flujo: se responde y se deja abierto.
    /// Solo la pestaña que lo creó puede recibirlo.
    pub fn attach(&mut self, token: &str, webview: WebViewId, load: WebResourceLoad) {
        let url = load.request().url.as_url().clone();
        let stream = self
            .streams
            .entry(token.to_owned())
            .or_insert_with(|| Stream::new(webview));
        if stream.webview != webview || stream.response.is_some() {
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

    /// Mensajes que la página dejó para Ferrum (ver [`DRAIN`]).
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
                        stream.append(index, &data);
                    }
                },
                Some("duration") => {
                    stream.duration = message.get("v").and_then(Value::as_f64);
                },
                Some("end") => stream.ended = true,
                Some("close") => {
                    if let Some(mut stream) = self.streams.remove(token) {
                        stream.finish();
                    }
                    continue;
                },
                _ => {},
            }
        }
        self.streams.retain(|_, stream| {
            stream.mux();
            stream.flush();
            if stream.too_big() {
                log::warn!("Flujo MSE demasiado grande: se corta");
                stream.finish();
                return false;
            }
            if stream.done() {
                stream.finish();
                return false;
            }
            true
        });
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
        }
    }

    fn too_big(&self) -> bool {
        let pending: usize = self
            .buffers
            .values()
            .map(|buffer| {
                buffer.pending.len() + buffer.fragments.iter().map(|f| f.data.len()).sum::<usize>()
            })
            .sum();
        pending + self.outbox.len() > MAX_PENDING_BYTES
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
                            Some(pieces) => buffer.fragments.extend(
                                pieces
                                    .into_iter()
                                    .map(|(start, data)| Fragment { start, data }),
                            ),
                            // Sin partir (formato raro): entero, al principio de la cola.
                            None => {
                                let start = buffer
                                    .track
                                    .as_ref()
                                    .and_then(|track| {
                                        mp4::fragment_span(&pending[item.start..item.end], track)
                                    })
                                    .map_or(0.0, |(start, _)| start);
                                buffer.fragments.push_back(Fragment {
                                    start,
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
            "MSE: búfer {index}: {} fragmentos en cola, {} bytes a medias, inicio: {}",
            buffer.fragments.len(),
            buffer.pending.len(),
            buffer.track.is_some()
        );
    }

    /// Monta lo que se pueda: primero el `moov` con todas las pistas, después
    /// los fragmentos de todas en orden de tiempo.
    fn mux(&mut self) {
        if self.buffers.is_empty() {
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
            match mp4::build_init(&tracks, ftyp, self.duration) {
                Some(init) => {
                    log::info!("MSE: {} pistas unidas", tracks.len());
                    self.outbox.extend(init);
                    self.header_sent = true;
                },
                None => return,
            }
        }
        loop {
            // Mientras no termine el flujo, se espera a tener algo de todas las
            // pistas para no adelantar una a otra.
            let every_track_ready = self
                .buffers
                .values()
                .all(|buffer| !buffer.fragments.is_empty());
            if !every_track_ready && !self.ended {
                break;
            }
            let next = self
                .buffers
                .iter()
                .enumerate()
                .filter_map(|(position, (_, buffer))| {
                    buffer.fragments.front().map(|f| (position, f.start))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1));
            let Some((position, _)) = next else {
                break;
            };
            let Some(buffer) = self.buffers.values_mut().nth(position) else {
                break;
            };
            let Some(fragment) = buffer.fragments.pop_front() else {
                break;
            };
            let track_id = u32::try_from(position + 1).unwrap_or(1);
            match mp4::renumber_fragment(&fragment.data, track_id, self.sequence) {
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
        self.ended
            && self.header_sent
            && self.response.is_some()
            && self.outbox.is_empty()
            && self
                .buffers
                .values()
                .all(|buffer| buffer.fragments.is_empty())
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
