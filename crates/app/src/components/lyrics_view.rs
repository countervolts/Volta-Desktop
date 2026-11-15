//! `LyricsView` from player-ui.tsx.
//!
//! Synced lines (25px bold, muted; the current line and hovered lines in the
//! text colour), auto-scroll that glides the current line to the middle over
//! 350 ms (ease-in-out), click to seek, and word-level fill. Unsynced lyrics
//! are plain 19px paragraphs.
//!
//! While playing, every line but the current and the first is blurred
//! (`filter: blur(var(--lyric-blur))`, the patched gpui's `filter_blur`),
//! unless the "lyrics blur" setting is off; a hovered line comes into focus.
//!
//! Approximations: the web sweeps a soft gradient through
//! each word with `background-clip: text`; here each word takes a colour
//! between the unsung (45 %) and sung (100 %) white by its own progress. The
//! 5 % scale of the current line is approximated with the font size.

use std::time::Instant;

use gpui::{
    Context, Entity, FontWeight, IntoElement, ParentElement, Render, ScrollHandle, SharedString,
    Styled, Task, Window, div, point, prelude::*, px,
};
use volta_logic::lyrics::{Lyrics, LyricsWord};

use crate::app_state::AppState;
use crate::components::motion::spinner;
use crate::components::ui::icon_with;
use crate::player::Player;
use crate::theme::{self, sz};

/// `LYRIC_LINE_LEAD_SECONDS`: a line is current slightly before its stamp.
const LINE_LEAD: f64 = 0.25;
/// `LYRIC_SCROLL_MS`.
const SCROLL_MS: f32 = 350.0;
/// `LYRIC_FILL_SPREAD` (percent of the word box).
const FILL_SPREAD: f32 = 20.0;
/// `--lyric-blur`.
const LYRIC_BLUR: f32 = 2.0;
/// `.lyric-line { transition: filter 0.25s linear }`.
const BLUR_TRANSITION_MS: f32 = 250.0;

pub struct LyricsView {
    state: Entity<AppState>,
    player: Entity<Player>,
    song_id: Option<String>,
    lyrics: Option<Lyrics>,
    pending: bool,
    error: bool,
    scroll: ScrollHandle,
    current: Option<usize>,
    glide: Option<(f32, f32, Instant)>,
    /// The line that was current before the last change, and when it changed:
    /// it blurs in while the new current line comes into focus.
    blur_change: Option<(Option<usize>, Instant)>,
    /// The full player's large, faded variant (`.embedded .lyrics-lines`).
    embedded: bool,
    /// `.lyric-line { transition: filter 0.25s linear }` for the whole list
    /// (pausing or resuming) and per hovered line.
    blur_on: crate::components::motion::Transition,
    /// The lyric clock (`LYRIC_CLOCK_MAX_LEAD`): the last position the player
    /// reported and when, advanced by wall time between reports.
    clock: (f64, Instant),
    hover_fx: std::collections::HashMap<usize, crate::components::motion::Transition>,
    /// Lyrics pasted for a song (`importedLyrics`), by song id.
    imported: Option<(String, Lyrics)>,
    /// `lyricsImportStatus`, shown for 2.6 s.
    pub import_status: Option<(String, Instant)>,
    _fetch: Option<Task<()>>,
}

