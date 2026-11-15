//! Port of the pure parts of `src/lib/local-music.ts`: tag parsers (ID3v2,
//! FLAC Vorbis comments and PICTURE, MP4 `ilst`), the library index (tracks,
//! albums, artists, recent albums, artwork), metadata edits, and the local
//! page helpers from App.tsx (`localAlbumOrder`, `similarAlbumsFor`, …).
//!
//! The parsers read through `Read + Seek` so the scanner pulls only the tag
//! region off disk, as the web reads byte ranges.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::model::{AlbumRecord, Artist, Song, Source};

pub const AUDIO_EXTENSIONS: [&str; 14] = ["aac", "aif", "aiff", "alac", "flac", "m4a", "mp3", "oga", "ogg", "opus", "wav", "wave", "weba", "wma"];
pub const IMAGE_EXTENSIONS: [&str; 6] = ["avif", "gif", "jpeg", "jpg", "png", "webp"];
const MAX_METADATA_BYTES: u64 = 4 * 1024 * 1024;
const MP4_TAIL_WINDOWS: [u64; 3] = [64 * 1024, 256 * 1024, 1024 * 1024];

fn extension(path: &str) -> String {
    path.rsplit('.').next().filter(|e| *e != path).unwrap_or("").to_lowercase()
}

pub fn is_supported_audio_file(path: &str) -> bool {
    AUDIO_EXTENSIONS.contains(&extension(path).as_str())
}

pub fn is_supported_image_file(path: &str) -> bool {
    IMAGE_EXTENSIONS.contains(&extension(path).as_str())
}

pub fn path_directory(path: &str) -> String {
    let parts: Vec<&str> = path.split(['/', '\\']).collect();
    parts[..parts.len().saturating_sub(1)].join("/")
}

fn normalized_stem(path: &str) -> String {
    let name = path.split(['/', '\\']).filter(|p| !p.is_empty()).next_back().unwrap_or("");
    let stem = match name.rfind('.') {
        Some(i) => &name[..i],
        None => name,
    };
    stem.trim().to_lowercase()
}

/// Tags read from a file. Artwork is kept out of the cached form.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disc_number: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling_rate: Option<f64>,
    /// Track length in seconds, from the audio stream (not a tag). Older
    /// caches lack it; the scan fills it in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// Embedded cover: bytes and MIME type.
    #[serde(skip)]
    pub artwork: Option<(Vec<u8>, String)>,
}

/// `LocalMetadataPatch`: fields the in-app editor may change.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disc_number: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
}

impl MetadataPatch {
    /// Fields set in `newer` replace these (`writeLocalMetadataOverrides`).
    pub fn merge(&mut self, newer: &MetadataPatch) {
        macro_rules! set {
            ($($f:ident),*) => { $(if newer.$f.is_some() { self.$f = newer.$f.clone(); })* };
        }
        set!(title, artist, album_artist, composer, album, track, disc_number, year, genre);
    }

    fn apply(&self, m: &mut ParsedMetadata) {
        macro_rules! set {
            ($($f:ident),*) => { $(if self.$f.is_some() { m.$f = self.$f.clone(); })* };
        }
        set!(title, artist, album_artist, composer, album, track, disc_number, year, genre);
    }

    pub fn apply_song(&self, s: &mut Song) {
        macro_rules! set {
            ($($f:ident),*) => { $(if self.$f.is_some() { s.$f = self.$f.clone(); })* };
        }
        set!(artist, album_artist, composer, album, track, disc_number, year, genre);
        if let Some(title) = &self.title {
            s.title = title.clone();
        }
    }
}

fn clean_text(value: &str) -> String {
    value.replace('\0', "").trim().to_string()
}

fn split_track(value: &str) -> Option<f64> {
    let digits: String = value.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<f64>().ok()
}

/// `metadataField`.
fn metadata_field(m: &mut ParsedMetadata, key: &str, value: &str) {
    let clean = clean_text(value);
    if clean.is_empty() {
        return;
    }
    let join = |existing: &Option<String>| Some(match existing {
        Some(e) => format!("{e}; {clean}"),
        None => clean.clone(),
    });
    match key {
        "title" => m.title = Some(clean),
        "artist" => m.artist = join(&m.artist),
        "albumartist" | "album artist" => m.album_artist = Some(clean),
        "composer" | "writer" => m.composer = join(&m.composer),
        "album" => m.album = Some(clean),
        // The web only knows `track` and `year`, so Vorbis `TRACKNUMBER` and
        // `DATE` are ignored there too (kept for identical ordering).
        "track" => m.track = split_track(&clean),
        "disc" | "discnumber" | "disc number" => m.disc_number = split_track(&clean),
        "year" => {
            if let Ok(year) = clean.chars().take(4).collect::<String>().parse::<f64>() {
                if year > 0.0 {
                    m.year = Some(year);
                }
            }
        }
        "genre" => m.genre = Some(clean),
        _ => {}
    }
}

