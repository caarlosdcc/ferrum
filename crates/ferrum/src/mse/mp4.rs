/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Lo justo de MP4 fragmentado (ISO BMFF) para unir pistas: leer cajas, saber
//! cuánto dura cada fragmento, renumerar pistas y montar un `moov` con todas.
//!
//! Los datos vienen de la página, así que nada aquí puede entrar en pánico:
//! todo lo que no cuadra devuelve `None`.

/// Una caja MP4 dentro de un búfer: `start..end`, con la carga útil a partir de
/// `start + header`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoxRef {
    pub kind: [u8; 4],
    pub start: usize,
    pub end: usize,
    pub header: usize,
}

impl BoxRef {
    fn payload_start(&self) -> usize {
        self.start + self.header
    }
}

/// Lee una caja que empieza en `start` y termina antes de `limit`. `None` si
/// está incompleta o mal formada.
fn read_box(data: &[u8], start: usize, limit: usize) -> Option<BoxRef> {
    let head = data.get(start..start.checked_add(8)?)?;
    let size32 = u32::from_be_bytes(head[0..4].try_into().ok()?);
    let kind: [u8; 4] = head[4..8].try_into().ok()?;
    let (size, header) = match size32 {
        0 => (limit.checked_sub(start)?, 8),
        1 => {
            let large = data.get(start + 8..start + 16)?;
            let size = u64::from_be_bytes(large.try_into().ok()?);
            (usize::try_from(size).ok()?, 16)
        },
        size => (usize::try_from(size).ok()?, 8),
    };
    let end = start.checked_add(size)?;
    if size < header || end > limit {
        return None;
    }
    Some(BoxRef {
        kind,
        start,
        end,
        header,
    })
}

/// Cajas completas seguidas entre `start` y `end`. Devuelve también hasta dónde
/// llegó: lo que queda es una caja a medias (o datos que no son cajas).
pub fn boxes_in(data: &[u8], start: usize, end: usize) -> (Vec<BoxRef>, usize) {
    let mut found = Vec::new();
    let mut position = start;
    while position < end {
        match read_box(data, position, end) {
            Some(item) => {
                position = item.end;
                found.push(item);
            },
            None => break,
        }
    }
    (found, position)
}

fn children(data: &[u8], parent: &BoxRef) -> Vec<BoxRef> {
    boxes_in(data, parent.payload_start(), parent.end).0
}

fn child(data: &[u8], parent: &BoxRef, kind: &[u8; 4]) -> Option<BoxRef> {
    children(data, parent)
        .into_iter()
        .find(|item| &item.kind == kind)
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(data: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(data.get(at..at + 8)?.try_into().ok()?))
}

fn put_u32(data: &mut [u8], at: usize, value: u32) -> Option<()> {
    data.get_mut(at..at + 4)?
        .copy_from_slice(&value.to_be_bytes());
    Some(())
}

/// Una pista de un segmento de inicialización.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub id: u32,
    pub timescale: u32,
    /// `vide`, `soun`…
    pub handler: [u8; 4],
    /// Duración por defecto de cada muestra (de `trex`).
    pub default_duration: u32,
    /// Tamaño y marcas por defecto de cada muestra (de `trex`).
    default_size: u32,
    default_flags: u32,
    /// Escala de tiempo del `moov` original: en ella están la duración de
    /// `tkhd` y las listas de edición (`elst`).
    movie_timescale: u32,
    trak: Vec<u8>,
    trex: Option<Vec<u8>>,
}

/// Segmento de inicialización (`ftyp` + `moov`).
#[derive(Clone, Debug, PartialEq)]
pub struct Init {
    pub ftyp: Option<Vec<u8>>,
    pub tracks: Vec<Track>,
}

