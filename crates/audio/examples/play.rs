//! Smoke test: `cargo run -p volta-audio --example play -- <file-or-url> [seek-seconds] [next-file]`
use std::time::{Duration, Instant};
use volta_audio::{AudioHandle, Command, LoadRequest, TrackSource};

fn source(arg: &str) -> TrackSource {
    if arg.starts_with("http") { TrackSource::Url(arg.into()) } else { TrackSource::File(arg.into()) }
}

fn main() {
    env_logger::init();
    let args: Vec<String> = std::env::args().collect();
    let audio = AudioHandle::spawn();
    let ext = |a: &str| a.rsplit('.').next().map(str::to_string);
    audio.send(Command::SetVolume(0.2));
    audio.send(Command::Load(LoadRequest { token: 1, source: source(&args[1]), fallback: None, extension: ext(&args[1]), start_at: 0.0, autoplay: true, gain: 1.0 }));
    if let Some(next) = args.get(3) {
        audio.send(Command::Preload(LoadRequest { token: 2, source: source(next), fallback: None, extension: ext(next), start_at: 0.0, autoplay: true, gain: 1.0 }));
    }
    let seek: Option<f64> = args.get(2).and_then(|s| s.parse().ok());
    let started = Instant::now();
    let mut seeked = false;
    while started.elapsed() < Duration::from_secs(8) {
        while let Some(event) = audio.try_event() { println!("event {event:?}"); }
        if !seeked && started.elapsed() > Duration::from_secs(2) {
            if let Some(t) = seek { audio.send(Command::Seek(t)); println!("seek -> {t}"); }
            seeked = true;
        }
        println!("t={:.2}s pos={:.3} token={} playing={} underruns={}", started.elapsed().as_secs_f64(), audio.position(), audio.active_token(), audio.is_playing(), audio.underruns());
        std::thread::sleep(Duration::from_millis(500));
    }
}