fn be32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([*b.get(o).unwrap_or(&0), *b.get(o + 1).unwrap_or(&0), *b.get(o + 2).unwrap_or(&0), *b.get(o + 3).unwrap_or(&0)])
}
fn be24(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([0, *b.get(o).unwrap_or(&0), *b.get(o + 1).unwrap_or(&0), *b.get(o + 2).unwrap_or(&0)])
}
fn be16(b: &[u8], o: usize) -> u32 {
    u32::from(*b.get(o).unwrap_or(&0)) << 8 | u32::from(*b.get(o + 1).unwrap_or(&0))
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([*b.get(o).unwrap_or(&0), *b.get(o + 1).unwrap_or(&0), *b.get(o + 2).unwrap_or(&0), *b.get(o + 3).unwrap_or(&0)])
}
fn sync_safe(b: &[u8], o: usize) -> u32 {
    (u32::from(*b.get(o).unwrap_or(&0)) << 21)
        | (u32::from(*b.get(o + 1).unwrap_or(&0)) << 14)
        | (u32::from(*b.get(o + 2).unwrap_or(&0)) << 7)
        | u32::from(*b.get(o + 3).unwrap_or(&0))
}

fn decode_text(bytes: &[u8], encoding: u8) -> String {
    let text = match encoding {
        1 | 2 => {
            let (mut bytes, mut big) = (bytes, encoding == 2);
            if bytes.len() >= 2 && encoding == 1 {
                match (bytes[0], bytes[1]) {
                    (0xff, 0xfe) => bytes = &bytes[2..],
                    (0xfe, 0xff) => {
                        big = true;
                        bytes = &bytes[2..];
                    }
                    _ => {}
                }
            }
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|c| if big { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) })
                .collect();
            String::from_utf16_lossy(&units)
        }
        0 => bytes.iter().map(|&b| b as char).collect(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    };
    clean_text(&text)
}

fn read_range<R: Read + Seek>(r: &mut R, start: u64, len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    if r.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut filled = 0;
    while filled < len {
        match r.read(&mut buf[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => filled += n,
        }
    }
    buf.truncate(filled);
    buf
}

fn id3_field(id: &str) -> Option<&'static str> {
    Some(match id {
        "TIT2" => "title",
        "TPE1" => "artist",
        "TPE2" => "albumartist",
        "TCOM" => "composer",
        "TALB" => "album",
        "TRCK" => "track",
        "TPOS" => "discnumber",
        "TDRC" | "TYER" => "year",
        "TCON" => "genre",
        _ => return None,
    })
}

fn apply_id3_frame(m: &mut ParsedMetadata, id: &str, frame: &[u8], artwork: bool) {
    if let Some(field) = id3_field(id) {
        if frame.len() > 1 {
            metadata_field(m, field, &decode_text(&frame[1..], frame[0]));
        }
    }
    if id != "APIC" || !artwork || frame.len() <= 4 {
        return;
    }
    let encoding = frame[0];
    let mut cursor = 1;
    while cursor < frame.len() && frame[cursor] != 0 {
        cursor += 1;
    }
    let mime = String::from_utf8_lossy(&frame[1..cursor]).to_string();
    let mime = if mime.is_empty() { "image/jpeg".to_string() } else { mime };
    cursor += 2;
    if cursor >= frame.len() {
        return;
    }
    let wide = encoding == 1 || encoding == 2;
    while cursor < frame.len() && if wide { frame[cursor] != 0 || frame.get(cursor + 1).copied().unwrap_or(0) != 0 } else { frame[cursor] != 0 } {
        cursor += if wide { 2 } else { 1 };
    }
    cursor += if wide { 2 } else { 1 };
    if cursor < frame.len() {
        m.artwork = Some((frame[cursor..].to_vec(), mime));
    }
}

fn read_id3<R: Read + Seek>(r: &mut R, head: &[u8], size: u64, artwork: bool) -> ParsedMetadata {
    let mut m = ParsedMetadata { codec: Some("mp3".into()), ..Default::default() };
    let version = head.get(3).copied().filter(|v| *v != 0).unwrap_or(3);
    let tag_end = size.min(10 + u64::from(sync_safe(head, 6))).min(MAX_METADATA_BYTES);
    let tag = read_range(r, 0, tag_end as usize);
    let mut offset = 10usize;
    if head.get(5).is_some_and(|f| f & 0x40 != 0) {
        let extended = if version >= 4 { sync_safe(&tag, offset) } else { be32(&tag, offset) };
        offset += 4 + extended as usize;
    }
    while offset + 10 <= tag.len() {
        let id = String::from_utf8_lossy(&tag[offset..offset + 4]).to_string();
        if !id.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
            break;
        }
        let frame_size = (if version >= 4 { sync_safe(&tag, offset + 4) } else { be32(&tag, offset + 4) }) as usize;
        if frame_size == 0 || offset + 10 + frame_size > tag.len() {
            break;
        }
        if id3_field(&id).is_some() || (artwork && id == "APIC") {
            apply_id3_frame(&mut m, &id, &tag[offset + 10..offset + 10 + frame_size], artwork);
        }
        offset += 10 + frame_size;
    }
    m
}