/// Lee el `moov` de un segmento de inicialización.
pub fn parse_init(data: &[u8]) -> Option<Init> {
    let (top, _) = boxes_in(data, 0, data.len());
    let ftyp = top
        .iter()
        .find(|item| &item.kind == b"ftyp")
        .map(|item| data[item.start..item.end].to_vec());
    let moov = top.iter().find(|item| &item.kind == b"moov")?;
    let moov_children = children(data, moov);
    let movie_timescale = moov_children
        .iter()
        .find(|item| &item.kind == b"mvhd")
        .and_then(|mvhd| {
            let version = *data.get(mvhd.payload_start())?;
            u32_at(
                data,
                mvhd.payload_start() + if version == 1 { 20 } else { 12 },
            )
        })
        .unwrap_or(1000)
        .max(1);
    let trexes: Vec<BoxRef> = moov_children
        .iter()
        .find(|item| &item.kind == b"mvex")
        .map(|mvex| {
            children(data, mvex)
                .into_iter()
                .filter(|item| &item.kind == b"trex")
                .collect()
        })
        .unwrap_or_default();

    let mut tracks = Vec::new();
    for trak in moov_children.iter().filter(|item| &item.kind == b"trak") {
        let tkhd = child(data, trak, b"tkhd")?;
        let version = *data.get(tkhd.payload_start())?;
        let id_at = tkhd.payload_start() + if version == 1 { 20 } else { 12 };
        let id = u32_at(data, id_at)?;
        let mdia = child(data, trak, b"mdia")?;
        let mdhd = child(data, &mdia, b"mdhd")?;
        let mdhd_version = *data.get(mdhd.payload_start())?;
        let timescale_at = mdhd.payload_start() + if mdhd_version == 1 { 20 } else { 12 };
        let timescale = u32_at(data, timescale_at)?;
        let hdlr = child(data, &mdia, b"hdlr")?;
        let handler: [u8; 4] = data
            .get(hdlr.payload_start() + 8..hdlr.payload_start() + 12)?
            .try_into()
            .ok()?;
        let trex = trexes
            .iter()
            .find(|trex| u32_at(data, trex.payload_start() + 4) == Some(id));
        let trex_field = |offset: usize| {
            trex.and_then(|trex| u32_at(data, trex.payload_start() + offset))
                .unwrap_or(0)
        };
        tracks.push(Track {
            id,
            timescale: timescale.max(1),
            handler,
            default_duration: trex_field(12),
            default_size: trex_field(16),
            default_flags: trex_field(20),
            movie_timescale,
            trak: data[trak.start..trak.end].to_vec(),
            trex: trex.map(|trex| data[trex.start..trex.end].to_vec()),
        });
    }
    (!tracks.is_empty()).then_some(Init { ftyp, tracks })
}