impl LyricsView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let player = state.read(cx).player.clone();
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            player,
            song_id: None,
            lyrics: None,
            pending: false,
            error: false,
            scroll: ScrollHandle::new(),
            current: None,
            glide: None,
            blur_change: None,
            embedded: false,
            blur_on: crate::components::motion::Transition::new(250).with_curve(0.0, 0.0, 1.0, 1.0),
            clock: (0.0, Instant::now()),
            hover_fx: Default::default(),
            imported: None,
            import_status: None,
            _fetch: None,
        }
    }

    pub fn embedded(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self { embedded: true, ..Self::new(state, cx) }
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        self.lyrics = None;
        self.error = false;
        self.current = None;
        self.glide = None;
        self.scroll.set_offset(point(px(0.0), px(0.0)));
        let song = self.player.read(cx).current_song().cloned();
        let client = self.state.read(cx).client_handle();
        if let Some((_, lyrics)) = self.imported.as_ref().filter(|(id, _)| song.as_ref().is_some_and(|s| &s.id == id)) {
            self.lyrics = Some(lyrics.clone());
            self.pending = false;
            return;
        }
        // Local tracks: a `.lrc` file next to the audio wins.
        if let Some(lyrics) = song
            .as_ref()
            .filter(|s| s.is_local())
            .and_then(|s| s.local_file_path.as_deref().or(s.local_path.as_deref()))
            .and_then(volta_services::local::sidecar_lyrics)
        {
            self.lyrics = Some(lyrics);
            self.pending = false;
            self._fetch = None;
            return;
        }
        let (Some(song), Some(client)) = (song, client) else {
            self.pending = false;
            self._fetch = None;
            return;
        };
        let external = self.state.read(cx).storage.get(volta_logic::storage::EXTERNAL_LYRICS_KEY) == "true";
        // Local files have no server to ask: a sidecar .lrc, clipboard import, or LRCLIB
        // when external lookup is on.
        let local = song.is_local();
        if local && !external {
            self.pending = false;
            return;
        }
        self.pending = true;
        // Server lyrics first; with external lookup on, LRCLIB replaces
        // unsynced or missing lyrics (`LyricsView` fetch chain).
        let task = cx.background_spawn(async move {
            let value = if local { None } else { client.lyrics(&song)? };
            if !external || value.as_ref().is_some_and(|l| l.synced) {
                return Ok(value);
            }
            let found = volta_services::lrclib::lrclib_lyrics(&song);
            Ok::<_, volta_services::NavidromeError>(match found {
                Some(lyrics) if lyrics.synced || value.as_ref().is_none_or(|v| v.lines.is_empty()) => Some(lyrics),
                _ => value,
            })
        });
        self._fetch = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |view, cx| {
                view.pending = false;
                match result {
                    Ok(lyrics) => view.lyrics = lyrics,
                    Err(_) => view.error = true,
                }
                cx.notify();
            });
        }));
    }

    /// `importLyrics`: parse the clipboard as LRC, TTML or plain text.
    pub fn import_from_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(song_id) = self.player.read(cx).current_song().map(|s| s.id.clone()) else { return };
        let text = cx.read_from_clipboard().and_then(|item| item.text());
        let status = match text {
            None => "Clipboard access is unavailable here.".to_string(),
            Some(text) => match volta_logic::lyrics::parse_lyrics_text(&text) {
                None => "No readable lyrics found in the clipboard.".into(),
                Some(lyrics) => {
                    let status = if lyrics.synced { "Imported synced lyrics" } else { "Imported plain lyrics" };
                    self.imported = Some((song_id, lyrics.clone()));
                    self.lyrics = Some(lyrics);
                    self.error = false;
                    self.pending = false;
                    self.current = None;
                    status.into()
                }
            },
        };
        self.import_status = Some((status, Instant::now()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(2600)).await;
            let _ = this.update(cx, |view, cx| {
                if view.import_status.as_ref().is_some_and(|(_, at)| at.elapsed().as_millis() >= 2500) {
                    view.import_status = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn current_line(lyrics: &Lyrics, time: f64) -> Option<usize> {
        if !lyrics.synced {
            return None;
        }
        lyrics
            .lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.start.is_some_and(|start| start <= time + LINE_LEAD))
            .map(|(index, _)| index)
            .next_back()
    }

    /// Starts gliding `index` to the vertical middle of the list.
    fn glide_to(&mut self, index: usize) {
        let (Some(line), root) = (self.scroll.bounds_for_item(index), self.scroll.bounds()) else { return };
        let offset = self.scroll.offset().y;
        // Child bounds are laid out before the list's scroll offset applies,
        // so they are already in content coordinates.
        let content_top = line.top() - root.top();
        let target = -(content_top - (root.size.height - line.size.height) / 2.0);
        let max = self.scroll.max_offset().y;
        let target = target.min(px(0.0)).max(-max);
        if crate::components::motion::reduced() {
            self.scroll.set_offset(point(px(0.0), target));
            return;
        }
        self.glide = Some((f32::from(offset), f32::from(target), Instant::now()));
    }

    fn step_glide(&mut self, window: &mut Window) {
        let Some((from, to, started)) = self.glide else { return };
        let t = (started.elapsed().as_secs_f32() * 1000.0 / SCROLL_MS).min(1.0);
        let eased = if t < 0.5 { 2.0 * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(2) / 2.0 };
        self.scroll.set_offset(point(px(0.0), px(from + (to - from) * eased)));
        if t < 1.0 {
            window.request_animation_frame();
        } else {
            self.glide = None;
        }
    }
}

/// `lyricTimestamp`: `[mm:ss.cc]`.
fn lyric_timestamp(seconds: f64) -> String {
    let total = volta_logic::js::js_round(seconds.max(0.0) * 100.0) as u64;
    format!("[{:02}:{:02}.{:02}]", total / 6000, (total % 6000) / 100, total % 100)
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn ramp(from: f64, to: f64, time: f64) -> f32 {
    (((time - from) / (to - from).max(0.001)).clamp(0.0, 1.0)) as f32
}

/// Alpha of a glyph centred at `at` (percent of its word box) under the fill
/// front `progress` (`--lyric-progress`): lit before it, unsung 20 % after.
fn fill_alpha(at: f32, progress: f32, base: f32, active: f32) -> f32 {
    let lit = ((progress + FILL_SPREAD - at) / FILL_SPREAD).clamp(0.0, 1.0);
    mix(base, active, lit)
}

/// `LYRIC_EMPHASIS_MIN_SECONDS` / `LYRIC_EMPHASIS_MAX_LETTERS`.
fn is_emphasis(word: &LyricsWord) -> bool {
    let letters = word.text.chars().filter(|c| *c != '(' && *c != ')').count();
    word.end - word.start >= 1.0 && (1..=7).contains(&letters)
}

/// The current line's timed words (`paintLyrics`): the fill sweeps through
/// each word letter by letter, a sung word settles 2px high, and a held word
/// pops letter by letter with a glow. Words are separate boxes so each can
/// lift; the line wraps between them.
fn current_words(words: &[LyricsWord], time: f64, base: f32, active: f32, font: f32, right: bool) -> gpui::AnyElement {
    let reduced = crate::components::motion::reduced();
    let space = font * 0.26;
    let white = |alpha: f32| gpui::white().opacity(alpha);
    // Syllables of one word (`real`+`ly`, no space between) stay on one
    // line, like the web's adjacent inline spans: one box per spaced word.
    let mut groups: Vec<Vec<&LyricsWord>> = vec![Vec::new()];
    for word in words {
        groups.last_mut().expect("non-empty").push(word);
        if word.space {
            groups.push(Vec::new());
        }
    }
    groups.retain(|g| !g.is_empty());
    let render_word = |word: &LyricsWord| -> gpui::AnyElement {
            let duration = (word.end - word.start).max(0.0);
            let chars: Vec<char> = word.text.chars().collect();
            let n = chars.len().max(1) as f32;
            let body: gpui::AnyElement = if is_emphasis(word) {
                let slice = duration / chars.len().max(1) as f64;
                div()
                    .flex()
                    .children(chars.iter().enumerate().map(|(i, ch)| {
                        let from = word.start + slice * i as f64;
                        let rise = ramp(from, from + 0.5, time);
                        let settle = ramp(from + 0.5, from + 1.0, time);
                        let rising = time < from + 0.5;
                        let progress = if rising { mix(-FILL_SPREAD, 90.0, rise) } else { mix(90.0, 100.0, settle) };
                        let lift = if rising { mix(0.0, -2.05, rise) } else { mix(-2.05, -2.0, settle) };
                        let (glow, glow_alpha) = if reduced {
                            (0.0, 0.0)
                        } else if rising {
                            (mix(0.0, 10.0, rise), mix(0.0, 0.4, rise))
                        } else {
                            (mix(10.0, 4.0, settle), mix(0.4, 0.0, settle))
                        };
                        div()
                            .relative()
                            .top(sz(lift))
                            .text_color(white(fill_alpha(50.0, progress, base, active)))
                            .when(glow_alpha > 0.0, |d| {
                                d.text_shadow(gpui::TextShadow {
                                    offset_x: px(0.0).into(),
                                    offset_y: px(0.0).into(),
                                    blur: sz(glow).into(),
                                    color: white(glow_alpha),
                                })
                            })
                            .child(ch.to_string())
                    }))
                    .into_any_element()
            } else {
                let progress = mix(-FILL_SPREAD, 100.0, ramp(word.start, word.start + duration, time));
                let mut highlights = Vec::new();
                for (i, (at, ch)) in word.text.char_indices().enumerate() {
                    let centre = (i as f32 + 0.5) / n * 100.0;
                    // gpui blends a highlight colour over the base colour
                    // (translucent white over white stays white), so the
                    // base is pure white and each glyph fades out instead.
                    let alpha = fill_alpha(centre, progress, base, active);
                    let style = gpui::HighlightStyle { fade_out: Some(1.0 - alpha), ..Default::default() };
                    highlights.push((at..at + ch.len_utf8(), style));
                }
                let lift = mix(0.0, -2.0, ramp(word.start + 0.1, word.start + 0.1 + duration, time));
                div()
                    .relative()
                    .top(sz(lift))
                    .text_color(gpui::white())
                    .child(gpui::StyledText::new(word.text.clone()).with_highlights(highlights))
                    .into_any_element()
            };
            body
    };
    div()
        .flex()
        .flex_wrap()
        .when(right, |d| d.justify_end())
        .children(groups.into_iter().map(|group| {
            let spaced = group.last().is_some_and(|w| w.space);
            div().flex_none().flex().when(spaced, |d| d.mr(sz(space))).children(group.into_iter().map(render_word))
        }))
        .into_any_element()
}

impl Render for LyricsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::tokens(cx).clone();
        let (song_id, raw_time, playing) = {
            let p = self.player.read(cx);
            (p.current_song().map(|s| s.id.clone()), p.position, p.playing)
        };
        // The player reports on its own timer; between reports the clock runs
        // on wall time (at most 0.25 s ahead) so fills move every frame.
        if raw_time != self.clock.0 {
            self.clock = (raw_time, Instant::now());
        }
        let time = if playing { raw_time + self.clock.1.elapsed().as_secs_f64().min(0.25) } else { raw_time };
        if song_id != self.song_id {
            self.song_id = song_id.clone();
            self.fetch(cx);
        }

        let empty = |icon: Option<gpui::AnyElement>, title: Option<&str>, body: gpui::AnyElement| {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .p(sz(35.0))
                .text_center()
                .text_color(tokens.muted)
                .children(icon.map(|i| div().mb(sz(15.0)).child(i)))
                .when_some(title, |d, t| {
                    d.child(
                        div()
                            .mb(sz(8.0))
                            .text_size(sz(15.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(tokens.secondary)
                            .child(t.to_string()),
                    )
                })
                .child(body)
                .into_any_element()
        };
        let paragraph = |text: &str| {
            div().text_size(sz(12.0)).line_height(gpui::relative(1.5)).child(text.to_string()).into_any_element()
        };

        if self.pending {
            return empty(Some(spinner(24.0).into_any_element()), None, paragraph("Loading lyrics…"));
        }
        if self.error {
            let view = cx.entity();
            return empty(
                None,
                None,
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(sz(12.0))
                    .child(paragraph("Could not load lyrics."))
                    .child(
                        crate::views::library::secondary_button("lyrics-retry", "Try Again", "", cx)
                            .on_click(move |_, _, cx| view.update(cx, |v, cx| v.fetch(cx))),
                    )
                    .into_any_element(),
            );
        }
        let Some(lyrics) = self.lyrics.clone().filter(|l| !l.lines.is_empty()) else {
            let playing_song = song_id.is_some();
            let local_song = self.player.read(cx).current_song().is_some_and(|s| s.is_local());
            return empty(
                Some(icon_with("message-square-text", 36.0, Some(1.4), false).into_any_element()),
                Some(if playing_song { "No lyrics available" } else { "Nothing playing" }),
                paragraph(if playing_song && local_song {
                    "Import lyrics from the clipboard for local files."
                } else if playing_song {
                    "This song has no lyrics on your server."
                } else {
                    "Play a song to see its lyrics."
                }),
            );
        };

        let current = Self::current_line(&lyrics, time);
        // A line of timed words animates every display frame while playing.
        if playing && current.and_then(|i| lyrics.lines.get(i)).is_some_and(|l| l.words.as_ref().is_some_and(|w| !w.is_empty())) {
            window.request_animation_frame();
        }
        if current != self.current {
            if !crate::components::motion::reduced() {
                self.blur_change = Some((self.current, Instant::now()));
            }
            self.current = current;
            if let Some(index) = current {
                self.glide_to(index);
            }
        }
        self.step_glide(window);
        // `.lyrics-lines.is-playing:not(.no-lyric-blur)`.
        let blur_lines = playing && self.state.read(cx).storage.get(volta_logic::storage::LYRICS_BLUR_KEY) != "false";
        // Pausing or resuming fades the blur rather than switching it.
        self.blur_on.set(blur_lines);
        let (blur_amount, blur_moving) = self.blur_on.value();
        let mut hover_moving = false;
        self.hover_fx.retain(|_, fx| {
            let (value, moving) = fx.value();
            hover_moving |= moving;
            value > 0.0 || moving
        });
        if blur_moving || hover_moving {
            window.request_animation_frame();
        }
        let hover_clear: std::collections::HashMap<usize, f32> = self.hover_fx.iter().map(|(i, fx)| (*i, fx.value().0)).collect();
        // The 0.25 s filter transition of the lines whose state just changed.
        let change_progress = match self.blur_change {
            Some((_, started)) => {
                let t = (started.elapsed().as_secs_f32() * 1000.0 / BLUR_TRANSITION_MS).min(1.0);
                if t < 1.0 {
                    window.request_animation_frame();
                } else {
                    self.blur_change = None;
                }
                t
            }
            None => 1.0,
        };
        let previous = self.blur_change.and_then(|(previous, _)| previous);
        // The 0.1 s ease-in-out colour and scale change of the line change.
        let change_focus = match self.blur_change {
            Some((_, started)) => {
                let t = (started.elapsed().as_secs_f32() * 10.0).min(1.0);
                if t < 0.5 { 2.0 * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(2) / 2.0 }
            }
            None => 1.0,
        };
        let line_blur = |index: usize| -> f32 {
            if blur_amount <= 0.0 || index == 0 {
                return 0.0;
            }
            // A hovered line comes into focus over the same 0.25 s.
            let scale = blur_amount * (1.0 - hover_clear.get(&index).copied().unwrap_or(0.0));
            let target = if Some(index) == current { 0.0 } else { 1.0 };
            let from = if Some(index) == previous { 0.0 } else if Some(index) == current && previous.is_some() { 1.0 } else { target };
            scale * LYRIC_BLUR * (from + (target - from) * change_progress)
        };

        let viewport = theme::viewport_css_width(window);
        let list_width = if self.embedded {
            f32::from(self.scroll.bounds().size.width) / (f32::from(window.rem_size()) / 16.0)
        } else if viewport <= 860.0 {
            280.0
        } else {
            crate::components::inspector::inspector_width(viewport)
        };
        let edge = list_width * 0.06;
        // `.embedded .lyrics-lines button { font-size: clamp(34px, 3vw, 52px); padding: 18px 0 }`.
        let (font, pad_y) = if self.embedded { ((viewport * 0.03).clamp(34.0, 52.0), 18.0) } else { (25.0, 15.0) };
        // `mask-image: linear-gradient(transparent, #000 8%, #000 83%, transparent)`.
        let root = self.scroll.bounds();
        let mask = |index: usize| -> f32 {
            let Some(line) = self.scroll.bounds_for_item(index) else { return 1.0 };
            let height = f32::from(root.size.height).max(1.0);
            // Child bounds exclude the scroll offset; add it for the on-screen y.
            let y = (f32::from(line.center().y + self.scroll.offset().y - root.top()) / height).clamp(0.0, 1.0);
            if y < 0.08 { y / 0.08 } else if y > 0.83 { (1.0 - y) / 0.17 } else { 1.0 }
        };
        let (text_color, muted_color, secondary_color) = if self.embedded {
            (gpui::white(), theme::hex("#ffffff5c"), theme::hex("#ffffffac"))
        } else {
            (tokens.text, tokens.muted, tokens.secondary)
        };

        let mut list = div()
            .id("lyrics-lines")
            .track_scroll(&self.scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .map(|d| {
                if self.embedded {
                    d.pt(sz(90.0)).pr(sz(edge)).pb(sz(150.0))
                } else {
                    d.pt(sz(30.0)).pl(sz(23.0)).pr(sz(23.0 + edge)).pb(sz(140.0))
                }
            });

        for (index, line) in lyrics.lines.iter().enumerate() {
            if !(lyrics.synced && line.start.is_some()) {
                list = list.child(
                    div()
                        .py(sz(10.0))
                        .text_size(sz(19.0))
                        .font_weight(FontWeight::MEDIUM)
                        .letter_spacing(sz(-0.65))
                        .line_height(gpui::relative(1.25))
                        .text_color(secondary_color)
                        .opacity(if self.embedded { mask(index) } else { 1.0 })
                        .child(line.text.clone()),
                );
                continue;
            }
            let is_current = Some(index) == current;
            let start = line.start.unwrap_or(0.0);
            let player = self.player.clone();
            let right = line.align == Some(volta_logic::lyrics::Align::Right);
            // `.lyric-line { transition: transform 0.1s ease-in-out }` and the
            // row's `color 0.1s`: the line entering and the one leaving.
            let focus = if is_current {
                change_focus
            } else if Some(index) == previous {
                1.0 - change_focus
            } else {
                0.0
            };
            let line_font = font * (1.0 + 0.05 * focus);
            let line_color = theme::mix(text_color, focus * 100.0, muted_color);
            let text: gpui::AnyElement = match line.words.as_ref().filter(|w| !w.is_empty() && is_current) {
                Some(words) => current_words(words, time, 0.45, 1.0, line_font, right),
                None => {
                    let content = if line.text.is_empty() { "♪".to_string() } else { line.text.clone() };
                    div().child(content).into_any_element()
                }
            };
            // `.lyric-line { filter: blur(var(--lyric-blur)) }`; the hovered line
            // is sharp (`button:hover … .lyric-line { filter: none }`).
            let group = SharedString::from(format!("lyric-row-{index}"));
            let blur = line_blur(index);
            let text = div().when(blur > 0.0, |d| d.filter_blur(sz(blur))).child(text);
            list = list.child(
                div()
                    .id(SharedString::from(format!("lyric-{index}")))
                    .group(group)
                    .w_full()
                    .py(sz(pad_y))
                    // `--lyric-current-scale: 1.05` approximated with the font size.
                    .text_size(sz(line_font))
                    .when(self.embedded, |d| d.opacity(mask(index)))
                    .font_weight(FontWeight::BOLD)
                    // `letter-spacing: -0.65px`; `-0.8px` in the full player.
                    .letter_spacing(sz(if self.embedded { -0.8 } else { -0.65 }))
                    .line_height(gpui::relative(1.25))
                    .cursor_pointer()
                    .when(right, |d| d.text_right())
                    .text_color(line_color)
                    .hover(move |s| s.text_color(text_color))
                    .on_click(move |_, _, cx| player.update(cx, |p, cx| p.seek(start, cx)))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.hover_fx
                            .entry(index)
                            .or_insert_with(|| crate::components::motion::Transition::new(250).with_curve(0.0, 0.0, 1.0, 1.0))
                            .set(*hovered);
                        cx.notify();
                    }))
                    // `lyricContextItems`: copy the line, with or without its time.
                    .on_mouse_down(gpui::MouseButton::Right, {
                        let state = self.state.clone();
                        let line_text = if line.text.is_empty() { "♪".to_string() } else { line.text.clone() };
                        move |event, _, cx| {
                            cx.stop_propagation();
                            let (plain, timed) = (line_text.clone(), format!("{} {}", lyric_timestamp(start), line_text));
                            let items = vec![
                                crate::components::menu::MenuItem::new("copy", "Copy lyric", move |_, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(plain.clone()))
                                }),
                                crate::components::menu::MenuItem::new("clock3", "Copy lyric with timestamp", move |_, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(timed.clone()))
                                }),
                            ];
                            crate::components::menu::open(&state, event.position, gpui::Anchor::TopLeft, items, cx);
                        }
                    })
                    .child(text)
                    // `.lyric-backing`: only under the line being sung.
                    .when_some(line.backing.clone().filter(|b| is_current && !b.is_empty()), |d, backing| {
                        d.child(
                            div()
                                .mt(sz(20.0))
                                .text_size(sz(font * 0.56))
                                .font_weight(FontWeight::SEMIBOLD)
                                .letter_spacing(sz(-0.3))
                                .child(current_words(&backing, time, 0.175, 0.35, font * 0.56, right)),
                        )
                    })
                    .when_some(line.translation.clone(), |d, t| {
                        d.child(
                            div()
                                .mt(sz(6.0))
                                .text_size(sz(font * 0.7))
                                .font_weight(FontWeight::MEDIUM)
                                .letter_spacing(sz(-0.2))
                                .text_color(secondary_color)
                                .opacity(0.45)
                                .child(t),
                        )
                    }),
            );
        }
        list.into_any_element()
    }
}