fn parse_vorbis_comment(m: &mut ParsedMetadata, block: &[u8]) {
    let mut cursor = 4 + le32(block, 0) as usize;
    let count = le32(block, cursor);
    cursor += 4;
    for _ in 0..count {
        if cursor + 4 > block.len() {
            break;
        }
        let size = le32(block, cursor) as usize;
        cursor += 4;
        if cursor + size > block.len() {
            break;
        }
        let value = String::from_utf8_lossy(&block[cursor..cursor + size]).to_string();
        cursor += size;
        if let Some(sep) = value.find('=').filter(|s| *s > 0) {
            metadata_field(m, &value[..sep].to_lowercase(), &value[sep + 1..]);
        }
    }
}

fn parse_flac_picture(m: &mut ParsedMetadata, block: &[u8]) {
    let mut cursor = 4usize;
    let mime_len = be32(block, cursor) as usize;
    cursor += 4 + mime_len;
    let desc_len = be32(block, cursor) as usize;
    cursor += 4 + desc_len + 16;
    if cursor + 4 > block.len() {
        return;
    }
    let len = be32(block, cursor) as usize;
    let start = cursor + 4;
    if start + len > block.len() {
        return;
    }
    let mime = String::from_utf8_lossy(block.get(8..8 + mime_len).unwrap_or_default()).to_string();
    m.artwork = Some((block[start..start + len].to_vec(), if mime.is_empty() { "image/jpeg".into() } else { mime }));
}

fn read_flac<R: Read + Seek>(r: &mut R, size: u64, artwork: bool) -> ParsedMetadata {
    let mut m = ParsedMetadata { codec: Some("flac".into()), ..Default::default() };
    let mut offset = 4u64;
    for _ in 0..64 {
        let header = read_range(r, offset, 4);
        if header.len() < 4 {
            break;
        }
        let last = header[0] & 0x80 != 0;
        let kind = header[0] & 0x7f;
        let len = u64::from(be24(&header, 1));
        let start = offset + 4;
        if start + len > size {
            break;
        }
        if kind == 0 && len >= 18 {
            // STREAMINFO: sample rate (20 bits) and bits per sample (5 bits).
            let info = read_range(r, start, 18);
            let rate = (u32::from(info[10]) << 12) | (u32::from(info[11]) << 4) | (u32::from(info[12]) >> 4);
            let bits = (((info[12] & 0x01) << 4) | (info[13] >> 4)) + 1;
            if rate > 0 {
                m.sampling_rate = Some(f64::from(rate));
                m.bit_depth = Some(f64::from(bits));
            }
        } else if kind == 4 && len <= MAX_METADATA_BYTES {
            parse_vorbis_comment(&mut m, &read_range(r, start, len as usize));
        } else if kind == 6 && artwork && len <= 16 * MAX_METADATA_BYTES {
            parse_flac_picture(&mut m, &read_range(r, start, len as usize));
        }
        offset = start + len;
        if last {
            break;
        }
    }
    m
}

fn parse_mp4(bytes: &[u8], artwork: bool) -> ParsedMetadata {
    fn walk(b: &[u8], start: usize, end: usize, container: &str, m: &mut ParsedMetadata, artwork: bool) {
        let mut offset = start;
        while offset + 8 <= end {
            let size = be32(b, offset) as usize;
            let kind = &b[offset + 4..offset + 8];
            let box_end = end.min(offset + if size == 0 { end - offset } else { size });
            if box_end <= offset + 8 {
                break;
            }
            let payload = offset + 8 + if kind == b"meta" { 4 } else { 0 };
            if [&b"moov"[..], b"trak", b"mdia", b"minf", b"stbl", b"udta", b"meta", b"ilst"].contains(&kind) {
                walk(b, payload, box_end, std::str::from_utf8(kind).unwrap_or(""), m, artwork);
            } else if kind == b"stsd" && payload + 8 <= box_end {
                let count = be32(b, payload + 4);
                let mut entry = payload + 8;
                for _ in 0..count {
                    if entry + 8 > box_end {
                        break;
                    }
                    let entry_size = be32(b, entry) as usize;
                    let entry_end = box_end.min(entry + entry_size);
                    if entry_size < 8 || entry_end <= entry {
                        break;
                    }
                    match &b[entry + 4..entry + 8].to_ascii_lowercase()[..] {
                        b"alac" => {
                            m.codec = Some("alac".into());
                            if entry_size >= 36 {
                                m.bit_depth = Some(f64::from(be16(b, entry + 26)));
                                m.sampling_rate = Some(f64::from(be32(b, entry + 32) >> 16));
                            }
                        }
                        b"mp4a" => m.codec = Some("aac".into()),
                        b"ac-3" => m.codec = Some("ac-3".into()),
                        b"ec-3" => m.codec = Some("e-ac-3".into()),
                        b"opus" => m.codec = Some("opus".into()),
                        _ => {}
                    }
                    entry = entry_end;
                }
            } else if container == "ilst" {
                let mut child = payload;
                while child + 8 <= box_end {
                    let child_size = be32(b, child) as usize;
                    let child_end = box_end.min(child + if child_size == 0 { box_end - child } else { child_size });
                    if child_end <= child + 8 {
                        break;
                    }
                    if &b[child + 4..child + 8] == b"data" && child_end >= child + 16 {
                        let flags = be32(b, child + 8);
                        let value = &b[child + 16..child_end];
                        let text = || String::from_utf8_lossy(value).to_string();
                        match kind {
                            b"\xa9nam" => metadata_field(m, "title", &text()),
                            b"\xa9ART" => metadata_field(m, "artist", &text()),
                            b"aART" => metadata_field(m, "albumartist", &text()),
                            b"\xa9wrt" => metadata_field(m, "composer", &text()),
                            b"\xa9alb" => metadata_field(m, "album", &text()),
                            b"\xa9day" => metadata_field(m, "year", &text()),
                            b"\xa9gen" => metadata_field(m, "genre", &text()),
                            b"trkn" if value.len() >= 4 => m.track = Some(f64::from(be16(value, 2))),
                            b"disk" if value.len() >= 4 => m.disc_number = Some(f64::from(be16(value, 2))),
                            b"covr" if artwork => {
                                m.artwork = Some((value.to_vec(), if flags == 14 { "image/png".into() } else { "image/jpeg".into() }))
                            }
                            _ => {}
                        }
                    }
                    child = child_end;
                }
            }
            offset = box_end;
        }
    }
    let mut m = ParsedMetadata::default();
    walk(bytes, 0, bytes.len(), "", &mut m, artwork);
    m
}

