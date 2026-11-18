//! Always-on log files, the same on every OS.
//!
//! Layout under `<data dir>/logs/`:
//!
//! ```text
//! latest/                      this session
//!   volta.log                  everything Volta does: startup, playback, network, errors, panics
//!   performance.log            memory, CPU, UI build times, slow loads, UI stalls
//!   system.txt                 versions, OS, hardware: what a bug report needs
//! session-2026-11-17_21-35-02/ the previous session, named by when it started
//! session-2026-11-16_09-12-44/ the one before
//! ```
//!
//! Only three sessions are kept: at launch `latest` is renamed after its own
//! start time and everything but the two newest `session-*` folders is deleted.
//! Logging never fails the app: if the folder can't be created the app runs
//! without files.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// Previous sessions kept next to `latest`.
pub const KEEP_PREVIOUS: usize = 2;
/// A single log file is capped; the previous half is kept as `<name>.1.log`.
const MAX_FILE_BYTES: u64 = 24 * 1024 * 1024;
const STAMP: &str = "%Y-%m-%d_%H-%M-%S";
/// Target used for performance lines; they go to `performance.log` only.
pub const PERF: &str = "perf";

static START: OnceLock<Instant> = OnceLock::new();
static MARKS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
static COVER_LOADS: AtomicU64 = AtomicU64::new(0);
static COVER_NANOS: AtomicU64 = AtomicU64::new(0);
static COVER_FROM_NETWORK: AtomicU64 = AtomicU64::new(0);
static COVER_FAILED: AtomicU64 = AtomicU64::new(0);
static SINK: Mutex<Option<Sink>> = Mutex::new(None);
static LATEST: OnceLock<PathBuf> = OnceLock::new();

struct Writer {
    out: BufWriter<File>,
    path: PathBuf,
    bytes: u64,
}

impl Writer {
    fn open(path: PathBuf) -> io::Result<Self> {
        let file = fs::OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self { out: BufWriter::new(file), path, bytes: 0 })
    }

    fn line(&mut self, text: &str) {
        if self.bytes > MAX_FILE_BYTES {
            let _ = self.out.flush();
            let backup = self.path.with_extension("1.log");
            let _ = fs::rename(&self.path, &backup);
            if let Ok(file) = fs::OpenOptions::new().create(true).append(true).open(&self.path) {
                self.out = BufWriter::new(file);
                self.bytes = 0;
            }
        }
        if self.out.write_all(text.as_bytes()).and_then(|_| self.out.write_all(b"\n")).is_ok() {
            self.bytes += text.len() as u64 + 1;
        }
    }
}

struct Sink {
    app: Writer,
    perf: Writer,
}

/// `<data dir>/logs`.
pub fn root() -> Option<PathBuf> {
    volta_services::storage::data_dir().map(|dir| dir.join("logs"))
}

/// The folder of the running session, once logging has started.
pub fn latest_dir() -> Option<&'static Path> {
    LATEST.get().map(PathBuf::as_path)
}

/// Moves the previous `latest` aside under its start time and deletes the
/// oldest sessions so only `keep` of them remain.
fn rotate(root: &Path, keep: usize) -> io::Result<()> {
    fs::create_dir_all(root)?;
    let latest = root.join("latest");
    if latest.is_dir() {
        let stamp = fs::read_to_string(latest.join(".started"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                let modified = fs::metadata(&latest).and_then(|m| m.modified()).ok()?;
                Some(chrono::DateTime::<chrono::Local>::from(modified).format(STAMP).to_string())
            })
            .unwrap_or_else(|| "unknown".into());
        let mut target = root.join(format!("session-{stamp}"));
        let mut n = 2;
        while target.exists() {
            target = root.join(format!("session-{stamp}-{n}"));
            n += 1;
        }
        fs::rename(&latest, &target)?;
    }
    let mut sessions: Vec<PathBuf> = fs::read_dir(root)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("session-")))
        .collect();
    // The timestamp is in the name, so name order is time order.
    sessions.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    for old in sessions.into_iter().skip(keep) {
        let _ = fs::remove_dir_all(old);
    }
    Ok(())
}