fn make_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 8);
    out.extend_from_slice(
        &u32::try_from(payload.len() + 8)
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

fn rescale(value: u64, from: u32, to: u32) -> u64 {
    (u128::from(value) * u128::from(to) / u128::from(from.max(1))).min(u128::from(u64::MAX)) as u64
}

fn put_u64(data: &mut [u8], at: usize, value: u64) -> Option<()> {
    data.get_mut(at..at + 8)?
        .copy_from_slice(&value.to_be_bytes());
    Some(())
}

/// Copia el `trak` de una pista con id `id`, pasando a la escala de tiempo
/// `timescale` del `moov` nuevo lo que estaba en la del original: la duración
/// de `tkhd` y las de la lista de edición. Si no, GStreamer cree, por ejemplo,
/// que una pista de una hora dura veintiuna.
fn renumber_trak(track: &Track, id: u32, timescale: u32) -> Option<Vec<u8>> {
    let from = track.movie_timescale;
    let mut out = track.trak.clone();
    let root = read_box(&out, 0, out.len())?;
    let tkhd = child(&out, &root, b"tkhd")?;
    let version = *out.get(tkhd.payload_start())?;
    let body = tkhd.payload_start();
    if version == 1 {
        put_u32(&mut out, body + 20, id)?;
        let duration = u64_at(&out, body + 28)?;
        if duration != u64::MAX {
            put_u64(&mut out, body + 28, rescale(duration, from, timescale))?;
        }
    } else {
        put_u32(&mut out, body + 12, id)?;
        let duration = u32_at(&out, body + 20)?;
        if duration != u32::MAX {
            let scaled = rescale(u64::from(duration), from, timescale).min(u64::from(u32::MAX - 1));
            put_u32(&mut out, body + 20, scaled as u32)?;
        }
    }
    if let Some(edts) = child(&out, &root, b"edts")
        && let Some(elst) = child(&out, &edts, b"elst")
    {
        let version = *out.get(elst.payload_start())?;
        let count = u32_at(&out, elst.payload_start() + 4)?;
        let entry = if version == 1 { 20 } else { 12 };
        let mut at = elst.payload_start() + 8;
        for _ in 0..count {
            if at + entry > elst.end {
                break;
            }
            if version == 1 {
                let duration = u64_at(&out, at)?;
                put_u64(&mut out, at, rescale(duration, from, timescale))?;
            } else {
                let duration = rescale(u64::from(u32_at(&out, at)?), from, timescale);
                put_u32(&mut out, at, duration.min(u64::from(u32::MAX)) as u32)?;
            }
            at += entry;
        }
    }
    Some(out)
}

fn trex_for(track: &Track, id: u32) -> Vec<u8> {
    match &track.trex {
        Some(trex) => {
            let mut out = trex.clone();
            let _ = put_u32(&mut out, 12, id);
            out
        },
        None => {
            let mut payload = vec![0; 24];
            payload[4..8].copy_from_slice(&id.to_be_bytes());
            payload[8..12].copy_from_slice(&1u32.to_be_bytes());
            make_box(b"trex", &payload)
        },
    }
}

/// Monta un segmento de inicialización con todas las pistas, numeradas desde
/// 1 en el orden dado. `duration` (segundos) fija la duración del vídeo.
pub fn build_init(
    tracks: &[&Track],
    ftyp: Option<&[u8]>,
    duration: Option<f64>,
) -> Option<Vec<u8>> {
    const TIMESCALE: u32 = 1000;
    let duration_units = duration
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .map(|seconds| (seconds * f64::from(TIMESCALE)).min(f64::from(u32::MAX)) as u32);

    let mut mvhd = vec![0u8; 100];
    mvhd[12..16].copy_from_slice(&TIMESCALE.to_be_bytes());
    mvhd[16..20].copy_from_slice(&duration_units.unwrap_or(0).to_be_bytes());
    mvhd[20..24].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    mvhd[24..26].copy_from_slice(&0x0100u16.to_be_bytes());
    for (index, value) in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000]
        .iter()
        .enumerate()
    {
        let at = 36 + index * 4;
        mvhd[at..at + 4].copy_from_slice(&value.to_be_bytes());
    }
    let next_id = u32::try_from(tracks.len() + 1).ok()?;
    mvhd[96..100].copy_from_slice(&next_id.to_be_bytes());

    let mut moov = make_box(b"mvhd", &mvhd);
    let mut mvex = Vec::new();
    if let Some(units) = duration_units {
        let mut mehd = vec![0u8; 8];
        mehd[4..8].copy_from_slice(&units.to_be_bytes());
        mvex.extend(make_box(b"mehd", &mehd));
    }
    for (index, track) in tracks.iter().enumerate() {
        let id = u32::try_from(index + 1).ok()?;
        moov.extend(renumber_trak(track, id, TIMESCALE)?);
        mvex.extend(trex_for(track, id));
    }
    moov.extend(make_box(b"mvex", &mvex));

    let mut out = match ftyp {
        Some(ftyp) => ftyp.to_vec(),
        None => make_box(b"ftyp", b"iso6\0\0\0\0iso6isommp41"),
    };
    out.extend(make_box(b"moov", &moov));
    Some(out)
}

