//! Decoded cover art, kept small and bounded.
//!
//! Covers are decoded off the main thread, downscaled to the size the view
//! asked for, and handed to gpui as ready `RenderImage`s. Passing gpui an
//! encoded `Image` instead routes it through the app-wide asset cache, which
//! never evicts and keeps every cover at full resolution: a 1500px album cover
//! costs ~9 MB there, and a local library holding full-size artwork for every
//! album easily reaches tens of GB.
//!
//! The cache has a byte budget. Covers used least recently are dropped from it
//! and from every window's sprite atlas. A cover used in the last few seconds
//! is never evicted, so a page currently on screen cannot churn.

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui::{App, RenderImage};
use image::{DynamicImage, Frame, ImageDecoder, ImageReader};

/// Decoded pixels the cache may hold before it starts evicting.
const BUDGET_BYTES: usize = 192 * 1024 * 1024;
/// Covers drawn this recently are kept even over budget.
const KEEP_RECENT: Duration = Duration::from_secs(3);

struct Entry {
    image: Arc<RenderImage>,
    bytes: usize,
    last_used: Instant,
}

#[derive(Default)]
pub struct CoverCache {
    entries: HashMap<String, Entry>,
    pending: HashSet<String>,
    bytes: usize,
    evict_scheduled: bool,
}

impl CoverCache {
    pub fn get(&mut self, key: &str) -> Option<Arc<RenderImage>> {
        let entry = self.entries.get_mut(key)?;
        entry.last_used = Instant::now();
        Some(entry.image.clone())
    }

    /// Marks `key` as loading; `false` if it already is.
    pub fn begin(&mut self, key: &str) -> bool {
        self.pending.insert(key.to_string())
    }

    /// Stores a decoded cover. Returns `true` when the cache went over budget
    /// and the caller should schedule [`CoverCache::evict`].
    pub fn insert(&mut self, key: String, image: Arc<RenderImage>) -> bool {
        self.pending.remove(&key);
        let bytes = image_bytes(&image);
        self.bytes += bytes;
        if let Some(old) = self.entries.insert(key, Entry { image, bytes, last_used: Instant::now() }) {
            self.bytes -= old.bytes;
        }
        if self.bytes > BUDGET_BYTES && !self.evict_scheduled {
            self.evict_scheduled = true;
            return true;
        }
        false
    }

    /// A failed load may be retried later (e.g. after reconnecting).
    pub fn failed(&mut self, key: &str) {
        self.pending.remove(key);
    }

    /// Drops least recently used covers down to 75% of the budget. Run it
    /// deferred, outside any window update, so the atlas of every window
    /// (including the one that was drawing) is reachable.
    pub fn evict(&mut self, cx: &mut App) {
        self.evict_scheduled = false;
        let target = BUDGET_BYTES / 4 * 3;
        if self.bytes <= target {
            return;
        }
        let now = Instant::now();
        let mut candidates: Vec<(Instant, String)> = self
            .entries
            .iter()
            .filter(|(_, e)| now.duration_since(e.last_used) > KEEP_RECENT)
            .map(|(k, e)| (e.last_used, k.clone()))
            .collect();
        candidates.sort_unstable_by_key(|(t, _)| *t);
        for (_, key) in candidates {
            if self.bytes <= target {
                break;
            }
            if let Some(entry) = self.entries.remove(&key) {
                self.bytes -= entry.bytes;
                cx.drop_image(entry.image, None);
            }
        }
    }

    pub fn clear(&mut self, cx: &mut App) {
        for (_, entry) in self.entries.drain() {
            cx.drop_image(entry.image, None);
        }
        self.pending.clear();
        self.bytes = 0;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

fn image_bytes(image: &RenderImage) -> usize {
    (0..image.frame_count()).filter_map(|i| image.as_bytes(i)).map(<[u8]>::len).sum()
}

type Job = Box<dyn FnOnce() + Send>;

/// A few dedicated decode threads. Opening a grid asks for every cover at
/// once; decoding them all in parallel on the general background pool spikes
/// memory (each full-size decode is ~9 MB before downscaling, and the
/// allocator keeps the high-water mark) and competes with audio and network
/// work. Jobs of the newest navigation [`epoch`](bump_epoch) run first, so
/// the page just opened fills in before covers a page already left asked
/// for; within an epoch they run in request order (top of the page first).
struct Pool {
    jobs: Mutex<Vec<(u64, u64, Job)>>,
    ready: Condvar,
}

static EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Starts a new navigation epoch: covers requested from now on go first.
pub fn bump_epoch() {
    EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

fn next_job(jobs: &mut Vec<(u64, u64, Job)>) -> Option<Job> {
    let index = (0..jobs.len()).max_by_key(|&i| (jobs[i].0, std::cmp::Reverse(jobs[i].1)))?;
    Some(jobs.swap_remove(index).2)
}

fn pool() -> &'static Pool {
    static POOL: OnceLock<&'static Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        let pool: &'static Pool = Box::leak(Box::new(Pool { jobs: Mutex::new(Vec::new()), ready: Condvar::new() }));
        let workers = std::thread::available_parallelism().map_or(2, |n| n.get() / 2).clamp(1, 3);
        for i in 0..workers {
            std::thread::Builder::new()
                .name(format!("volta-cover-{i}"))
                .spawn(move || loop {
                    let job = {
                        let mut jobs = pool.jobs.lock().unwrap_or_else(|e| e.into_inner());
                        loop {
                            if let Some(job) = next_job(&mut jobs) {
                                break job;
                            }
                            jobs = pool.ready.wait(jobs).unwrap_or_else(|e| e.into_inner());
                        }
                    };
                    job();
                    // A burst of decodes leaves freed buffers in glibc's
                    // per-thread arenas; hand them back once the queue drains.
                    #[cfg(all(target_os = "linux", target_env = "gnu"))]
                    if pool.jobs.lock().map_or(false, |jobs| jobs.is_empty()) {
                        // SAFETY: malloc_trim only releases free heap pages.
                        unsafe {
                            libc::malloc_trim(0);
                        }
                    }
                })
                .expect("spawn cover decode thread");
        }
        pool
    })
}