fn find_mp4_box<'a>(bytes: &'a [u8], wanted: &[u8; 4]) -> Option<&'a [u8]> {
    (4..bytes.len().saturating_sub(4)).find_map(|offset| {
        if &bytes[offset..offset + 4] != wanted {
            return None;
        }
        let start = offset - 4;
        let size = be32(bytes, start) as usize;
        (size >= 8 && start + size <= bytes.len()).then(|| &bytes[start..start + size])
    })
}

/// `mergeMetadata`: the first non-empty value of each field wins.
fn merge(first: ParsedMetadata, second: ParsedMetadata) -> ParsedMetadata {
    macro_rules! pick {
        ($($f:ident),*) => { ParsedMetadata { $($f: first.$f.or(second.$f),)* } };
    }
    pick!(title, artist, album_artist, composer, album, track, disc_number, year, genre, codec, bit_depth, sampling_rate, duration, artwork)
}

fn read_mp4<R: Read + Seek>(r: &mut R, head: &[u8], size: u64, artwork: bool) -> ParsedMetadata {
    let mut offset = 0usize;
    while offset + 8 <= head.len() {
        let box_size = be32(head, offset) as usize;
        if box_size < 8 {
            break;
        }
        if &head[offset + 4..offset + 8] == b"moov" {
            let end = ((offset + box_size) as u64).min(MAX_METADATA_BYTES);
            return parse_mp4(&read_range(r, 0, end as usize), artwork);
        }
        offset += box_size;
    }
    for window in MP4_TAIL_WINDOWS {
        let start = size.saturating_sub(window);
        let tail = read_range(r, start, (size - start) as usize);
        if let Some(found) = find_mp4_box(&tail, b"moov") {
            return merge(parse_mp4(head, artwork), parse_mp4(found, artwork));
        }
        if (tail.len() as u64) < window {
            break;
        }
    }
    parse_mp4(head, artwork)
}

/// `readAudioMetadata`: tags (and the cover, when wanted) from an open file.
pub fn read_audio_metadata<R: Read + Seek>(r: &mut R, size: u64, artwork: bool) -> ParsedMetadata {
    let head = read_range(r, 0, size.min(16 * 1024) as usize);
    if head.starts_with(b"ID3") {
        read_id3(r, &head, size, artwork)
    } else if head.starts_with(b"fLaC") {
        read_flac(r, size, artwork)
    } else if head.get(4..8) == Some(b"ftyp") {
        read_mp4(r, &head, size, artwork)
    } else {
        ParsedMetadata::default()
    }
}

/// One audio file found by the scan.
#[derive(Debug, Clone)]
pub struct ScannedTrack {
    /// Path relative to the library root, `/`-separated.
    pub path: String,
    /// Absolute path on disk.
    pub file: String,
    pub size: f64,
    pub modified: f64,
    pub metadata: ParsedMetadata,
}

/// One image file: relative path and absolute path.
#[derive(Debug, Clone)]
pub struct ScannedImage {
    pub path: String,
    pub file: String,
}

#[derive(Debug, Clone, Default)]
pub struct LocalLibrary {
    pub directory_name: String,
    pub root: String,
    pub tracks: Vec<Song>,
    pub albums: Vec<AlbumRecord>,
    pub recent_albums: Vec<AlbumRecord>,
    pub artists: Vec<Artist>,
    /// Raw tags per relative path, kept apart from in-app edits.
    pub baseline: HashMap<String, ParsedMetadata>,
}

/// `localeCompare(…, { sensitivity: "base", numeric })`: case-insensitive
/// with digit runs compared by value.
pub fn natural_cmp(a: &str, b: &str, numeric: bool) -> Ordering {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    if !numeric {
        return a.cmp(&b);
    }
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        it.next();
                    }
                    s.trim_start_matches('0').to_string()
                };
                let (m, n) = (take(&mut x), take(&mut y));
                let order = m.len().cmp(&n.len()).then(m.cmp(&n));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(c), Some(d)) => {
                if c != d {
                    return c.cmp(&d);
                }
                x.next();
                y.next();
            }
        }
    }
}

fn metadata_order(value: Option<f64>) -> f64 {
    value.filter(|v| v.is_finite()).unwrap_or(f64::MAX)
}

