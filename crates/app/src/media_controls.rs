//! OS media controls: the native counterpart of `navigator.mediaSession` in
//! use-player.ts. Metadata follows the current song, the playback state
//! follows play/pause, and the OS's play, pause, stop, previous, next and seek
//! requests drive the player.
//!
//! The web publishes the authenticated cover URL as artwork. Here the cover is
//! written to a local file instead, so no Subsonic token reaches the OS media
//! service.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use gpui::{App, Entity, Window};
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection};

use crate::app_state::AppState;

pub struct MediaSession {
    controls: MediaControls,
    /// (song id, cover written) last published, and the playing flag.
    published: (String, bool),
    playing: Option<bool>,
    cover_file: Option<PathBuf>,
}

fn hwnd(window: &Window) -> Option<*mut std::ffi::c_void> {
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                return Some(h.hwnd.get() as *mut std::ffi::c_void);
            }
        }
        None
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = window;
        None
    }
}

/// Creates the controls and starts routing OS requests to the player.
pub fn start(state: &Entity<AppState>, window: &Window, cx: &mut App) {
    let config = PlatformConfig { display_name: "Volta", dbus_name: "volta", hwnd: hwnd(window) };
    let Ok(mut controls) = MediaControls::new(config) else {
        log::warn!("media controls are unavailable");
        return;
    };
    let (sender, receiver) = channel();
    if controls.attach(move |event| drop(sender.send(event))).is_err() {
        log::warn!("media controls could not attach");
        return;
    }
    state.update(cx, |s, _| s.media = Some(MediaSession { controls, published: Default::default(), playing: None, cover_file: None }));
    poll(state.clone(), receiver, cx);
}

fn poll(state: Entity<AppState>, receiver: Receiver<MediaControlEvent>, cx: &mut App) {
    let state = state.downgrade();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_millis(120)).await;
            let events: Vec<MediaControlEvent> = receiver.try_iter().collect();
            if events.is_empty() {
                continue;
            }
            let alive = state.update(cx, |s, cx| {
                let player = s.player.clone();
                player.update(cx, |p, cx| {
                    for event in events {
                        match event {
                            MediaControlEvent::Play => p.play(cx),
                            MediaControlEvent::Pause => p.pause(cx),
                            MediaControlEvent::Toggle => p.toggle(cx),
                            MediaControlEvent::Next => p.next(cx),
                            MediaControlEvent::Previous => p.previous(cx),
                            MediaControlEvent::Stop => p.stop(cx),
                            // `seekbackward` / `seekforward` default to 10s.
                            MediaControlEvent::Seek(direction) => {
                                let delta = if direction == SeekDirection::Forward { 10.0 } else { -10.0 };
                                let at = p.position + delta;
                                p.seek(at, cx);
                            }
                            MediaControlEvent::SeekBy(direction, by) => {
                                let delta = by.as_secs_f64() * if direction == SeekDirection::Forward { 1.0 } else { -1.0 };
                                let at = p.position + delta;
                                p.seek(at, cx);
                            }
                            MediaControlEvent::SetPosition(MediaPosition(at)) => p.seek(at.as_secs_f64(), cx),
                            _ => {}
                        }
                    }
                });
            });
            if alive.is_err() {
                break;
            }
        }
    })
    .detach();
}

impl AppState {
    /// Publishes the current song and playback state (`setMetadata`,
    /// `playbackState`, `setPositionState`) when they change.
    pub fn sync_media_session(&mut self, cx: &mut gpui::Context<Self>) {
        if self.media.is_none() {
            return;
        }
        let (song, playing, position, duration) = {
            let p = self.player.read(cx);
            (p.current_song().cloned(), p.playing, p.position, p.duration)
        };
        let cover = song.as_ref().and_then(|s| self.cover(s.cover_art.as_deref(), 512, cx));
        let Some(media) = self.media.as_mut() else { return };
        let key = (song.as_ref().map(|s| s.id.clone()).unwrap_or_default(), cover.is_some());
        if key != media.published {
            media.published = key;
            media.playing = None;
            match &song {
                None => {
                    let _ = media.controls.set_metadata(MediaMetadata::default());
                }
                Some(song) => {
                    if let Some(old) = media.cover_file.take() {
                        let _ = std::fs::remove_file(old);
                    }
                    media.cover_file = cover.as_ref().and_then(|image| write_cover(&song.id, image));
                    let url = media.cover_file.as_ref().map(|p| format!("file://{}", p.display()));
                    let _ = media.controls.set_metadata(MediaMetadata {
                        title: Some(&song.title),
                        artist: Some(song.artist.as_deref().unwrap_or("")),
                        album: Some(song.album.as_deref().unwrap_or("")),
                        cover_url: url.as_deref(),
                        duration: (duration > 0.0).then(|| Duration::from_secs_f64(duration)),
                    });
                }
            }
        }
        if media.playing != Some(playing) {
            media.playing = Some(playing);
            let progress = Some(MediaPosition(Duration::from_secs_f64(position.max(0.0))));
            let state = match (&song, playing) {
                (None, _) => MediaPlayback::Stopped,
                (Some(_), true) => MediaPlayback::Playing { progress },
                (Some(_), false) => MediaPlayback::Paused { progress },
            };
            let _ = media.controls.set_playback(state);
        }
    }
}

/// Writes the cover to `<data dir>/media/cover-<hash>.png`.
fn write_cover(song_id: &str, image: &gpui::RenderImage) -> Option<PathBuf> {
    let dir = volta_services::storage::data_dir()?.join("media");
    std::fs::create_dir_all(&dir).ok()?;
    let hash = song_id.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    let path = dir.join(format!("cover-{hash:08x}.png"));
    crate::covers::to_rgba(image)?.save_with_format(&path, image::ImageFormat::Png).ok()?;
    Some(path)
}