/// Runs `work` on the decode pool; the receiver resolves with its result.
pub fn run<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> futures::channel::oneshot::Receiver<T> {
    let (tx, rx) = futures::channel::oneshot::channel();
    let pool = pool();
    let epoch = EPOCH.load(std::sync::atomic::Ordering::Relaxed);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let job: Job = Box::new(move || {
        // A dropped receiver means nobody waits for this cover any more.
        if !tx.is_canceled() {
            let _ = tx.send(work());
        }
    });
    pool.jobs.lock().unwrap_or_else(|e| e.into_inner()).push((epoch, seq, job));
    pool.ready.notify_one();
    rx
}

/// Loads a local artwork file as a cover of at most `max_side` pixels,
/// through a downscaled copy kept in `thumbs` (keyed by path, size, mtime and
/// requested size) so later launches skip decoding the full-size original.
pub fn load_local(path: &str, max_side: u32, thumbs: Option<&Path>) -> Option<RenderImage> {
    let meta = std::fs::metadata(path).ok()?;
    let thumb = thumbs.map(|dir| {
        let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
        let fingerprint = format!("{path}\u{1f}{}\u{1f}{modified}\u{1f}{max_side}", meta.len());
        let hash = fingerprint.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100000001b3));
        dir.join(format!("local-{hash:016x}"))
    });
    if let Some(image) = thumb.as_ref().and_then(|p| std::fs::read(p).ok()).and_then(|bytes| decode(&bytes, max_side)) {
        return Some(image);
    }
    let bytes = std::fs::read(path).ok()?;
    let image = decode_dynamic(&bytes, max_side)?;
    if let Some(thumb) = thumb {
        let mut out = Vec::new();
        let encoded = if image.color().has_alpha() {
            image.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        } else {
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90).encode_image(&image.to_rgb8())
        };
        if encoded.is_ok() {
            let _ = std::fs::create_dir_all(thumb.parent().unwrap_or(&thumb));
            let _ = std::fs::write(&thumb, &out);
        }
    }
    Some(to_render_image(image))
}

/// Decodes `bytes` (PNG/JPEG/GIF/WebP/BMP; SVG must be rasterized first),
/// applies EXIF orientation, and shrinks it so neither side exceeds
/// `max_side`. Animated images keep only their first frame.
pub fn decode(bytes: &[u8], max_side: u32) -> Option<RenderImage> {
    decode_dynamic(bytes, max_side).map(to_render_image)
}

fn decode_dynamic(bytes: &[u8], max_side: u32) -> Option<DynamicImage> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().ok();
    let mut image = DynamicImage::from_decoder(decoder).ok()?;
    if let Some(orientation) = orientation {
        image.apply_orientation(orientation);
    }
    let max_side = max_side.max(1);
    if image.width() > max_side || image.height() > max_side {
        // Area averaging: much faster than a filtered resize at these ratios,
        // and alias-free when shrinking.
        image = image.thumbnail(max_side, max_side);
    }
    Some(image)
}

fn to_render_image(image: DynamicImage) -> RenderImage {
    let mut rgba = image.into_rgba8();
    // gpui sprites are BGRA.
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    RenderImage::new(vec![Frame::new(rgba)])
}

/// The BGRA first frame of a cover as an RGBA image, for code that needs the
/// pixels themselves (genre tiles, the media-session artwork file).
pub fn to_rgba(image: &RenderImage) -> Option<image::RgbaImage> {
    let size = image.size(0);
    let mut data = image.as_bytes(0)?.to_vec();
    for pixel in data.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    image::RgbaImage::from_raw(size.width.0 as u32, size.height.0 as u32, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 255]));
        let mut out = Vec::new();
        DynamicImage::ImageRgba8(image).write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn decode_downscales_to_max_side() {
        let image = decode(&png(1500, 1000), 300).unwrap();
        let size = image.size(0);
        assert_eq!((size.width.0, size.height.0), (300, 200));
        assert_eq!(image_bytes(&image), 300 * 200 * 4);
    }

    #[test]
    fn decode_keeps_small_images_and_swaps_to_bgra() {
        let image = decode(&png(40, 40), 300).unwrap();
        assert_eq!(image.size(0).width.0, 40);
        assert_eq!(&image.as_bytes(0).unwrap()[..4], &[30, 20, 10, 255]);
        assert_eq!(to_rgba(&image).unwrap().get_pixel(0, 0).0, [10, 20, 30, 255]);
    }
}