/// `compareMetadataTracks`.
fn compare_tracks(a: &Song, b: &Song) -> Ordering {
    metadata_order(a.disc_number)
        .partial_cmp(&metadata_order(b.disc_number))
        .unwrap_or(Ordering::Equal)
        .then(metadata_order(a.track).partial_cmp(&metadata_order(b.track)).unwrap_or(Ordering::Equal))
        .then(natural_cmp(&a.title, &b.title, false))
        .then(natural_cmp(a.local_path.as_deref().unwrap_or(""), b.local_path.as_deref().unwrap_or(""), true))
}

fn metadata_key(value: &str) -> String {
    value.nfkd().collect::<String>().to_lowercase().trim().to_string()
}

pub fn local_artist_id(name: &str) -> String {
    format!("local-artist:{}", metadata_key(name))
}

pub fn local_album_id(artist: &str, album: &str) -> String {
    format!("local-album:{}\u{1f}{}", metadata_key(artist), metadata_key(album))
}

/// The cover id the app resolves from disk.
pub fn local_cover_id(file: &str) -> String {
    format!("local-file:{file}")
}

fn preferred_artwork_score(stem: &str) -> i32 {
    match stem {
        "cover" => 100,
        "folder" => 95,
        "front" => 90,
        "album" => 85,
        "artwork" => 80,
        "artist" => 75,
        _ => 0,
    }
}

/// `preferredArtworkByDirectory`: the best-named image per folder.
fn preferred_artwork(images: &[ScannedImage]) -> HashMap<String, String> {
    let mut best: HashMap<String, &ScannedImage> = HashMap::new();
    for image in images {
        let directory = path_directory(&image.path);
        let beats = |current: &ScannedImage| {
            let score = preferred_artwork_score(&normalized_stem(&image.path)) - preferred_artwork_score(&normalized_stem(&current.path));
            if score != 0 { score > 0 } else { natural_cmp(&image.path, &current.path, false) == Ordering::Greater }
        };
        if best.get(&directory).is_none_or(|current| beats(current)) {
            best.insert(directory, image);
        }
    }
    best.into_iter().map(|(dir, image)| (dir, image.file.clone())).collect()
}

fn artist_directory(path: &str) -> String {
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    let folders = parts.len().saturating_sub(1);
    if folders >= 2 {
        parts[..parts.len() - 2].join("/")
    } else if folders == 1 {
        parts[..parts.len() - 1].join("/")
    } else {
        String::new()
    }
}

/// The folders whose first track should supply embedded artwork
/// (`artworkRepresentatives`): folders without an image file.
pub fn artwork_representatives(tracks: &[ScannedTrack], images: &[ScannedImage]) -> Vec<usize> {
    let with_images: HashSet<String> = preferred_artwork(images).into_keys().collect();
    let mut seen = HashSet::new();
    let mut indexes = Vec::new();
    for (index, track) in tracks.iter().enumerate() {
        let directory = path_directory(&track.path);
        if seen.insert(directory.clone()) && !with_images.contains(&directory) {
            indexes.push(index);
        }
    }
    indexes
}

fn song_from(track: &ScannedTrack, metadata: &ParsedMetadata) -> Song {
    let name = track.path.rsplit('/').next().unwrap_or(&track.path);
    let stem = match name.rfind('.') {
        Some(i) => &name[..i],
        None => name,
    };
    Song {
        id: format!("local:{}", track.path),
        source: Some(Source::Local),
        title: metadata
            .title
            .clone()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .or_else(|| Some(stem.trim().to_string()).filter(|s| !s.is_empty()))
            .unwrap_or_else(|| "Untitled track".into()),
        artist: metadata.artist.clone(),
        album_artist: metadata.album_artist.clone(),
        composer: metadata.composer.clone(),
        album: metadata.album.clone(),
        track: metadata.track,
        disc_number: metadata.disc_number,
        year: metadata.year,
        genre: metadata.genre.clone(),
        codec: metadata.codec.clone(),
        bit_depth: metadata.bit_depth,
        sampling_rate: metadata.sampling_rate,
        duration: metadata.duration.filter(|d| *d > 0.0),
        suffix: Some(extension(&track.path)).filter(|e| !e.is_empty()),
        size: Some(track.size),
        local_modified: Some(track.modified),
        local_path: Some(track.path.clone()),
        local_file_path: Some(track.file.clone()),
        ..Default::default()
    }
}