/// Starts file logging and the background sampler. Returns the session folder.
pub fn start() -> Option<PathBuf> {
    START.get_or_init(Instant::now);
    // Offscreen screenshot runs aren't real sessions.
    if std::env::var_os("VOLTA_SHOT").is_some() || std::env::var_os("VOLTA_NO_LOG_FILES").is_some() {
        return None;
    }
    let root = root()?;
    let opened = (|| -> io::Result<PathBuf> {
        rotate(&root, KEEP_PREVIOUS)?;
        let latest = root.join("latest");
        fs::create_dir_all(&latest)?;
        fs::write(latest.join(".started"), chrono::Local::now().format(STAMP).to_string())?;
        let sink = Sink { app: Writer::open(latest.join("volta.log"))?, perf: Writer::open(latest.join("performance.log"))? };
        *SINK.lock() = Some(sink);
        Ok(latest)
    })();
    match opened {
        Ok(latest) => {
            let _ = LATEST.set(latest.clone());
            install_panic_hook();
            spawn_sampler();
            Some(latest)
        }
        Err(error) => {
            eprintln!("volta: log files are unavailable ({error})");
            None
        }
    }
}

fn stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

/// One formatted line: time, level, thread, target, message.
fn format_record(record: &log::Record) -> String {
    let thread = std::thread::current();
    format!("{} {:<5} [{}] {}: {}", stamp(), record.level(), thread.name().unwrap_or("worker"), record.target(), record.args())
}

/// Which records reach the files. Volta's own crates are logged in detail;
/// dependencies only when they have something to say.
pub fn file_enabled(target: &str, level: log::Level) -> bool {
    if target == PERF || target.starts_with("volta") {
        level <= log::Level::Debug
    } else if target.starts_with("gpui") {
        level <= log::Level::Info
    } else {
        level <= log::Level::Warn
    }
}

pub fn write(record: &log::Record) {
    let line = format_record(record);
    let mut guard = SINK.lock();
    let Some(sink) = guard.as_mut() else { return };
    let writer = if record.target() == PERF { &mut sink.perf } else { &mut sink.app };
    writer.line(&line);
    // Warnings and errors are what matters after a crash, so don't leave them buffered.
    if record.level() <= log::Level::Warn {
        let _ = writer.out.flush();
    }
}

pub fn flush() {
    if let Some(sink) = SINK.lock().as_mut() {
        let _ = sink.app.out.flush();
        let _ = sink.perf.out.flush();
    }
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        log::error!(target: "volta::panic", "panic: {info}\n{backtrace}");
        flush();
        previous(info);
    }));
}

// ---- UI health -----------------------------------------------------------

/// Milliseconds since start at the last UI heartbeat.
static HEARTBEAT: AtomicU64 = AtomicU64::new(0);
static STALLED: AtomicBool = AtomicBool::new(false);
static RENDERS: AtomicU64 = AtomicU64::new(0);
static RENDER_NANOS: AtomicU64 = AtomicU64::new(0);
static RENDER_MAX_NANOS: AtomicU64 = AtomicU64::new(0);
static SLOW_RENDERS: AtomicU64 = AtomicU64::new(0);

fn uptime() -> Duration {
    START.get().map(Instant::elapsed).unwrap_or_default()
}

/// Called from the UI thread on a timer; the sampler thread watches it.
pub fn heartbeat() {
    HEARTBEAT.store(uptime().as_millis() as u64, Ordering::Relaxed);
}

/// Times how long a view takes to build its element tree.
pub struct RenderTimer(Instant);

impl RenderTimer {
    pub fn start() -> Self {
        Self(Instant::now())
    }
}

impl Drop for RenderTimer {
    fn drop(&mut self) {
        let elapsed = self.0.elapsed();
        let nanos = elapsed.as_nanos() as u64;
        RENDERS.fetch_add(1, Ordering::Relaxed);
        RENDER_NANOS.fetch_add(nanos, Ordering::Relaxed);
        RENDER_MAX_NANOS.fetch_max(nanos, Ordering::Relaxed);
        if elapsed > Duration::from_millis(33) {
            SLOW_RENDERS.fetch_add(1, Ordering::Relaxed);
            if elapsed > Duration::from_millis(100) {
                log::warn!(target: PERF, "slow UI build: {:.1} ms", elapsed.as_secs_f64() * 1000.0);
            }
        }
    }
}