/// Inicio y fin (segundos) de un fragmento `moof` de la pista `track`.
pub fn fragment_span(moof: &[u8], track: &Track) -> Option<(f64, f64)> {
    let root = read_box(moof, 0, moof.len())?;
    let traf = child(moof, &root, b"traf")?;
    let tfhd = child(moof, &traf, b"tfhd")?;
    let tfhd_flags = u32_at(moof, tfhd.payload_start())? & 0x00ff_ffff;
    let mut at = tfhd.payload_start() + 8;
    if tfhd_flags & 0x01 != 0 {
        at += 8; // base-data-offset
    }
    if tfhd_flags & 0x02 != 0 {
        at += 4; // sample-description-index
    }
    let mut default_duration = track.default_duration;
    if tfhd_flags & 0x08 != 0 {
        default_duration = u32_at(moof, at)?;
    }

    let tfdt = child(moof, &traf, b"tfdt")?;
    let base = if *moof.get(tfdt.payload_start())? == 1 {
        u64_at(moof, tfdt.payload_start() + 4)?
    } else {
        u64::from(u32_at(moof, tfdt.payload_start() + 4)?)
    };

    let mut total: u64 = 0;
    for trun in children(moof, &traf)
        .into_iter()
        .filter(|item| &item.kind == b"trun")
    {
        let flags = u32_at(moof, trun.payload_start())? & 0x00ff_ffff;
        let count = u32_at(moof, trun.payload_start() + 4)?;
        let mut at = trun.payload_start() + 8;
        if flags & 0x001 != 0 {
            at += 4; // data-offset
        }
        if flags & 0x004 != 0 {
            at += 4; // first-sample-flags
        }
        let per_sample = [0x100u32, 0x200, 0x400, 0x800]
            .iter()
            .filter(|flag| flags & **flag != 0)
            .count()
            * 4;
        for _ in 0..count {
            let duration = if flags & 0x100 != 0 {
                u32_at(moof, at)?
            } else {
                default_duration
            };
            total += u64::from(duration);
            at += per_sample;
        }
    }
    let scale = f64::from(track.timescale);
    Some((base as f64 / scale, (base + total) as f64 / scale))
}

/// Copia un fragmento (`moof` + `mdat` seguidos) como pista `id` con número de
/// secuencia `sequence`. Solo admite fragmentos cuyos datos se cuentan desde
/// el propio `moof`, que es lo habitual en DASH.
pub fn renumber_fragment(fragment: &[u8], id: u32, sequence: u32) -> Option<Vec<u8>> {
    let mut out = fragment.to_vec();
    let root = read_box(&out, 0, out.len())?;
    let mfhd = child(&out, &root, b"mfhd")?;
    put_u32(&mut out, mfhd.payload_start() + 4, sequence)?;
    for traf in children(&out, &root)
        .into_iter()
        .filter(|item| &item.kind == b"traf")
    {
        let tfhd = child(&out, &traf, b"tfhd")?;
        let flags = u32_at(&out, tfhd.payload_start())? & 0x00ff_ffff;
        if flags & 0x01 != 0 {
            return None; // base-data-offset absoluto: no se puede mover
        }
        put_u32(&mut out, tfhd.payload_start() + 4, id)?;
    }
    Some(out)
}

/// Una muestra de un fragmento: duración, tamaño, marcas, desfase de
/// composición y dónde están sus bytes dentro del `mdat`.
#[derive(Clone, Copy, Debug)]
struct Sample {
    duration: u32,
    size: u32,
    flags: u32,
    composition_offset: u32,
    data_start: usize,
}

