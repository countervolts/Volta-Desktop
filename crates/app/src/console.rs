//! Log capture for the developer console window.
//!
//! The same `env_logger` output that goes to the terminal is also kept in a
//! bounded in-memory buffer, so the console works identically on every OS
//! (a release build on Windows or a Finder-launched macOS app has no terminal).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;

/// Lines kept; the oldest are dropped first.
const MAX_LINES: usize = 1000;

#[derive(Clone)]
pub struct Line {
    pub level: log::Level,
    pub text: String,
}

static LINES: Mutex<VecDeque<Line>> = Mutex::new(VecDeque::new());
/// Bumped on every change so the window only redraws when something happened.
static REVISION: AtomicU64 = AtomicU64::new(0);

struct Tee(env_logger::Logger);

impl log::Log for Tee {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.0.enabled(metadata) || crate::logs::file_enabled(metadata.target(), metadata.level())
    }

    fn log(&self, record: &log::Record) {
        // Every record that passes the file filter goes to disk, whatever the terminal shows.
        if crate::logs::file_enabled(record.target(), record.level()) {
            crate::logs::write(record);
        }
        // Performance lines live in performance.log only.
        if record.target() == crate::logs::PERF {
            return;
        }
        if self.0.matches(record) {
            push(Line {
                level: record.level(),
                text: format!("{} {:<5} {}  {}", chrono::Local::now().format("%H:%M:%S"), record.level(), record.target(), record.args()),
            });
        }
        self.0.log(record);
    }

    fn flush(&self) {
        self.0.flush();
        crate::logs::flush();
    }
}

/// Installs the logger: terminal output as before, plus the capture buffer.
pub fn init_logger() {
    let inner = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,volta=info,volta_audio=info")).build();
    // The files want debug detail from Volta even though the terminal doesn't.
    let max = inner.filter().max(log::LevelFilter::Debug);
    crate::logs::start();
    if log::set_boxed_logger(Box::new(Tee(inner))).is_ok() {
        log::set_max_level(max);
        crate::logs::write_system_report();
    }
}

fn push(line: Line) {
    let mut lines = LINES.lock();
    if lines.len() == MAX_LINES {
        lines.pop_front();
    }
    lines.push_back(line);
    REVISION.fetch_add(1, Ordering::Relaxed);
}

pub fn revision() -> u64 {
    REVISION.load(Ordering::Relaxed)
}

pub fn snapshot() -> Vec<Line> {
    LINES.lock().iter().cloned().collect()
}

pub fn clear() {
    LINES.lock().clear();
    REVISION.fetch_add(1, Ordering::Relaxed);
}
