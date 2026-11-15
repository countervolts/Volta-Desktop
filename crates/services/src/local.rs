//! Local music scanning (`scanLocalDirectory`, `readAudioMetadataBatch`,
//! `readFolderArtwork`, the IndexedDB caches) on the filesystem.
//!
//! Tags are cached by relative path with the file's size and mtime, so an
//! unchanged library parses nothing on the next scan. Embedded covers are
//! extracted once per folder into `<data dir>/local-artwork/`.

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use volta_logic::local_music::{
    LocalLibrary, MetadataPatch, ParsedMetadata, ScannedImage, ScannedTrack, artwork_representatives, build_library,
    is_supported_audio_file, is_supported_image_file, path_directory, read_audio_metadata,
};

/// Storage key for the chosen folder (the web's saved directory handle).
pub const LOCAL_DIRECTORY_KEY: &str = "volta-local-music-directory";
/// Storage key for the tag cache (`track-metadata` store).
pub const LOCAL_METADATA_KEY: &str = "volta-local-metadata";
/// Storage key for in-app metadata edits (`track-metadata-overrides`).
pub const LOCAL_OVERRIDES_KEY: &str = "volta-local-metadata-overrides";

/// `CachedMetadata`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedMetadata {
    pub size: f64,
    pub last_modified: f64,
    pub metadata: ParsedMetadata,
}

pub type MetadataCache = HashMap<String, CachedMetadata>;

const READERS: usize = 8;

fn modified_ms(meta: &std::fs::Metadata) -> f64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// Runs `task` over `items` on a small thread pool, keeping order.
fn pool<T: Sync, U: Send + Default>(items: &[T], task: impl Fn(&T) -> U + Sync) -> Vec<U> {
    let mut out: Vec<U> = Vec::with_capacity(items.len());
    out.resize_with(items.len(), U::default);
    let chunk = items.len().div_ceil(READERS).max(1);
    std::thread::scope(|scope| {
        for (inputs, outputs) in items.chunks(chunk).zip(out.chunks_mut(chunk)) {
            let task = &task;
            scope.spawn(move || {
                for (input, output) in inputs.iter().zip(outputs.iter_mut()) {
                    *output = task(input);
                }
            });
        }
    });
    out
}

fn read_tags(path: &Path, size: u64, artwork: bool) -> ParsedMetadata {
    match File::open(path) {
        Ok(mut file) => read_audio_metadata(&mut file, size, artwork),
        Err(_) => ParsedMetadata::default(),
    }
}

fn artwork_extension(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "jpg",
    }
}

/// Result of a scan: the library plus the refreshed tag cache.
pub struct Scan {
    pub library: LocalLibrary,
    pub cache: MetadataCache,
}

/// Walks `root`, reads tags (reusing `cache`), extracts folder covers into
/// `artwork_dir`, applies `overrides`, and builds the library.
pub fn scan_directory(root: &Path, cache: &MetadataCache, overrides: &HashMap<String, MetadataPatch>, artwork_dir: &Path) -> Scan {
    let mut audio: Vec<(PathBuf, String, u64, f64)> = Vec::new();
    let mut images: Vec<ScannedImage> = Vec::new();
    for entry in walkdir::WalkDir::new(root).follow_links(true).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path().to_path_buf();
        let rel = relative(root, &path);
        if is_supported_audio_file(&rel) {
            let Ok(meta) = entry.metadata() else { continue };
            audio.push((path, rel, meta.len(), modified_ms(&meta)));
        } else if is_supported_image_file(&rel) {
            images.push(ScannedImage { path: rel, file: path.to_string_lossy().into_owned() });
        }
    }
    let tracks: Vec<ScannedTrack> = pool(&audio, |(path, rel, size, modified)| {
        let mut metadata = match cache.get(rel) {
            Some(hit) if hit.size == *size as f64 && hit.last_modified == *modified => hit.metadata.clone(),
            _ => read_tags(path, *size, false),
        };
        // Cached entries from before durations were stored get one now.
        if metadata.duration.is_none() {
            metadata.duration = Some(volta_audio::probe_duration(path).unwrap_or(0.0));
        }
        Some(ScannedTrack { path: rel.clone(), file: path.to_string_lossy().into_owned(), size: *size as f64, modified: *modified, metadata })
    })
    .into_iter()
    .flatten()
    .collect();

    // Embedded covers: one representative track per folder without an image.
    let _ = std::fs::create_dir_all(artwork_dir);
    let representatives = artwork_representatives(&tracks, &images);
    let picks: Vec<&ScannedTrack> = representatives.iter().filter_map(|&i| tracks.get(i)).collect();
    let embedded: HashMap<String, String> = pool(&picks, |track| {
        let directory = path_directory(&track.path);
        let fingerprint = format!("{directory}\u{1f}{}:{}", track.size, track.modified);
        let hash = fingerprint.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100000001b3));
        for ext in ["jpg", "png", "webp", "gif"] {
            let existing = artwork_dir.join(format!("{hash:016x}.{ext}"));
            if existing.exists() {
                return Some((directory, existing.to_string_lossy().into_owned()));
            }
        }
        let metadata = read_tags(Path::new(&track.file), track.size as u64, true);
        let (bytes, mime) = metadata.artwork?;
        let file = artwork_dir.join(format!("{hash:016x}.{}", artwork_extension(&mime)));
        std::fs::write(&file, bytes).ok()?;
        Some((directory, file.to_string_lossy().into_owned()))
    })
    .into_iter()
    .flatten()
    .collect();

    let cache: MetadataCache = tracks
        .iter()
        .map(|t| (t.path.clone(), CachedMetadata { size: t.size, last_modified: t.modified, metadata: t.metadata.clone() }))
        .collect();
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Music".into());
    let library = build_library(&name, &root.to_string_lossy(), tracks, &images, &embedded, overrides);
    Scan { library, cache }
}

pub fn read_cache(raw: &str) -> MetadataCache {
    serde_json::from_str(raw).unwrap_or_default()
}

pub fn read_overrides(raw: &str) -> HashMap<String, MetadataPatch> {
    serde_json::from_str(raw).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_a_folder_with_untagged_files() {
        let root = std::env::temp_dir().join(format!("volta-scan-test-{}", std::process::id()));
        let album = root.join("Artist").join("Record");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("01 Intro.mp3"), b"not really audio").unwrap();
        std::fs::write(album.join("cover.jpg"), b"jpg").unwrap();
        std::fs::write(root.join("notes.txt"), b"skip").unwrap();
        let scan = scan_directory(&root, &MetadataCache::new(), &HashMap::new(), &root.join(".art"));
        assert_eq!(scan.library.tracks.len(), 1);
        assert_eq!(scan.library.tracks[0].title, "01 Intro");
        assert!(scan.library.albums[0].local_artwork_url.as_deref().unwrap().ends_with("cover.jpg"));
        assert_eq!(scan.cache.len(), 1);
        std::fs::remove_dir_all(&root).unwrap();
    }
}

/// Lyrics from a sidecar file next to a local track: `Song.flac` ->
/// `Song.lrc` (or `.LRC`). Parsed as TTML, LRC or plain text, like a
/// clipboard import.
pub fn sidecar_lyrics(audio_path: &str) -> Option<volta_logic::lyrics::Lyrics> {
    let path = Path::new(audio_path);
    let bytes = ["lrc", "LRC"].iter().find_map(|ext| std::fs::read(path.with_extension(ext)).ok())?;
    let text = String::from_utf8_lossy(&bytes);
    volta_logic::lyrics::parse_lyrics_text(text.trim_start_matches('\u{feff}'))
}