/// Parte un fragmento (`moof` + `mdat`, como bytes seguidos en `fragment`) en
/// trozos de como mucho `max_seconds`, cada uno con su `moof` y su `mdat`.
///
/// Hace falta porque YouTube manda fragmentos de varios segundos y el
/// reproductor de Servo (GStreamer con `decodebin3`) tiene colas cortas: si
/// llegan diez segundos de audio antes del primer cuadro de vídeo, se llena la
/// del audio y todo se queda esperando. Con trozos pequeños, audio y vídeo van
/// intercalados. Devuelve `(inicio en segundos, bytes)` de cada trozo.
pub fn split_fragment(
    fragment: &[u8],
    track: &Track,
    max_seconds: f64,
) -> Option<Vec<(f64, Vec<u8>)>> {
    let moof = read_box(fragment, 0, fragment.len())?;
    if &moof.kind != b"moof" {
        return None;
    }
    let traf = child(fragment, &moof, b"traf")?;
    let tfhd = child(fragment, &traf, b"tfhd")?;
    let tfhd_flags = u32_at(fragment, tfhd.payload_start())? & 0x00ff_ffff;
    if tfhd_flags & 0x01 != 0 {
        return None; // base-data-offset absoluto
    }
    let mut at = tfhd.payload_start() + 8;
    if tfhd_flags & 0x02 != 0 {
        at += 4;
    }
    let mut default_duration = track.default_duration;
    let mut default_size = track.default_size;
    let mut default_flags = track.default_flags;
    if tfhd_flags & 0x08 != 0 {
        default_duration = u32_at(fragment, at)?;
        at += 4;
    }
    if tfhd_flags & 0x10 != 0 {
        default_size = u32_at(fragment, at)?;
        at += 4;
    }
    if tfhd_flags & 0x20 != 0 {
        default_flags = u32_at(fragment, at)?;
    }
    let tfdt = child(fragment, &traf, b"tfdt")?;
    let base = if *fragment.get(tfdt.payload_start())? == 1 {
        u64_at(fragment, tfdt.payload_start() + 4)?
    } else {
        u64::from(u32_at(fragment, tfdt.payload_start() + 4)?)
    };

    // Muestras de todos los `trun`, con la posición de sus bytes.
    let mut samples = Vec::new();
    let mut trun_version = 0u8;
    let mut next_data = moof.end + 8; // tras la cabecera del `mdat` siguiente
    for trun in children(fragment, &traf)
        .into_iter()
        .filter(|item| &item.kind == b"trun")
    {
        trun_version = trun_version.max(*fragment.get(trun.payload_start())?);
        let flags = u32_at(fragment, trun.payload_start())? & 0x00ff_ffff;
        let count = u32_at(fragment, trun.payload_start() + 4)?;
        let mut at = trun.payload_start() + 8;
        if flags & 0x001 != 0 {
            let offset = i64::from(u32_at(fragment, at)? as i32);
            next_data = usize::try_from(moof.start as i64 + offset).ok()?;
            at += 4;
        }
        let first_flags = if flags & 0x004 != 0 {
            at += 4;
            Some(u32_at(fragment, at - 4)?)
        } else {
            None
        };
        for index in 0..count {
            let mut field = || {
                let value = u32_at(fragment, at);
                at += 4;
                value
            };
            let duration = if flags & 0x100 != 0 {
                field()?
            } else {
                default_duration
            };
            let size = if flags & 0x200 != 0 {
                field()?
            } else {
                default_size
            };
            let sample_flags = if flags & 0x400 != 0 {
                field()?
            } else if index == 0 {
                first_flags.unwrap_or(default_flags)
            } else {
                default_flags
            };
            let composition_offset = if flags & 0x800 != 0 { field()? } else { 0 };
            samples.push(Sample {
                duration,
                size,
                flags: sample_flags,
                composition_offset,
                data_start: next_data,
            });
            next_data = next_data.checked_add(usize::try_from(size).ok()?)?;
        }
    }
    if samples.is_empty() || next_data > fragment.len() {
        return None;
    }

    let limit = (max_seconds * f64::from(track.timescale)).max(1.0) as u64;
    let mut pieces = Vec::new();
    let mut piece_start = 0;
    let mut time = base;
    while piece_start < samples.len() {
        let mut end = piece_start;
        let mut span: u64 = 0;
        while end < samples.len()
            && (end == piece_start || span + u64::from(samples[end].duration) <= limit)
        {
            span += u64::from(samples[end].duration);
            end += 1;
        }
        let chunk = &samples[piece_start..end];
        pieces.push((
            time as f64 / f64::from(track.timescale),
            build_piece(fragment, track.id, time, chunk, trun_version)?,
        ));
        time += span;
        piece_start = end;
    }
    Some(pieces)
}