/// `libraryFromFiles`: index tracks into albums and artists with artwork.
/// `embedded` maps a folder to an extracted cover file on disk.
pub fn build_library(
    directory_name: &str,
    root: &str,
    mut tracks: Vec<ScannedTrack>,
    images: &[ScannedImage],
    embedded: &HashMap<String, String>,
    overrides: &HashMap<String, MetadataPatch>,
) -> LocalLibrary {
    tracks.sort_by(|a, b| natural_cmp(&a.path, &b.path, true));
    let folder_artwork = preferred_artwork(images);
    let same_stem: HashMap<String, String> =
        images.iter().map(|i| (format!("{}\u{1f}{}", path_directory(&i.path), normalized_stem(&i.path)), i.file.clone())).collect();
    let mut baseline = HashMap::new();
    let mut entries: Vec<(Song, String)> = tracks
        .iter()
        .map(|track| {
            let mut metadata = track.metadata.clone();
            metadata.artwork = None;
            baseline.insert(track.path.clone(), metadata.clone());
            if let Some(patch) = overrides.get(&track.path) {
                patch.apply(&mut metadata);
            }
            (song_from(track, &metadata), track.path.clone())
        })
        .collect();
    entries.sort_by(|a, b| compare_tracks(&a.0, &b.0));

    let mut albums: IndexMap<String, AlbumRecord> = IndexMap::new();
    let mut artists: IndexMap<String, Artist> = IndexMap::new();
    let mut artist_albums: HashMap<String, HashSet<String>> = HashMap::new();
    let mut album_artwork: HashMap<String, String> = HashMap::new();
    let mut out_tracks = Vec::with_capacity(entries.len());
    for (mut track, path) in entries {
        let trimmed = |v: &Option<String>| v.as_deref().map(str::trim).filter(|a| !a.is_empty()).map(str::to_string);
        let artist = trimmed(&track.artist).unwrap_or_else(|| "Unknown artist".into());
        let album_artist = trimmed(&track.album_artist).unwrap_or_else(|| artist.clone());
        let album = trimmed(&track.album).unwrap_or_else(|| "Unknown album".into());
        let album_artist_id = local_artist_id(&album_artist);
        let album_id = local_album_id(&album_artist, &album);
        let directory = path_directory(&path);
        let cover = embedded
            .get(&directory)
            .or_else(|| same_stem.get(&format!("{directory}\u{1f}{}", normalized_stem(&path))))
            .or_else(|| folder_artwork.get(&directory))
            .cloned();
        track.artist = Some(artist.clone());
        track.artist_id = Some(local_artist_id(&artist));
        track.album_artist = Some(album_artist.clone());
        track.album = Some(album.clone());
        track.album_id = Some(album_id.clone());
        track.local_artwork_url = cover.clone();
        track.cover_art = cover.as_deref().map(local_cover_id);
        if let Some(cover) = &cover {
            album_artwork.entry(album_id.clone()).or_insert_with(|| cover.clone());
        }
        let record = albums.entry(album_id.clone()).or_insert_with(|| AlbumRecord {
            id: album_id.clone(),
            source: Some(Source::Local),
            name: Some(album.clone()),
            artist: Some(album_artist.clone()),
            artist_id: Some(album_artist_id.clone()),
            year: track.year,
            genre: track.genre.clone(),
            song: Some(Vec::new()),
            song_count: Some(0.0),
            duration: Some(0.0),
            local_modified: Some(0.0),
            ..Default::default()
        });
        if record.year.is_none() {
            record.year = track.year;
        }
        if record.genre.is_none() {
            record.genre = track.genre.clone();
        }
        record.local_modified = Some(record.local_modified.unwrap_or(0.0).max(track.local_modified.unwrap_or(0.0)));
        if let Some(art) = album_artwork.get(&album_id) {
            record.local_artwork_url = Some(art.clone());
            record.cover_art = Some(local_cover_id(art));
        }
        record.duration = Some(record.duration.unwrap_or(0.0) + track.duration.unwrap_or(0.0));
        let songs = record.song.get_or_insert_with(Vec::new);
        songs.push(track.clone());
        record.song_count = Some(songs.len() as f64);
        let record_art = record.local_artwork_url.clone();

        let artist_record = artists.entry(album_artist_id.clone()).or_insert_with(|| Artist {
            id: album_artist_id.clone(),
            source: Some(Source::Local),
            name: album_artist.clone(),
            album: Some(Vec::new()),
            album_count: Some(0.0),
            ..Default::default()
        });
        artist_albums.entry(album_artist_id.clone()).or_default().insert(album_id.clone());
        if artist_record.local_artwork_url.is_none() {
            if let Some(art) = folder_artwork.get(&artist_directory(&path)).cloned().or(record_art) {
                artist_record.cover_art = Some(local_cover_id(&art));
                artist_record.local_artwork_url = Some(art);
            }
        }
        artist_record.local_modified = Some(artist_record.local_modified.unwrap_or(0.0).max(track.local_modified.unwrap_or(0.0)));
        out_tracks.push(track);
    }
    // Artists hold their albums in first-seen order, built after the albums
    // are complete (the web pushes live album objects).
    let album_order: Vec<String> = albums.keys().cloned().collect();
    for (id, artist) in artists.iter_mut() {
        let owned = artist_albums.get(id).cloned().unwrap_or_default();
        let list: Vec<AlbumRecord> = album_order.iter().filter(|a| owned.contains(*a)).filter_map(|a| albums.get(a).cloned()).collect();
        artist.album_count = Some(list.len() as f64);
        artist.album = Some(list);
    }
    let mut album_list: Vec<AlbumRecord> = albums.into_values().collect();
    album_list.sort_by(|a, b| natural_cmp(a.name.as_deref().unwrap_or(""), b.name.as_deref().unwrap_or(""), false));
    let mut artist_list: Vec<Artist> = artists.into_values().collect();
    artist_list.sort_by(|a, b| natural_cmp(&a.name, &b.name, false));
    let mut recent = album_list.clone();
    recent.sort_by(|a, b| {
        b.local_modified
            .unwrap_or(0.0)
            .partial_cmp(&a.local_modified.unwrap_or(0.0))
            .unwrap_or(Ordering::Equal)
            .then(natural_cmp(a.name.as_deref().unwrap_or(""), b.name.as_deref().unwrap_or(""), false))
    });
    LocalLibrary {
        directory_name: directory_name.to_string(),
        root: root.to_string(),
        tracks: out_tracks,
        albums: album_list,
        recent_albums: recent,
        artists: artist_list,
        baseline,
    }
}