/// A startup milestone, logged once with the time since launch. The sequence of
/// these shows where a slow start (a huge library) spends its time.
pub fn mark(name: &'static str) {
    {
        let mut marks = MARKS.lock();
        if marks.contains(&name) {
            return;
        }
        marks.push(name);
    }
    log::info!(target: PERF, "startup: {name} at +{} ms", uptime().as_millis());
}

/// One cover finished loading (decoded and resized).
pub fn cover_loaded(elapsed: Duration, from_network: bool, ok: bool) {
    COVER_LOADS.fetch_add(1, Ordering::Relaxed);
    COVER_NANOS.fetch_add(elapsed.as_nanos() as u64, Ordering::Relaxed);
    if from_network {
        COVER_FROM_NETWORK.fetch_add(1, Ordering::Relaxed);
    }
    if !ok {
        COVER_FAILED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Logs how long `what` took, as a performance line.
pub fn timed(what: &str, started: Instant) {
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    if ms >= 250.0 {
        log::warn!(target: PERF, "{what}: {ms:.0} ms (slow)");
    } else {
        log::info!(target: PERF, "{what}: {ms:.0} ms");
    }
}

const TICK: Duration = Duration::from_millis(500);
/// A sample every this many ticks (10 s).
const SAMPLE_EVERY: u32 = 20;
/// The UI thread counts as stalled after this long without a heartbeat.
const STALL_MS: u64 = 3000;

fn spawn_sampler() {
    let _ = std::thread::Builder::new().name("volta-log".into()).spawn(|| {
        let mut ticks = 0u32;
        let mut last_cpu = process_cpu_seconds();
        let mut last_at = Instant::now();
        heartbeat();
        loop {
            std::thread::sleep(TICK);
            ticks += 1;
            flush();
            let now = uptime().as_millis() as u64;
            let late = now.saturating_sub(HEARTBEAT.load(Ordering::Relaxed));
            if late > STALL_MS && !STALLED.swap(true, Ordering::Relaxed) {
                log::warn!(target: "volta::health", "UI thread has not responded for {late} ms (frozen, or the app was suspended)");
            } else if late < 2000 && STALLED.swap(false, Ordering::Relaxed) {
                log::info!(target: "volta::health", "UI thread responsive again");
            }
            if ticks.is_multiple_of(SAMPLE_EVERY) {
                let wall = last_at.elapsed().as_secs_f64();
                let cpu = process_cpu_seconds();
                let percent = match (cpu, last_cpu) {
                    (Some(now), Some(before)) if wall > 0.0 => Some((now - before) / wall * 100.0),
                    _ => None,
                };
                last_cpu = cpu;
                last_at = Instant::now();
                let renders = RENDERS.swap(0, Ordering::Relaxed);
                let nanos = RENDER_NANOS.swap(0, Ordering::Relaxed);
                let max = RENDER_MAX_NANOS.swap(0, Ordering::Relaxed);
                let slow = SLOW_RENDERS.swap(0, Ordering::Relaxed);
                let average = if renders > 0 { nanos as f64 / renders as f64 / 1e6 } else { 0.0 };
                let loads = COVER_LOADS.swap(0, Ordering::Relaxed);
                let cover_nanos = COVER_NANOS.swap(0, Ordering::Relaxed);
                let requests = volta_services::metrics::take_request_stats();
                if loads > 0 || requests.count > 0 {
                    log::info!(
                        target: PERF,
                        "io: covers loaded={loads} (avg {:.0}ms, {} from network, {} failed) | server requests={} avg={:.0}ms max={:.0}ms slow={} failed={}",
                        if loads > 0 { cover_nanos as f64 / loads as f64 / 1e6 } else { 0.0 },
                        COVER_FROM_NETWORK.swap(0, Ordering::Relaxed),
                        COVER_FAILED.swap(0, Ordering::Relaxed),
                        requests.count,
                        requests.average_ms,
                        requests.max_ms,
                        requests.slow,
                        requests.failed,
                    );
                }
                log::info!(
                    target: PERF,
                    "process: uptime={}s memory={} cpu={} threads={} | ui builds={} avg={:.2}ms max={:.1}ms slow(>33ms)={}",
                    uptime().as_secs(),
                    crate::components::perf_overlay::resident_memory().map(megabytes).unwrap_or_else(|| "n/a".into()),
                    percent.map(|p| format!("{p:.1}%")).unwrap_or_else(|| "n/a".into()),
                    thread_count().map(|t| t.to_string()).unwrap_or_else(|| "n/a".into()),
                    renders,
                    average,
                    max as f64 / 1e6,
                    slow,
                );
            }
        }
    });
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1}MB", bytes as f64 / 1024.0 / 1024.0)
}

#[cfg(target_os = "linux")]
fn thread_count() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status").ok()?.lines().find_map(|l| l.strip_prefix("Threads:")).and_then(|v| v.trim().parse().ok())
}