/// `moof` + `mdat` con unas muestras seguidas.
fn build_piece(
    fragment: &[u8],
    id: u32,
    base: u64,
    samples: &[Sample],
    trun_version: u8,
) -> Option<Vec<u8>> {
    let mut trun = vec![trun_version, 0x00, 0x0f, 0x01]; // duración, tamaño, marcas, desfase y data-offset
    trun.extend_from_slice(&u32::try_from(samples.len()).ok()?.to_be_bytes());
    trun.extend_from_slice(&[0; 4]); // data-offset, se rellena después
    let mut mdat = Vec::new();
    for sample in samples {
        trun.extend_from_slice(&sample.duration.to_be_bytes());
        trun.extend_from_slice(&sample.size.to_be_bytes());
        trun.extend_from_slice(&sample.flags.to_be_bytes());
        trun.extend_from_slice(&sample.composition_offset.to_be_bytes());
        let end = sample
            .data_start
            .checked_add(usize::try_from(sample.size).ok()?)?;
        mdat.extend_from_slice(fragment.get(sample.data_start..end)?);
    }
    let mut tfhd = vec![0x00, 0x02, 0x00, 0x00]; // default-base-is-moof
    tfhd.extend_from_slice(&id.to_be_bytes());
    let mut tfdt = vec![0x01, 0, 0, 0];
    tfdt.extend_from_slice(&base.to_be_bytes());
    let traf = [
        make_box(b"tfhd", &tfhd),
        make_box(b"tfdt", &tfdt),
        make_box(b"trun", &trun),
    ]
    .concat();
    let mfhd = make_box(b"mfhd", &[0, 0, 0, 0, 0, 0, 0, 1]);
    let mut moof = make_box(b"moof", &[mfhd, make_box(b"traf", &traf)].concat());
    // El data-offset cuenta desde el inicio del `moof` hasta los datos del `mdat`.
    let data_offset = u32::try_from(moof.len() + 8).ok()?;
    let at = moof.len() - (trun.len() + 8) + 8 + 8;
    put_u32(&mut moof, at, data_offset)?;
    moof.extend(make_box(b"mdat", &mdat));
    Some(moof)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn full(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
        let mut payload = vec![version];
        payload.extend_from_slice(&flags.to_be_bytes()[1..]);
        payload.extend_from_slice(body);
        make_box(kind, &payload)
    }

    /// Segmento de inicialización de una pista, como los de DASH.
    pub fn init_segment(id: u32, timescale: u32, handler: &[u8; 4]) -> Vec<u8> {
        let mut tkhd = vec![0u8; 80];
        tkhd[8..12].copy_from_slice(&id.to_be_bytes());
        let mut mdhd = vec![0u8; 20];
        mdhd[8..12].copy_from_slice(&timescale.to_be_bytes());
        let mut hdlr = vec![0u8; 21];
        hdlr[4..8].copy_from_slice(handler);
        let mdia = [full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr)].concat();
        let trak = [full(b"tkhd", 0, 3, &tkhd), make_box(b"mdia", &mdia)].concat();
        let mut trex = vec![0u8; 20];
        trex[0..4].copy_from_slice(&id.to_be_bytes());
        trex[4..8].copy_from_slice(&1u32.to_be_bytes());
        trex[8..12].copy_from_slice(&(timescale / 10).to_be_bytes());
        let moov = [
            full(b"mvhd", 0, 0, &[0u8; 96]),
            make_box(b"trak", &trak),
            make_box(b"mvex", &full(b"trex", 0, 0, &trex)),
        ]
        .concat();
        [
            make_box(b"ftyp", b"iso6\0\0\0\0iso6dash"),
            make_box(b"moov", &moov),
        ]
        .concat()
    }

    /// Fragmento con `count` muestras de duración por defecto desde `base`.
    pub fn fragment(id: u32, base: u32, count: u32) -> Vec<u8> {
        let tfhd = full(b"tfhd", 0, 0x02_0000, &id.to_be_bytes());
        let tfdt = full(b"tfdt", 0, 0, &base.to_be_bytes());
        let mut trun_body = count.to_be_bytes().to_vec();
        trun_body.extend_from_slice(&0u32.to_be_bytes());
        let trun = full(b"trun", 0, 0x001, &trun_body);
        let traf = make_box(b"traf", &[tfhd, tfdt, trun].concat());
        let moof = make_box(
            b"moof",
            &[full(b"mfhd", 0, 0, &7u32.to_be_bytes()), traf].concat(),
        );
        [moof, make_box(b"mdat", &[1, 2, 3, 4])].concat()
    }

    #[test]
    fn reads_boxes_and_stops_at_partial_ones() {
        let data = [make_box(b"free", &[0; 4]), make_box(b"skip", &[0; 10])].concat();
        let (found, used) = boxes_in(&data, 0, data.len());
        assert_eq!(found.len(), 2);
        assert_eq!(used, data.len());
        let (found, used) = boxes_in(&data, 0, data.len() - 3);
        assert_eq!(found.len(), 1);
        assert_eq!(used, 12);
        // Basura: no entra en pánico.
        assert_eq!(
            boxes_in(&[0, 0, 0, 3, b'x', b'x', b'x', b'x'], 0, 8)
                .0
                .len(),
            0
        );
    }

    #[test]
    fn parses_init_and_fragment_times() {
        let init = parse_init(&init_segment(1, 90_000, b"vide")).expect("init");
        assert_eq!(init.tracks.len(), 1);
        let track = &init.tracks[0];
        assert_eq!(
            (track.id, track.timescale, &track.handler),
            (1, 90_000, b"vide")
        );
        assert_eq!(track.default_duration, 9_000);

        let frag = fragment(1, 180_000, 10);
        let (boxes, _) = boxes_in(&frag, 0, frag.len());
        let moof = &frag[boxes[0].start..boxes[0].end];
        let (start, end) = fragment_span(moof, track).expect("tiempos");
        assert!((start - 2.0).abs() < 1e-9 && (end - 3.0).abs() < 1e-9);
    }

    /// Fragmento con muestras de tamaño 4 y duración por defecto, y su mdat.
    fn fragment_with_samples(id: u32, base: u32, count: u32) -> Vec<u8> {
        let tfhd = full(b"tfhd", 0, 0x02_0000, &id.to_be_bytes());
        let tfdt = full(b"tfdt", 0, 0, &base.to_be_bytes());
        let mut body = count.to_be_bytes().to_vec();
        body.extend_from_slice(&0u32.to_be_bytes()); // data-offset (se fija abajo)
        for _ in 0..count {
            body.extend_from_slice(&4u32.to_be_bytes()); // tamaño
        }
        let trun = full(b"trun", 0, 0x201, &body);
        let traf = make_box(b"traf", &[tfhd, tfdt, trun].concat());
        let mut moof = make_box(
            b"moof",
            &[full(b"mfhd", 0, 0, &1u32.to_be_bytes()), traf].concat(),
        );
        let offset = u32::try_from(moof.len() + 8).expect("tamaño");
        let len = moof.len();
        let at = len - (12 + 4 * count as usize) + 4 + 4;
        put_u32(&mut moof, at, offset).expect("offset");
        let data: Vec<u8> = (0..count * 4).map(|i| i as u8).collect();
        [moof, make_box(b"mdat", &data)].concat()
    }

    #[test]
    fn splits_long_fragments_into_short_pieces() {
        let track = parse_init(&init_segment(1, 1000, b"soun"))
            .expect("init")
            .tracks
            .remove(0);
        // 10 muestras de 100 unidades (0,1 s) desde 2 s: 1 s en total.
        let frag = fragment_with_samples(1, 2000, 10);
        let pieces = split_fragment(&frag, &track, 0.3).expect("trozos");
        assert_eq!(pieces.len(), 4);
        let starts: Vec<f64> = pieces.iter().map(|(start, _)| *start).collect();
        assert_eq!(starts, vec![2.0, 2.3, 2.6, 2.9]);
        // Los bytes de las muestras se conservan en orden y cada trozo se lee bien.
        let mut data = Vec::new();
        for (start, piece) in &pieces {
            let (boxes, used) = boxes_in(piece, 0, piece.len());
            assert_eq!(used, piece.len());
            let moof = &piece[boxes[0].start..boxes[0].end];
            let (from, _) = fragment_span(moof, &track).expect("tiempos");
            assert!((from - start).abs() < 1e-9);
            data.extend_from_slice(&piece[boxes[1].start + 8..boxes[1].end]);
        }
        assert_eq!(data, (0..40).map(|i| i as u8).collect::<Vec<u8>>());
    }

    #[test]
    fn rescales_edit_lists_to_the_new_timescale() {
        // Pista con escala de película 15 360 y una edición de 15 360 unidades (1 s).
        let mut init = init_segment(1, 15_360, b"vide");
        let elst = full(
            b"elst",
            0,
            0,
            &[
                1u32.to_be_bytes(),
                15_360u32.to_be_bytes(),
                0u32.to_be_bytes(),
                0x0001_0000u32.to_be_bytes(),
            ]
            .concat(),
        );
        let edts = make_box(b"edts", &elst);
        // El mvhd de prueba tiene escala 0: se pone 15 360.
        let (top, _) = boxes_in(&init, 0, init.len());
        let moov = top[1];
        let mvhd = child(&init, &moov, b"mvhd").expect("mvhd");
        put_u32(&mut init, mvhd.payload_start() + 12, 15_360).expect("escala");
        let mut track = parse_init(&init).expect("init").tracks.remove(0);
        // Se añade la lista de edición al trak.
        let mut trak = track.trak[8..].to_vec();
        trak.extend(edts);
        track.trak = make_box(b"trak", &trak);
        let out = renumber_trak(&track, 3, 1000).expect("trak");
        let root = read_box(&out, 0, out.len()).expect("trak");
        let elst = child(&out, &child(&out, &root, b"edts").expect("edts"), b"elst").expect("elst");
        assert_eq!(u32_at(&out, elst.payload_start() + 8), Some(1000));
    }

    #[test]
    fn merges_tracks_and_renumbers_fragments() {
        let video = parse_init(&init_segment(1, 90_000, b"vide")).expect("vídeo");
        let audio = parse_init(&init_segment(1, 48_000, b"soun")).expect("audio");
        let merged = build_init(
            &[&video.tracks[0], &audio.tracks[0]],
            video.ftyp.as_deref(),
            Some(12.5),
        )
        .expect("init unido");
        let parsed = parse_init(&merged).expect("se vuelve a leer");
        let ids: Vec<u32> = parsed.tracks.iter().map(|track| track.id).collect();
        assert_eq!(ids, vec![1, 2]);
        assert_eq!(parsed.tracks[1].handler, *b"soun");

        let frag = fragment(1, 0, 4);
        let (boxes, _) = boxes_in(&frag, 0, frag.len());
        assert_eq!(boxes.len(), 2);
        let out = renumber_fragment(&frag, 2, 42).expect("fragmento");
        let root = read_box(&out, 0, out.len()).expect("moof");
        let traf = child(&out, &root, b"traf").expect("traf");
        let tfhd = child(&out, &traf, b"tfhd").expect("tfhd");
        assert_eq!(u32_at(&out, tfhd.payload_start() + 4), Some(2));
        let mfhd = child(&out, &root, b"mfhd").expect("mfhd");
        assert_eq!(u32_at(&out, mfhd.payload_start() + 4), Some(42));
        assert_eq!(out.len(), frag.len());
    }
}