/// `localAlbumOrder`.
pub fn local_album_order(library: &LocalLibrary, sort: &str) -> Vec<AlbumRecord> {
    match sort {
        "newest" => library.recent_albums.clone(),
        "alphabeticalByArtist" => {
            let mut albums = library.albums.clone();
            albums.sort_by(|a, b| {
                natural_cmp(a.artist.as_deref().unwrap_or(""), b.artist.as_deref().unwrap_or(""), false)
                    .then(natural_cmp(a.name.as_deref().unwrap_or(""), b.name.as_deref().unwrap_or(""), false))
            });
            albums
        }
        _ => library.albums.clone(),
    }
}

fn genre_key(genre: Option<&str>) -> String {
    genre.unwrap_or("").split([',', ';', '|', '/']).next().unwrap_or("").trim().to_lowercase()
}

/// `similarAlbumsFor`.
pub fn similar_albums_for(source: &AlbumRecord, library: &[AlbumRecord]) -> Vec<AlbumRecord> {
    let source_genre = genre_key(source.genre.as_deref());
    let mut scored: Vec<(f64, &AlbumRecord)> = library
        .iter()
        .filter(|a| a.id != source.id)
        .map(|album| {
            let same_genre = !source_genre.is_empty() && genre_key(album.genre.as_deref()) == source_genre;
            let same_artist = source.artist_id.is_some() && source.artist_id == album.artist_id;
            let distance = match (source.year, album.year) {
                (Some(a), Some(b)) if a != 0.0 && b != 0.0 => (a - b).abs(),
                _ => 100.0,
            };
            let score = if same_genre { 100.0 } else { 0.0 } + if same_artist { 25.0 } else { 0.0 } + (10.0 - distance.min(10.0)).max(0.0);
            (score, album)
        })
        .filter(|(score, album)| *score > 10.0 || genre_key(album.genre.as_deref()) == source_genre)
        .collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then(crate::format::album_name(a.1).cmp(crate::format::album_name(b.1)))
    });
    scored.into_iter().take(6).map(|(_, a)| a.clone()).collect()
}

/// `similarArtistsFor`.
pub fn similar_artists_for(source: &Artist, library: &[AlbumRecord]) -> Vec<Artist> {
    let genres: HashSet<String> =
        source.album.iter().flatten().map(|a| genre_key(a.genre.as_deref())).filter(|g| !g.is_empty()).collect();
    let mut candidates: IndexMap<String, (Artist, u32)> = IndexMap::new();
    for album in library {
        let Some(name) = album.artist.as_deref().map(str::trim).filter(|n| !n.is_empty()) else { continue };
        let id = album.artist_id.clone().unwrap_or_else(|| name.to_string());
        if id == source.id {
            continue;
        }
        let genre = genre_key(album.genre.as_deref());
        if genre.is_empty() || !genres.contains(&genre) {
            continue;
        }
        let entry = candidates.entry(id.clone()).or_insert_with(|| {
            (Artist { id, name: name.to_string(), cover_art: album.cover_art.clone(), album_count: Some(0.0), ..Default::default() }, 0)
        });
        entry.0.album_count = Some(entry.0.album_count.unwrap_or(0.0) + 1.0);
        entry.1 += 1;
    }
    let mut list: Vec<(Artist, u32)> = candidates.into_values().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.name.cmp(&b.0.name)));
    list.into_iter().take(6).map(|(a, _)| a).collect()
}