#[cfg(not(target_os = "linux"))]
fn thread_count() -> Option<u64> {
    None
}

/// CPU time (user + system) this process has used, in seconds.
#[cfg(unix)]
fn process_cpu_seconds() -> Option<f64> {
    // SAFETY: `usage` is a valid out-pointer for getrusage.
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut usage) != 0 {
            return None;
        }
        usage
    };
    let seconds = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    Some(seconds(usage.ru_utime) + seconds(usage.ru_stime))
}

#[cfg(windows)]
fn process_cpu_seconds() -> Option<f64> {
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn GetProcessTimes(process: isize, creation: *mut FileTime, exit: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
    }
    let (mut creation, mut exit, mut kernel, mut user) = (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
    // SAFETY: all four out-pointers are valid FILETIME structs.
    let ok = unsafe { GetProcessTimes(GetCurrentProcess(), &mut creation, &mut exit, &mut kernel, &mut user) };
    if ok == 0 {
        return None;
    }
    let ticks = |t: FileTime| ((t.high as u64) << 32 | t.low as u64) as f64 / 1e7;
    Some(ticks(kernel) + ticks(user))
}

// ---- system report -------------------------------------------------------

fn command(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program).args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

pub fn os_description() -> String {
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED.get_or_init(os_description_uncached).clone()
}

fn os_description_uncached() -> String {
    #[cfg(target_os = "macos")]
    {
        let version = command("sw_vers", &["-productVersion"]).unwrap_or_default();
        let build = command("sw_vers", &["-buildVersion"]).unwrap_or_default();
        format!("macOS {version} ({build})")
    }
    #[cfg(target_os = "linux")]
    {
        let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        let pretty = release.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).map(|v| v.trim_matches('"').to_string());
        let kernel = command("uname", &["-r"]).unwrap_or_default();
        let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        format!("{} (kernel {kernel}, session {session}, desktop {desktop})", pretty.unwrap_or_else(|| "Linux".into()))
    }
    #[cfg(target_os = "windows")]
    {
        command("cmd", &["/C", "ver"]).unwrap_or_else(|| "Windows".into())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        std::env::consts::OS.to_string()
    }
}

pub fn hardware_description() -> String {
    static CACHED: OnceLock<String> = OnceLock::new();
    CACHED.get_or_init(hardware_description_uncached).clone()
}

fn hardware_description_uncached() -> String {
    #[cfg(target_os = "macos")]
    {
        let cpu = command("sysctl", &["-n", "machdep.cpu.brand_string"]).unwrap_or_else(|| "unknown CPU".into());
        let memory = command("sysctl", &["-n", "hw.memsize"]).and_then(|v| v.parse::<u64>().ok()).map(|b| format!("{} GB", b / 1024 / 1024 / 1024)).unwrap_or_default();
        format!("{cpu}, {memory} RAM")
    }
    #[cfg(target_os = "linux")]
    {
        let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
        let cpu = cpuinfo.lines().find_map(|l| l.strip_prefix("model name")).and_then(|l| l.split_once(':')).map(|(_, v)| v.trim().to_string()).unwrap_or_else(|| "unknown CPU".into());
        let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let memory = meminfo
            .lines()
            .find_map(|l| l.strip_prefix("MemTotal:"))
            .and_then(|l| l.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
            .map(|kb| format!("{} GB", kb / 1024 / 1024))
            .unwrap_or_default();
        format!("{cpu}, {memory} RAM")
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "unknown CPU".into())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        String::new()
    }
}