/// `applyLocalMetadataEdits`: re-index after an in-app edit.
pub fn apply_metadata_edits(library: &LocalLibrary, edits: &HashMap<String, MetadataPatch>) -> LocalLibrary {
    let images: Vec<ScannedImage> = Vec::new();
    let embedded: HashMap<String, String> = library
        .tracks
        .iter()
        .filter_map(|t| Some((path_directory(t.local_path.as_ref()?), t.local_artwork_url.clone()?)))
        .collect();
    let tracks: Vec<ScannedTrack> = library
        .tracks
        .iter()
        .filter_map(|t| {
            let path = t.local_path.clone()?;
            Some(ScannedTrack {
                metadata: library.baseline.get(&path).cloned().unwrap_or_default(),
                path,
                file: t.local_file_path.clone().unwrap_or_default(),
                size: t.size.unwrap_or(0.0),
                modified: t.local_modified.unwrap_or(0.0),
            })
        })
        .collect();
    let mut rebuilt = build_library(&library.directory_name, &library.root, tracks, &images, &embedded, edits);
    // Per-track covers (same-stem images) survive the rebuild.
    let covers: HashMap<String, (Option<String>, Option<String>)> =
        library.tracks.iter().map(|t| (t.id.clone(), (t.local_artwork_url.clone(), t.cover_art.clone()))).collect();
    for track in &mut rebuilt.tracks {
        if let Some((url, cover)) = covers.get(&track.id) {
            track.local_artwork_url = url.clone();
            track.cover_art = cover.clone();
        }
    }
    rebuilt
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn id3_frame(id: &str, text: &str) -> Vec<u8> {
        let mut payload = vec![3u8];
        payload.extend_from_slice(text.as_bytes());
        let mut frame = id.as_bytes().to_vec();
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(&[0, 0]);
        frame.extend(payload);
        frame
    }

    #[test]
    fn parses_id3v23_text_frames() {
        let frames: Vec<u8> =
            [id3_frame("TIT2", "Blue"), id3_frame("TPE1", "Miles"), id3_frame("TRCK", "3/12"), id3_frame("TYER", "1999")].concat();
        let size = frames.len() as u32;
        let mut file = b"ID3\x03\x00\x00".to_vec();
        file.extend_from_slice(&[(size >> 21) as u8 & 0x7f, (size >> 14) as u8 & 0x7f, (size >> 7) as u8 & 0x7f, size as u8 & 0x7f]);
        file.extend(frames);
        file.extend_from_slice(&[0xff; 64]);
        let len = file.len() as u64;
        let m = read_audio_metadata(&mut Cursor::new(file), len, false);
        assert_eq!(m.title.as_deref(), Some("Blue"));
        assert_eq!(m.artist.as_deref(), Some("Miles"));
        assert_eq!(m.track, Some(3.0));
        assert_eq!(m.year, Some(1999.0));
        assert_eq!(m.codec.as_deref(), Some("mp3"));
    }

    #[test]
    fn parses_flac_vorbis_comments_and_streaminfo() {
        let comments = ["TITLE=Song", "ARTIST=A", "ARTIST=B", "TRACK=2", "ALBUM=Rec"];
        let mut block = Vec::new();
        block.extend_from_slice(&4u32.to_le_bytes());
        block.extend_from_slice(b"volt");
        block.extend_from_slice(&(comments.len() as u32).to_le_bytes());
        for c in comments {
            block.extend_from_slice(&(c.len() as u32).to_le_bytes());
            block.extend_from_slice(c.as_bytes());
        }
        let mut info = [0u8; 18];
        // 44100 Hz, 16 bits.
        let rate: u32 = 44100;
        info[10] = (rate >> 12) as u8;
        info[11] = (rate >> 4) as u8;
        info[12] = ((rate & 0xf) << 4) as u8;
        info[13] = 15 << 4;
        let mut file = b"fLaC".to_vec();
        file.extend_from_slice(&[0, 0, 0, 18]);
        file.extend_from_slice(&info);
        let len = block.len() as u32;
        file.extend_from_slice(&[0x84, (len >> 16) as u8, (len >> 8) as u8, len as u8]);
        file.extend(block);
        let size = file.len() as u64;
        let m = read_audio_metadata(&mut Cursor::new(file), size, false);
        assert_eq!(m.title.as_deref(), Some("Song"));
        assert_eq!(m.artist.as_deref(), Some("A; B"));
        assert_eq!(m.track, Some(2.0));
        assert_eq!(m.sampling_rate, Some(44100.0));
        assert_eq!(m.bit_depth, Some(16.0));
    }

    fn scanned(path: &str, title: &str, artist: &str, album: &str, track: f64) -> ScannedTrack {
        ScannedTrack {
            path: path.into(),
            file: format!("/music/{path}"),
            size: 1.0,
            modified: 1.0,
            metadata: ParsedMetadata {
                title: Some(title.into()),
                artist: Some(artist.into()),
                album: Some(album.into()),
                track: Some(track),
                ..Default::default()
            },
        }
    }

    #[test]
    fn builds_albums_artists_and_artwork() {
        let tracks = vec![
            scanned("A/Rec/02.flac", "Two", "Ann", "Rec", 2.0),
            scanned("A/Rec/01.flac", "One", "Ann", "Rec", 1.0),
            scanned("B/x.mp3", "X", "bob", "Other", 1.0),
        ];
        let images = vec![ScannedImage { path: "A/Rec/cover.jpg".into(), file: "/music/A/Rec/cover.jpg".into() }];
        let lib = build_library("music", "/music", tracks, &images, &HashMap::new(), &HashMap::new());
        assert_eq!(lib.albums.len(), 2);
        assert_eq!(lib.artists.len(), 2);
        let rec = lib.albums.iter().find(|a| a.name.as_deref() == Some("Rec")).unwrap();
        assert_eq!(rec.song.as_ref().unwrap()[0].title, "One");
        assert_eq!(rec.local_artwork_url.as_deref(), Some("/music/A/Rec/cover.jpg"));
        assert_eq!(rec.id, "local-album:ann\u{1f}rec");
        let mut edits = HashMap::new();
        edits.insert("B/x.mp3".to_string(), MetadataPatch { album: Some("Rec".into()), artist: Some("Ann".into()), ..Default::default() });
        let edited = apply_metadata_edits(&lib, &edits);
        assert_eq!(edited.albums.len(), 1);
        assert_eq!(natural_cmp("track 10", "track 9", true), Ordering::Greater);
    }
}