/// Writes `system.txt` and the first lines of `volta.log`: everything a bug
/// report needs that isn't a log line.
pub fn write_system_report() {
    let Some(dir) = latest_dir() else { return };
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let data_dir = volta_services::storage::data_dir().map(|d| d.display().to_string()).unwrap_or_else(|| "(none)".into());
    let env: Vec<String> = std::env::vars().filter(|(k, _)| k.starts_with("VOLTA_") || k == "RUST_LOG").map(|(k, v)| format!("{k}={v}")).collect();
    let report = format!(
        "Volta system report\n\
         Written:   {}\n\
         Version:   {}\n\
         Channel:   {}\n\
         Commit:    {}\n\
         Built:     {} ({} build)\n\
         Target:    {}\n\
         Compiler:  {}\n\
         OS:        {}\n\
         Arch:      {}\n\
         Hardware:  {} ({} logical cores)\n\
         Data dir:  {}\n\
         Logs dir:  {}\n\
         Locale:    {}\n\
         Env:       {}\n",
        stamp(),
        crate::build_info::VERSION,
        crate::build_info::CHANNEL,
        crate::build_info::commit_label(),
        crate::build_info::built_label(),
        crate::build_info::PROFILE,
        crate::build_info::TARGET,
        crate::build_info::RUSTC,
        os_description(),
        std::env::consts::ARCH,
        hardware_description(),
        cores,
        data_dir,
        dir.display(),
        std::env::var("LANG").or_else(|_| std::env::var("LC_ALL")).unwrap_or_else(|_| "unknown".into()),
        if env.is_empty() { "(none)".into() } else { env.join(" ") },
    );
    let _ = fs::write(dir.join("system.txt"), &report);
    log::info!("{}", crate::build_info::summary());
    for line in report.lines().skip(1) {
        log::info!(target: "volta::system", "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(root: &Path, name: &str, started: &str) {
        fs::create_dir_all(root.join(name)).unwrap();
        fs::write(root.join(name).join(".started"), started).unwrap();
        fs::write(root.join(name).join("volta.log"), name).unwrap();
    }

    fn names(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(root).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        names.sort();
        names
    }

    #[test]
    fn keeps_latest_plus_two_older_sessions() {
        let root = std::env::temp_dir().join(format!("volta-logs-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        session(&root, "session-2026-11-10_08-00-00", "x");
        session(&root, "session-2026-11-11_08-00-00", "x");
        session(&root, "latest", "2026-11-12_08-00-00");

        rotate(&root, KEEP_PREVIOUS).unwrap();
        // `latest` became a dated session; the oldest was deleted; no `latest` until start() makes one.
        assert_eq!(names(&root), vec!["session-2026-11-11_08-00-00", "session-2026-11-12_08-00-00"]);

        // Rotating again with a fresh latest keeps only the two newest.
        session(&root, "latest", "2026-11-13_08-00-00");
        rotate(&root, KEEP_PREVIOUS).unwrap();
        assert_eq!(names(&root), vec!["session-2026-11-12_08-00-00", "session-2026-11-13_08-00-00"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn two_sessions_with_the_same_start_do_not_collide() {
        let root = std::env::temp_dir().join(format!("volta-logs-collide-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        session(&root, "latest", "2026-11-12_08-00-00");
        rotate(&root, KEEP_PREVIOUS).unwrap();
        session(&root, "latest", "2026-11-12_08-00-00");
        rotate(&root, KEEP_PREVIOUS).unwrap();
        assert_eq!(names(&root).len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dependencies_are_quieter_than_volta() {
        assert!(file_enabled("volta_audio::engine", log::Level::Debug));
        assert!(file_enabled(PERF, log::Level::Info));
        assert!(!file_enabled("hyper", log::Level::Info));
        assert!(file_enabled("hyper", log::Level::Warn));
    }
}
