//! `SettingsView` from app/settings-view.tsx: hero, tab bar, and grouped
//! rows per pane. Every value is stored under the web's key and format.

use std::rc::Rc;

use gpui::{AnyElement, App, BoxShadow, Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, Window, div, point, prelude::*, px};
use volta_logic::storage as keys;

use crate::app_state::{AppState, Dialog};
use crate::components::select::select;
use crate::components::ui::{icon, icon_with};
use crate::theme::{self, Tokens, mix, sz, transparent};
use crate::views::library::secondary_button;

pub const TABS: [(&str, &str, &str); 11] = [
    ("appearance", "Appearance", "palette"),
    ("playback", "Playback", "audio-lines"),
    ("performance", "Performance", "gauge"),
    ("privacy", "Privacy", "shield-check"),
    ("sharing", "Sharing", "share2"),
    ("keyboard", "Keyboard", "keyboard"),
    ("help", "Help", "life-buoy"),
    ("account", "Account", "circle-user-round"),
    ("about", "About", "info"),
    ("developer", "Developer", "flask-conical"),
    ("diagnostics", "Diagnostics", "gauge"),
];

/// Diagnostics are offered on the beta channel on the web; natively in debug
/// builds or with `VOLTA_DIAGNOSTICS=1`.
pub fn diagnostics_enabled() -> bool {
    cfg!(debug_assertions) || std::env::var("VOLTA_DIAGNOSTICS").is_ok_and(|v| v == "1")
}

fn tone(name: &str) -> gpui::Hsla {
    theme::hex(match name {
        "blue" => "#0a84ff",
        "green" => "#30d158",
        "indigo" => "#5e5ce6",
        "orange" => "#ff9f0a",
        "pink" => "#ff375f",
        "purple" => "#bf5af2",
        "teal" => "#40c8e0",
        _ => "#8e8e93",
    })
}

/// `.setting-icon`: a 28px tinted tile with a white 16px glyph.
fn setting_icon(tone_name: &str, glyph: &str) -> AnyElement {
    div()
        .size(sz(28.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(sz(7.0))
        .bg(tone(tone_name))
        .text_color(gpui::white())
        .child(icon(glyph, 16.0))
        .into_any_element()
}

/// One `.setting-row`: icon, title / subtitle, trailing control.
fn row(tokens: &Tokens, tone_name: &str, glyph: &str, title: impl Into<SharedString>, subtitle: impl Into<SharedString>, control: AnyElement) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(sz(15.0))
        .min_h(sz(54.0))
        .px(sz(12.0))
        .py(sz(10.0))
        .text_size(sz(12.0))
        .child(setting_icon(tone_name, glyph))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sz(3.0))
                .flex_1()
                .min_w_0()
                .child(div().font_weight(FontWeight::MEDIUM).child(title.into()))
                .child(div().text_size(sz(11.0)).line_height(gpui::relative(1.4)).text_color(tokens.muted).child(subtitle.into())),
        )
        .child(div().flex_none().child(control))
        .into_any_element()
}

/// `input[type=checkbox]`: 18px, 5px radius; accent fill with a white tick.
pub(crate) fn checkbox(id: &str, checked: bool, tokens: &Tokens, on_toggle: impl Fn(bool, &mut App) + 'static) -> AnyElement {
    let border = if checked { tokens.accent } else { mix(tokens.text, 32.0, transparent()) };
    let hover = mix(tokens.text, 52.0, transparent());
    div()
        .id(SharedString::from(format!("check-{id}")))
        .size(sz(18.0))
        .flex()
        .items_center()
        .justify_center()
        .border(sz(1.5))
        .border_color(border)
        .rounded(sz(5.0))
        .cursor_pointer()
        .when(checked, |d| d.bg(tokens.accent).text_color(gpui::white()).child(icon_with("check", 11.0, Some(3.0), false)))
        .when(!checked, move |d| d.hover(move |s| s.border_color(hover)))
        .on_click(move |_, _, cx| on_toggle(!checked, cx))
        .into_any_element()
}

/// `.setting-stepper`: − value +.
pub(crate) fn stepper(id: &str, value: String, tokens: &Tokens, on_step: impl Fn(i32, &mut App) + 'static) -> AnyElement {
    let on_step = Rc::new(on_step);
    let button = |delta: i32, label: &'static str| {
        let on_step = on_step.clone();
        div()
            .id(SharedString::from(format!("{id}-{delta}")))
            .size(sz(27.0))
            .flex()
            .items_center()
            .justify_center()
            .text_color(tokens.secondary)
            .text_size(sz(12.0))
            .cursor_pointer()
            .hover(|s| s.text_color(tokens.text).bg(tokens.canvas))
            .on_click(move |_, _, cx| on_step(delta, cx))
            .child(label)
    };
    div()
        .flex()
        .items_center()
        .overflow_hidden()
        .border(sz(1.0))
        .border_color(tokens.line)
        .rounded(sz(6.0))
        .bg(tokens.sidebar)
        .child(button(-1, "−"))
        .child(
            div()
                .w(sz(34.0))
                .h(sz(27.0))
                .flex()
                .items_center()
                .justify_center()
                .border_l(sz(1.0))
                .border_r(sz(1.0))
                .border_color(tokens.line)
                .text_size(sz(11.0))
                .text_color(tokens.text)
                .font_features(theme::fonts::tabular_nums())
                .child(value),
        )
        .child(button(1, "+"))
        .into_any_element()
}

/// `.settings-section`: heading, grouped rows, footer.
fn section(tokens: &Tokens, heading: &str, rows: Vec<AnyElement>, footer: &str) -> AnyElement {
    let count = rows.len();
    div()
        .flex()
        .flex_col()
        .gap(sz(12.0))
        .min_w_0()
        .child(div().text_size(sz(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(tokens.text).child(heading.to_string()))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .border(sz(1.0))
                .border_color(tokens.line)
                .rounded(sz(12.0))
                .bg(tokens.canvas)
                .children(rows.into_iter().enumerate().map(|(i, r)| {
                    div().when(i > 0, |d| d.border_t(sz(1.0)).border_color(tokens.line)).child(r).into_any_element()
                }))
                .when(count == 0, |d| d),
        )
        .child(
            div()
                .px(sz(2.0))
                .text_size(sz(11.0))
                .line_height(gpui::relative(1.45))
                .text_color(tokens.muted)
                .child(footer.to_string()),
        )
        .into_any_element()
}

fn pref(state: &Entity<AppState>, key: &str, cx: &App) -> String {
    state.read(cx).storage.get(key)
}

fn set_pref(state: &Entity<AppState>, key: &str, value: &str, cx: &mut App) {
    log::info!("setting changed: {key} = {value}");
    state.update(cx, |s, cx| {
        s.storage.set(key, value);
        if key == keys::FULLSCREEN_FPS_KEY {
            crate::components::fullscreen_background::set_fps(value);
        }
        if key == keys::TRANSITION_MODE_KEY || key == keys::CROSSFADE_KEY || key == keys::AUTOMIX_ALBUMS_KEY {
            s.sync_transition(cx);
        }
        // Turning tracking off empties the profile; turning it on reads it back.
        if key == keys::LISTENING_HISTORY_ENABLED_KEY {
            if value == "false" {
                s.listening_events.clear();
            } else {
                s.load_listening_history();
            }
            s.load_learning();
        }
        cx.notify();
    });
}

/// A `Select` that stores its value under `key`.
fn pref_select(
    state: &Entity<AppState>,
    id: &'static str,
    key: &'static str,
    value: &'static str,
    options: &'static [(&'static str, &'static str)],
    cx: &App,
) -> AnyElement {
    let target = state.clone();
    select(
        state,
        id,
        value,
        options,
        Rc::new(move |v, _, cx| set_pref(&target, key, v, cx)),
        cx,
    )
    .into_any_element()
}

fn as_static(options: &'static [(&'static str, &'static str)], value: &str, fallback: &'static str) -> &'static str {
    options.iter().find(|(v, _)| *v == value).map(|(v, _)| *v).unwrap_or(fallback)
}

fn link_button(id: &str, label: &str, glyph: &str, url: &'static str, cx: &App) -> AnyElement {
    secondary_button(id, label, glyph, cx).on_click(move |_, _, cx| cx.open_url(url)).into_any_element()
}

fn appearance(state: &Entity<AppState>, tokens: &Tokens, window: &mut Window, cx: &mut App) -> AnyElement {
    const THEMES: &[(&str, &str)] = &[("system", "Match system"), ("dark", "Dark"), ("light", "Light"), ("high-contrast", "High contrast")];
    let theme_value = cx.global::<theme::ThemeState>().preference.as_str();
    let theme_target = state.clone();
    let theme_select = select(
        state,
        "theme",
        as_static(THEMES, theme_value, "dark"),
        THEMES,
        Rc::new(move |value, _, cx| {
            theme_target.update(cx, |s, _| s.storage.set(keys::THEME_KEY, value));
            let preference = keys::ThemePreference::from_stored(value);
            {
                let state = cx.global_mut::<theme::ThemeState>();
                state.preference = preference;
                state.refresh();
            }
            theme::apply(cx);
            cx.refresh_windows();
        }),
        cx,
    )
    .into_any_element();
    let accent_now = cx.global::<theme::ThemeState>().accent;
    let picker_state = state.update(cx, |s, cx| s.accent_picker(window, cx));
    let reset_state = state.clone();
    let swatches = div()
        .flex()
        .items_center()
        .gap(sz(10.0))
        .child(div().text_size(sz(11.0)).text_color(tokens.muted).font_features(theme::fonts::tabular_nums()).child(accent_now.to_stored().to_uppercase()))
        .child(gpui_component::color_picker::ColorPicker::new(&picker_state).anchor(gpui::Anchor::TopRight))
        .when(!accent_now.is_default(), |d| {
            d.child(secondary_button("accent-reset", "Reset", "rotate-ccw", cx).on_click(move |_, window, cx| {
                let default = theme::Accent::DEFAULT;
                let picker = reset_state.update(cx, |s, cx| {
                    s.set_accent(default, cx);
                    s.accent_picker(window, cx)
                });
                picker.update(cx, |p, cx| p.set_value(default.swatch(), window, cx));
            }))
        })
        .into_any_element();
    let (floating, separate, scale) = {
        let s = state.read(cx);
        (s.floating_sidebar, s.separate_pins, cx.global::<theme::ThemeState>().scale)
    };
    let (fs, ss, pv) = (state.clone(), state.clone(), state.clone());
    let refresh = pref(state, "volta-refresh-collection-on-visit", cx) == "true";
    let rs = state.clone();
    section(
        tokens,
        "Appearance",
        vec![
            row(tokens, "blue", "palette", "Theme", "Choose the color scheme", theme_select),
            row(tokens, "pink", "palette", "Accent color", "Pick any color. It recolors buttons, toggles, highlights, and icons across Volta.", swatches),
            row(
                tokens,
                "indigo",
                "panel-left",
                "Floating sidebar",
                "Inset navigation with rounded corners",
                checkbox("floating", floating, tokens, move |value, cx| {
                    fs.update(cx, |s, cx| {
                        s.floating_sidebar = value;
                        s.storage.set(keys::FLOATING_SIDEBAR_KEY, if value { "true" } else { "false" });
                        cx.notify();
                    })
                }),
            ),
            row(
                tokens,
                "teal",
                "panel-left",
                "Separate pinned artists and albums",
                "Show albums and artists in their own sidebar sections",
                checkbox("separate-pins", separate, tokens, move |value, cx| {
                    ss.update(cx, |s, cx| {
                        s.separate_pins = value;
                        s.storage.set(keys::SEPARATE_PINNED_KEY, if value { "true" } else { "false" });
                        cx.notify();
                    })
                }),
            ),
            row(
                tokens,
                "teal",
                "scaling",
                "Interface scale",
                format!("Currently {}%. Open a full-screen preview to try a size before you keep it.", scale.round() as i32),
                secondary_button("preview-scale", "Preview scale", "eye", cx)
                    .on_click(move |_, _, cx| {
                        let current = cx.global::<theme::ThemeState>().scale;
                        pv.update(cx, |s, cx| {
                            s.scale_preview = Some((current, current));
                            cx.notify();
                        })
                    })
                    .into_any_element(),
            ),
            row(
                tokens,
                "green",
                "refresh-cw",
                "Refresh collection on Home visits",
                "Change “From Your Collection” every time Home opens",
                checkbox("refresh-collection", refresh, tokens, move |v, cx| {
                    set_pref(&rs, "volta-refresh-collection-on-visit", if v { "true" } else { "false" }, cx)
                }),
            ),
        ],
        "Shape the look and motion of the player.",
    )
}

fn clamp_step(raw: &str, fallback: i32) -> i32 {
    raw.parse::<f64>().ok().filter(|v| (1.0..=10.0).contains(v)).map(|v| v.round() as i32).unwrap_or(fallback)
}

fn playback(state: &Entity<AppState>, tokens: &Tokens, cx: &mut App) -> AnyElement {
    const QUALITY: &[(&str, &str)] = &[("original", "Original · no transcoding"), ("compatible", "Compatible · MP3 320 kbps")];
    const INFINITE_MODE: &[(&str, &str)] = &[("algorithm", "Algorithm suggestions"), ("random", "Random songs")];
    const TRANSITIONS: &[(&str, &str)] = &[("automix", "AutoMix"), ("crossfade", "Crossfade"), ("off", "Off")];
    const NORMALIZATION: &[(&str, &str)] = &[("off", "Off"), ("track", "Track gain"), ("album", "Album gain")];
    let player = state.read(cx).player.clone();
    let (original, normalization) = {
        let p = player.read(cx);
        (p.original, p.normalization)
    };
    let quality_player = player.clone();
    let quality = select(
        state,
        "streaming-quality",
        if original { "original" } else { "compatible" },
        QUALITY,
        Rc::new(move |v, _, cx| quality_player.update(cx, |p, cx| p.set_original(v == "original", cx))),
        cx,
    );
    let infinite_enabled = pref(state, keys::INFINITE_PLAY_ENABLED_KEY, cx) == "true";
    // `Number(raw)` like the web: an unset value is 0, which clamps to 1.
    let infinite_count = {
        let value = volta_logic::js::js_number(&pref(state, keys::INFINITE_PLAY_COUNT_KEY, cx));
        if value.is_finite() { (value.trunc() as i32).clamp(1, 200) } else { 50 }
    };
    let infinite_mode = if pref(state, keys::INFINITE_PLAY_MODE_KEY, cx) == "random" { "random" } else { "algorithm" };
    let warn = pref(state, keys::WARN_BEFORE_LEAVE_KEY, cx) != "false";
    let transition = {
        let raw = pref(state, keys::TRANSITION_MODE_KEY, cx);
        match raw.as_str() {
            "automix" => "automix",
            "crossfade" => "crossfade",
            "off" => "off",
            _ => {
                if pref(state, keys::CROSSFADE_KEY, cx).parse::<f64>().is_ok_and(|v| v > 0.0) {
                    "crossfade"
                } else {
                    "off"
                }
            }
        }
    };
    let crossfade = pref(state, keys::CROSSFADE_KEY, cx).parse::<f64>().ok().filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 12.0) as i32).unwrap_or(0);
    let step = clamp_step(&pref(state, keys::VOLUME_SCROLL_STEP_KEY, cx), 5);
    let shift_step = clamp_step(&pref(state, keys::VOLUME_SHIFT_SCROLL_STEP_KEY, cx), 10);
    let blur = pref(state, keys::LYRICS_BLUR_KEY, cx) != "false";
    let (s1, s2, s3, s4, s5, s6, s7, s8) =
        (state.clone(), state.clone(), state.clone(), state.clone(), state.clone(), state.clone(), state.clone(), state.clone());
    let transition_select = select(
        state,
        "transitions",
        transition,
        TRANSITIONS,
        Rc::new(move |mode, _, cx| {
            // Crossfade needs a usable length; seed 6 s on selection.
            if mode == "crossfade" && crossfade == 0 {
                set_pref(&s5, keys::CROSSFADE_KEY, "6", cx);
            }
            set_pref(&s5, keys::TRANSITION_MODE_KEY, mode, cx);
        }),
        cx,
    );
    let norm_player = player.clone();
    let norm_state = state.clone();
    let normalization_select = select(
        state,
        "normalization",
        match normalization {
            volta_logic::format::Normalization::Track => "track",
            volta_logic::format::Normalization::Album => "album",
            _ => "off",
        },
        NORMALIZATION,
        Rc::new(move |v, _, cx| {
            set_pref(&norm_state, keys::NORMALIZATION_KEY, v, cx);
            norm_player.update(cx, |p, cx| {
                p.normalization = volta_logic::format::Normalization::from_stored(v);
                cx.notify();
            });
        }),
        cx,
    );
    let mut rows = vec![
        row(tokens, "blue", "audio-lines", "Streaming quality", "Original sends the source file to your browser", quality.into_any_element()),
        row(
            tokens,
            "pink",
            "infinity",
            "Infinite Play",
            "Keep the queue filled automatically as it gets low",
            checkbox("infinite-play", infinite_enabled, tokens, move |_, cx| {
                s1.update(cx, |s, cx| s.toggle_infinite_play(cx))
            }),
        ),
        row(
            tokens,
            "pink",
            "list-music",
            "Infinite Play songs",
            "Choose how many tracks are added from the floating bar",
            stepper("infinite-count", infinite_count.to_string(), tokens, move |d, cx| {
                set_pref(&s2, keys::INFINITE_PLAY_COUNT_KEY, &(infinite_count + d).clamp(1, 200).to_string(), cx)
            }),
        ),
        row(
            tokens,
            "pink",
            "shuffle",
            "Infinite Play source",
            "Use your recommendations or choose songs randomly",
            pref_select(state, "infinite-mode", keys::INFINITE_PLAY_MODE_KEY, infinite_mode, INFINITE_MODE, cx),
        ),
        row(
            tokens,
            "orange",
            "triangle-alert",
            "Warn before leaving while playing",
            "Ask for confirmation before closing or leaving Volta",
            checkbox("warn-leave", warn, tokens, move |v, cx| set_pref(&s3, keys::WARN_BEFORE_LEAVE_KEY, if v { "true" } else { "false" }, cx)),
        ),
        row(
            tokens,
            "purple",
            "blend",
            "Song transitions",
            "AutoMix blends each pair of songs like a DJ, matching tempo and key. Crossfade uses a fixed length. Off keeps the gapless handoff.",
            transition_select.into_any_element(),
        ),
    ];
    if transition == "automix" {
        let albums_state = state.clone();
        let mix_albums = pref(state, keys::AUTOMIX_ALBUMS_KEY, cx) != "false";
        rows.push(row(
            tokens,
            "purple",
            "audio-lines",
            "Mix albums too",
            "Blend albums played in track order as well. Turn this off to keep an album gapless from start to finish.",
            checkbox("automix-albums", mix_albums, tokens, move |v, cx| {
                set_pref(&albums_state, keys::AUTOMIX_ALBUMS_KEY, if v { "true" } else { "false" }, cx)
            }),
        ));
    }
    if transition == "crossfade" {
        rows.push(row(
            tokens,
            "purple",
            "blend",
            "Crossfade length",
            "How long the two songs overlap. AutoMix ignores this and plans each blend itself.",
            stepper("crossfade", format!("{}s", if crossfade == 0 { 6 } else { crossfade }), tokens, move |d, cx| {
                let base = if crossfade == 0 { 6 } else { crossfade };
                set_pref(&s4, keys::CROSSFADE_KEY, &(base + d).clamp(1, 12).to_string(), cx)
            }),
        ));
    }
    let shift_label = if cfg!(target_os = "macos") { "Option" } else { "Alt" };
    rows.extend([
        row(
            tokens,
            "green",
            "volume2",
            "Volume scroll step",
            "Change the volume by this percentage with each scroll",
            stepper("volume-step", format!("{step}%"), tokens, move |d, cx| {
                set_pref(&s6, keys::VOLUME_SCROLL_STEP_KEY, &(step + d).clamp(1, 10).to_string(), cx)
            }),
        ),
        row(
            tokens,
            "green",
            "volume2",
            format!("{shift_label} + scroll step"),
            format!("Change the volume by this percentage while holding {shift_label} (or Ctrl) and scrolling"),
            stepper("volume-shift-step", format!("{shift_step}%"), tokens, move |d, cx| {
                set_pref(&s7, keys::VOLUME_SHIFT_SCROLL_STEP_KEY, &(shift_step + d).clamp(1, 10).to_string(), cx)
            }),
        ),
        row(
            tokens,
            "green",
            "volume2",
            "Loudness normalization",
            "Match volume across tracks using ReplayGain tags. Requires a server that exposes them.",
            normalization_select.into_any_element(),
        ),
        row(
            tokens,
            "blue",
            "file-text",
            "Blur surrounding lyrics",
            "Keep nearby lines soft while the current line plays",
            checkbox("lyrics-blur", blur, tokens, move |v, cx| set_pref(&s8, keys::LYRICS_BLUR_KEY, if v { "true" } else { "false" }, cx)),
        ),
    ]);
    section(tokens, "Playback", rows, "Streaming, queue behavior, and session handling.")
}

fn privacy(state: &Entity<AppState>, tokens: &Tokens, cx: &mut App) -> AnyElement {
    let external = pref(state, keys::EXTERNAL_LYRICS_KEY, cx) != "false";
    let history = pref(state, keys::LISTENING_HISTORY_ENABLED_KEY, cx) != "false";
    let update_check = pref(state, keys::UPDATE_CHECK_KEY, cx) != "false";
    let checking = state.read(cx).update_checking;
    let (s7, s8) = (state.clone(), state.clone());
    let channel_value = if pref(state, keys::UPDATE_CHANNEL_KEY, cx) == "nightly" { "nightly" } else { "stable" };
    let channel_state = state.clone();
    let channel_select = select(
        state,
        "update-channel",
        channel_value,
        &[("stable", "Stable"), ("nightly", "Nightly")],
        Rc::new(move |value, _, cx| {
            set_pref(&channel_state, keys::UPDATE_CHANNEL_KEY, value, cx);
            // The banner may belong to the other channel; check again right away.
            channel_state.update(cx, |s, cx| {
                s.update = None;
                s.check_for_updates(true, cx);
            });
        }),
        cx,
    );
    let (s1, s2, s3, s4, s5, s6) = (state.clone(), state.clone(), state.clone(), state.clone(), state.clone(), state.clone());
    let profile = state.read(cx).profile_summary();
    let storage_summary = state.read(cx).storage_summary();
    let clearing = state.read(cx).clearing_caches;
    let storage_row = div()
        .flex()
        .items_center()
        .gap(sz(15.0))
        .min_h(sz(54.0))
        .px(sz(12.0))
        .py(sz(10.0))
        .text_size(sz(12.0))
        .child(setting_icon("teal", "database"))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sz(3.0))
                .flex_1()
                .min_w_0()
                .child(div().font_weight(FontWeight::MEDIUM).child("Storage and caches"))
                .child(div().text_size(sz(11.0)).text_color(tokens.muted).child(storage_summary)),
        )
        .child(
            div()
                .flex()
                .gap(sz(8.0))
                .flex_wrap()
                .justify_end()
                .child(
                    secondary_button("open-storage", "Open storage", "folder-open", cx)
                        .on_click(|_, _, cx| open_folder(volta_services::storage::data_dir(), cx)),
                )
                .child(
                    secondary_button("open-cache", "Open cache", "folder-open", cx)
                        .on_click(|_, _, cx| open_folder(crate::app_state::artwork_cache_dir(), cx)),
                )
                .child(
                    secondary_button("clear-caches", if clearing { "Clearing…" } else { "Clear caches" }, "trash2", cx)
                        .on_click(move |_, _, cx| s5.update(cx, |s, cx| s.clear_caches(cx))),
                )
                .child(
                    secondary_button("refresh-storage", "", "refresh-cw", cx)
                        .px(sz(10.0))
                        .on_click(move |_, _, cx| s6.update(cx, |_, cx| cx.notify())),
                ),
        )
        .into_any_element();
    section(
        tokens,
        "Privacy",
        vec![
            row(
                tokens,
                "blue",
                "file-text",
                "External lyrics lookup",
                "When your server has no lyrics, look up song metadata with LRCLIB.",
                checkbox("external-lyrics", external, tokens, move |v, cx| {
                    set_pref(&s1, keys::EXTERNAL_LYRICS_KEY, if v { "true" } else { "false" }, cx)
                }),
            ),
            row(
                tokens,
                "green",
                "refresh-cw",
                "Check for updates",
                format!(
                    "Stable follows tagged releases. Nightly follows the newest successful build, open it to download your platform\u{2019}s artifact. You have {}.",
                    crate::build_info::summary()
                ),
                div()
                    .flex()
                    .items_center()
                    .gap(sz(10.0))
                    .child(channel_select)
                    .child(
                        secondary_button("check-updates-now", if checking { "Checking…" } else { "Check now" }, "", cx)
                            .on_click(move |_, _, cx| s8.update(cx, |s, cx| s.check_for_updates(true, cx))),
                    )
                    .child(checkbox("update-check", update_check, tokens, move |v, cx| {
                        set_pref(&s7, keys::UPDATE_CHECK_KEY, if v { "true" } else { "false" }, cx)
                    }))
                    .into_any_element(),
            ),
            row(
                tokens,
                "purple",
                "brain",
                "Personalized recommendations",
                "Keep local listening time, skips, favorites, searches, album views, and the learned model that ties them together.",
                checkbox("listening-history", history, tokens, move |v, cx| {
                    set_pref(&s2, keys::LISTENING_HISTORY_ENABLED_KEY, if v { "true" } else { "false" }, cx)
                }),
            ),
            row(
                tokens,
                "indigo",
                "activity",
                "Listening profile",
                profile,
                div()
                    .flex()
                    .gap(sz(8.0))
                    .child(secondary_button("view-profile", "View profile", "", cx).on_click(move |_, _, cx| {
                        s3.update(cx, |s, cx| {
                            s.dialog = Some(Dialog::ListeningHistory);
                            cx.notify();
                        })
                    }))
                    .child(
                        secondary_button("clear-profile", "Clear data", "", cx)
                            .on_click(move |_, _, cx| s4.update(cx, |s, cx| s.clear_listening_profile(cx))),
                    )
                    .into_any_element(),
            ),
            storage_row,
            {
                let target = state.clone();
                row(
                    tokens,
                    "pink",
                    "sliders-horizontal",
                    "Recommendation engine",
                    "Tune how adventurous the shelf is and watch the slate re-rank live.",
                    secondary_button("tune-engine", "Tune engine", "", cx)
                        .on_click(move |_, _, cx| {
                            target.update(cx, |s, cx| {
                                s.dialog = Some(Dialog::RecommendationEngine);
                                cx.notify();
                            })
                        })
                        .into_any_element(),
                )
            },
        ],
        "What Volta is allowed to remember locally. Nothing is ever sent to Volta.",
    )
}

/// Opens `path` in Finder, Explorer, or the Linux file manager, creating it first.
fn open_folder(path: Option<std::path::PathBuf>, cx: &mut App) {
    let Some(path) = path else { return };
    let _ = std::fs::create_dir_all(&path);
    log::info!("opening folder {}", path.display());
    cx.open_with_system(&path);
}

/// One label / value line in the About lists.
fn fact(tokens: &Tokens, label: &str, value: impl Into<SharedString>) -> AnyElement {
    div()
        .flex()
        .items_start()
        .justify_between()
        .gap(sz(18.0))
        .min_h(sz(38.0))
        .px(sz(14.0))
        .py(sz(10.0))
        .text_size(sz(12.0))
        .child(div().flex_none().text_color(tokens.muted).child(label.to_string()))
        .child(div().min_w_0().text_right().font_weight(FontWeight::MEDIUM).font_features(theme::fonts::tabular_nums()).child(value.into()))
        .into_any_element()
}

/// Settings → About: every version a bug report needs, in one place.
fn about(state: &Entity<AppState>, tokens: &Tokens, window: &Window, cx: &mut App) -> AnyElement {
    use crate::build_info as build;
    let channel = match build::CHANNEL {
        "stable" => "Stable release",
        "nightly" => "Nightly build",
        _ => "Development build",
    };
    let update_channel = if pref(state, keys::UPDATE_CHANNEL_KEY, cx) == "nightly" { "Nightly" } else { "Stable" };
    let scale = cx.global::<theme::ThemeState>().scale;
    let data_dir = volta_services::storage::data_dir();
    let logs_dir = crate::logs::root();
    let show = |p: &Option<std::path::PathBuf>| p.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "not available".into());
    let display = format!(
        "{} × {} at {}x, interface scale {}%",
        f32::from(window.viewport_size().width).round(),
        f32::from(window.viewport_size().height).round(),
        window.scale_factor(),
        scale.round()
    );
    let libraries = build::libraries();
    let renderer = if cfg!(target_os = "macos") {
        "Metal"
    } else if cfg!(target_os = "windows") {
        "DirectX"
    } else {
        "Vulkan (wgpu)"
    };

    let mut report = format!(
        "{}\nBuilt: {} ({} build)\nTarget: {}\nCompiler: {}\nOS: {}\nHardware: {}\nDisplay: {display}\nRenderer: {renderer}\nSQLite: {} (database layout v{})\nUpdates: {update_channel} channel\n",
        build::summary(),
        build::built_label(),
        build::PROFILE,
        build::TARGET,
        build::RUSTC,
        crate::logs::os_description(),
        crate::logs::hardware_description(),
        volta_services::storage::sqlite_version(),
        volta_services::storage::schema_version(),
    );
    for (name, version) in &libraries {
        report.push_str(&format!("{name} {version}\n"));
    }
    let copy_text = report.clone();
    let check_state = state.clone();

    let top = row(
        tokens,
        "pink",
        "info",
        format!("Volta {}", build::VERSION),
        format!("{channel} · commit {} · built {}", build::commit_label(), build::built_label()),
        div()
            .flex()
            .flex_wrap()
            .justify_end()
            .gap(sz(8.0))
            .child(secondary_button("about-copy", "Copy version info", "clipboard-copy", cx).on_click(move |_, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_text.clone()));
            }))
            .child(
                secondary_button("about-check", "Check for updates", "refresh-cw", cx)
                    .on_click(move |_, _, cx| check_state.update(cx, |s, cx| s.check_for_updates(true, cx))),
            )
            .into_any_element(),
    );

    let app = section(
        tokens,
        "Volta",
        vec![
            top,
            fact(tokens, "Version", build::VERSION),
            fact(tokens, "Build", channel),
            fact(tokens, "Update channel", update_channel),
            fact(tokens, "Commit", build::commit_label()),
            fact(tokens, "Built", format!("{} · {} build", build::built_label(), build::PROFILE)),
            fact(tokens, "Target", build::TARGET),
            fact(tokens, "Rust compiler", build::RUSTC.trim_start_matches("rustc ")),
        ],
        "Include these when you report a problem. Copy version info puts all of this on the clipboard.",
    );
    let system = section(
        tokens,
        "This device",
        vec![
            fact(tokens, "Operating system", crate::logs::os_description()),
            fact(tokens, "Hardware", crate::logs::hardware_description()),
            fact(tokens, "Display", display),
            fact(tokens, "Graphics", renderer),
            fact(tokens, "Audio engine", format!("symphonia {} · cpal {}", lib(&libraries, "symphonia"), lib(&libraries, "cpal"))),
        ],
        "Detected when Volta started. Nothing here is sent anywhere.",
    );
    let mut library_rows = vec![
        fact(tokens, "SQLite", format!("{} (database layout v{})", volta_services::storage::sqlite_version(), volta_services::storage::schema_version())),
    ];
    library_rows.extend(libraries.iter().map(|(name, version)| fact(tokens, name, *version)));
    let libs = section(tokens, "Libraries", library_rows, "The exact versions this build of Volta was compiled against.");
    let files = section(
        tokens,
        "Your data",
        vec![
            row(
                tokens,
                "teal",
                "database",
                "Storage",
                show(&data_dir),
                secondary_button("about-open-storage", "Open", "folder-open", cx).on_click({
                    let dir = data_dir.clone();
                    move |_, _, cx| open_folder(dir.clone(), cx)
                }).into_any_element(),
            ),
            row(
                tokens,
                "orange",
                "file-text",
                "Logs",
                show(&logs_dir),
                secondary_button("about-open-logs", "Open", "folder-open", cx).on_click({
                    let dir = logs_dir.clone();
                    move |_, _, cx| open_folder(dir.clone(), cx)
                }).into_any_element(),
            ),
        ],
        "Settings, the artwork cache, and log files all live in these folders.",
    );
    div().flex().flex_col().gap(sz(27.0)).child(app).child(system).child(libs).child(files).into_any_element()
}

fn lib<'a>(libraries: &[(&'a str, &'a str)], name: &str) -> &'a str {
    libraries.iter().find(|(n, _)| *n == name).map(|(_, v)| *v).unwrap_or("?")
}

/// Settings that trade smoothness for CPU, GPU, and battery.
fn performance(state: &Entity<AppState>, tokens: &Tokens, cx: &mut App) -> AnyElement {
    const FPS: &[(&str, &str)] = &[("15", "15 fps"), ("24", "24 fps"), ("30", "30 fps"), ("60", "60 fps"), ("120", "120 fps")];
    const ANIMATED: &[(&str, &str)] = &[("prominent", "Album and player views"), ("everywhere", "Everywhere"), ("off", "Off")];
    let bg_limit = pref(state, keys::BACKGROUND_FPS_LIMIT_KEY, cx) == "true";
    let experimental = pref(state, "volta-experimental-artwork-loading", cx) == "true";
    let (fps_state, es) = (state.clone(), state.clone());
    let animated_value = {
        let mode = pref(state, keys::ANIMATED_ARTWORK_MODE_KEY, cx);
        if mode == "off" || pref(state, keys::ANIMATED_ARTWORK_KEY, cx) == "false" {
            "off"
        } else if mode == "everywhere" {
            "everywhere"
        } else {
            "prominent"
        }
    };
    let animated_target = state.clone();
    let animated = select(
        state,
        "animated-artwork",
        animated_value,
        ANIMATED,
        Rc::new(move |mode, _, cx| {
            set_pref(&animated_target, keys::ANIMATED_ARTWORK_KEY, if mode == "off" { "false" } else { "true" }, cx);
            set_pref(&animated_target, keys::ANIMATED_ARTWORK_MODE_KEY, mode, cx);
        }),
        cx,
    );
    let fps_value = as_static(FPS, &pref(state, keys::FULLSCREEN_FPS_KEY, cx), "60");
    let fps_select = pref_select(state, "fullscreen-fps", keys::FULLSCREEN_FPS_KEY, fps_value, FPS, cx);
    section(
        tokens,
        "Performance",
        vec![
            row(tokens, "purple", "sparkles", "Animated artwork", "Choose where animated artwork is allowed to play", animated.into_any_element()),
            row(
                tokens,
                "pink",
                "gauge",
                "Full-screen player frame rate",
                "How often the animated background redraws. Lower uses less power; it is capped by your display",
                fps_select,
            ),
            row(
                tokens,
                "orange",
                "gauge",
                "Limit frame rate in background",
                "Cap Volta at about 30 fps while its window isn't focused, saving CPU and GPU. Off by default, so animations stay smooth. Takes effect on the next launch.",
                checkbox("background-fps-limit", bg_limit, tokens, move |v, cx| set_pref(&fps_state, keys::BACKGROUND_FPS_LIMIT_KEY, if v { "true" } else { "false" }, cx)),
            ),
            row(
                tokens,
                "orange",
                "flask-conical",
                "Experimental artwork loading",
                "Try aggressive artwork preloading",
                checkbox("experimental-artwork", experimental, tokens, move |v, cx| {
                    set_pref(&es, "volta-experimental-artwork-loading", if v { "true" } else { "false" }, cx)
                }),
            ),
        ],
        "Options that affect how smoothly Volta draws and how much power it uses.",
    )
}

/// Developer options, offered in every build on every OS.
fn developer(state: &Entity<AppState>, tokens: &Tokens, cx: &mut App) -> AnyElement {
    let console_at_startup = pref(state, keys::CONSOLE_AT_STARTUP_KEY, cx) == "true";
    let (open_state, toggle_state, mix_state) = (state.clone(), state.clone(), state.clone());
    section(
        tokens,
        "Developer",
        vec![
            row(
                tokens,
                "purple",
                "audio-lines",
                "AutoMix preview",
                "Pick two random songs from your library and hear the AutoMix transition between them, with the blend point, length, tempo and bass swap shown. Pick new songs or listen again.",
                secondary_button("open-automix-preview", "Open preview", "", cx)
                    .on_click(move |_, _, cx| crate::tools::open_automix_preview(&mix_state, cx))
                    .into_any_element(),
            ),
            row(
                tokens,
                "orange",
                "gauge",
                "Performance window",
                "Open a separate window with live frame rate, memory, and runtime metrics for Volta.",
                secondary_button("open-performance-window", "Click here", "", cx)
                    .on_click(move |_, _, cx| crate::tools::open_performance(&open_state, cx))
                    .into_any_element(),
            ),
            row(
                tokens,
                "gray",
                "file-text",
                "Open console at startup",
                "Show Volta's log output in a console window every time the app launches. Takes effect on the next launch.",
                checkbox("console-at-startup", console_at_startup, tokens, move |v, cx| {
                    set_pref(&toggle_state, keys::CONSOLE_AT_STARTUP_KEY, if v { "true" } else { "false" }, cx)
                }),
            ),
        ],
        "Tools for debugging and profiling Volta. They work the same on Linux, macOS, and Windows.",
    )
}

fn diagnostics(state: &Entity<AppState>, tokens: &Tokens, cx: &mut App) -> AnyElement {
    let (s2, s3) = (state.clone(), state.clone());
    let preview = |kind: crate::components::notifications::NoticeKind, label: &'static str, message: &'static str, id: &'static str| {
        let target = s2.clone();
        secondary_button(id, label, "", cx).min_h(sz(30.0)).px(sz(10.0)).py(sz(5.0)).on_click(move |_, _, cx| {
            target.update(cx, |s, cx| s.notify(message, kind, cx))
        })
    };
    use crate::components::notifications::NoticeKind;
    section(
        tokens,
        "Diagnostics",
        vec![
            row(
                tokens,
                "pink",
                "bell-ring",
                "Notification previews",
                "Preview each notification style and its entrance and dismissal motion.",
                div()
                    .flex()
                    .flex_wrap()
                    .justify_end()
                    .gap(sz(6.0))
                    .child(preview(NoticeKind::Info, "Info", "This is how an update looks.", "preview-info"))
                    .child(preview(NoticeKind::Success, "Success", "Your change was saved.", "preview-success"))
                    .child(preview(NoticeKind::Playback, "Playback", "Now playing a new song.", "preview-playback"))
                    .child(preview(NoticeKind::Error, "Error", "Something could not be completed.", "preview-error"))
                    .into_any_element(),
            ),
            row(
                tokens,
                "gray",
                "clipboard-copy",
                "Copy local diagnostic summary",
                "Create a browser, network, and viewport summary for a support report. It stays on this device until you choose where to paste it.",
                secondary_button("copy-diagnostics", "Copy summary", "", cx)
                    .on_click(move |_, window, cx| {
                        let summary = format!(
                            "Volta Desktop {}\nOS: {} {}\nWindow: {:.0}×{:.0}\nScale: {}%",
                            env!("CARGO_PKG_VERSION"),
                            std::env::consts::OS,
                            std::env::consts::ARCH,
                            f32::from(theme::content_size(window).width),
                            f32::from(theme::content_size(window).height),
                            cx.global::<theme::ThemeState>().scale
                        );
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(summary));
                        s3.update(cx, |s, cx| s.notify("Diagnostic summary copied.", NoticeKind::Info, cx));
                    })
                    .into_any_element(),
            ),
        ],
        "Development tools. These are only offered on the beta channel.",
    )
}

fn account_identity(tokens: &Tokens, username: &str, detail: &str, size: f32) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(sz(12.0))
        .min_w_0()
        .child(div().flex_none().text_color(tokens.muted).child(icon("circle-user-round", size)))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sz(3.0))
                .min_w_0()
                .child(div().text_size(sz(12.5)).font_weight(FontWeight::SEMIBOLD).child(username.to_string()))
                .child(div().text_size(sz(11.0)).text_color(tokens.muted).child(detail.to_string())),
        )
        .into_any_element()
}

fn account(state: &Entity<AppState>, tokens: &Tokens, cx: &mut App) -> AnyElement {
    let (username, server, accounts, connecting) = {
        let s = state.read(cx);
        let server = s.client_handle().map(|c| c.server.clone()).unwrap_or_default();
        (s.account_label(), server, s.accounts.clone(), s.connecting)
    };
    let local = state.read(cx).is_local();
    let disconnect = state.clone();
    let current = section(
        tokens,
        if local { "Session" } else { "Navidrome" },
        vec![
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(sz(15.0))
                .min_h(sz(54.0))
                .px(sz(12.0))
                .py(sz(10.0))
                .child(account_identity(tokens, &username, &server, 32.0))
                .child(secondary_button("disconnect", "Disconnect", "", cx).on_click(move |_, _, cx| disconnect.update(cx, |s, cx| s.disconnect(cx))))
                .into_any_element(),
        ],
        "The library Volta is currently connected to.",
    );
    let mut out = div().flex().flex_col().gap(sz(24.0)).child(current);
    if !accounts.is_empty() && !local {
        let rows = accounts
            .iter()
            .enumerate()
            .map(|(i, saved)| {
                let is_current = saved.server == server && saved.username == username;
                let (switch, forget) = (state.clone(), state.clone());
                let (a, b) = (saved.clone(), saved.clone());
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(sz(15.0))
                    .min_h(sz(54.0))
                    .px(sz(12.0))
                    .py(sz(10.0))
                    .child(account_identity(
                        tokens,
                        &saved.username,
                        saved.server.trim_start_matches("https://").trim_start_matches("http://"),
                        28.0,
                    ))
                    .child(
                        div()
                            .flex()
                            .gap(sz(8.0))
                            .child(
                                secondary_button(&format!("switch-{i}"), if is_current { "Current" } else { "Switch" }, "", cx)
                                    .when(is_current || connecting, |d| d.opacity(0.5))
                                    .on_click(move |_, _, cx| {
                                        if !is_current {
                                            let saved = a.clone();
                                            switch.update(cx, |s, cx| {
                                                s.connect(saved.server, saved.username, String::new(), Some(saved.auth), true, false, cx)
                                            });
                                        }
                                    }),
                            )
                            .child(
                                secondary_button(&format!("forget-{i}"), "Forget", "", cx)
                                    .on_click(move |_, _, cx| forget.update(cx, |s, cx| s.forget_account(&b.server, &b.username, cx))),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        out = out.child(section(tokens, "Accounts", rows, "Libraries you have signed in to. Volta keeps the auth token, not your password."));
    }
    out.into_any_element()
}

pub fn render(state: &Entity<AppState>, _content_width: f32, window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    let tokens = theme::tokens(cx).clone();
    let active = state.read(cx).settings_tab;
    let (username, server) = {
        let s = state.read(cx);
        (s.account_label(), s.client_handle().map(|c| c.server.clone()).unwrap_or_default())
    };
    let hero = div()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap(sz(20.0))
        .pt(sz(3.0))
        .px(sz(2.0))
        .pb(sz(23.0))
        .border_b(sz(1.0))
        .border_color(tokens.line)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sz(5.0))
                .min_w_0()
                .child(div().text_size(sz(10.0)).font_weight(FontWeight::SEMIBOLD).letter_spacing(sz(10.0 * 0.09)).text_color(tokens.muted).child("PREFERENCES"))
                .child(div().text_size(sz(31.0)).font_weight(FontWeight::BOLD).letter_spacing(sz(-0.65)).line_height(gpui::relative(1.2)).child("Settings"))
                .child(
                    div()
                        .max_w(sz(420.0))
                        .text_size(sz(12.5))
                        .line_height(gpui::relative(1.55))
                        .text_color(tokens.secondary)
                        .child("Tune how Volta looks, plays, and personalizes music for you. Everything below stays on this device."),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(sz(11.0))
                .min_w_0()
                .px(sz(15.0))
                .py(sz(10.0))
                .border(sz(1.0))
                .border_color(tokens.line)
                .rounded(sz(10.0))
                .bg(tokens.canvas)
                .child(div().flex_none().text_color(tokens.accent).child(icon("circle-user-round", 30.0)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(sz(2.0))
                        .min_w_0()
                        .child(div().text_size(sz(12.5)).font_weight(FontWeight::SEMIBOLD).child(username))
                        .child(div().text_size(sz(10.5)).text_color(tokens.muted).child(server)),
                ),
        );
    let tabs = div()
        .flex()
        .flex_wrap()
        .gap(sz(4.0))
        .p(sz(4.0))
        .border(sz(1.0))
        .border_color(tokens.line)
        .rounded(sz(12.0))
        .bg(tokens.sidebar)
        .children(TABS.iter().filter(|(id, _, _)| *id != "diagnostics" || diagnostics_enabled()).map(|&(id, label, glyph)| {
            let selected = id == active;
            let target = state.clone();
            div()
                .id(SharedString::from(format!("settings-tab-{id}")))
                .flex()
                .items_center()
                .gap(sz(7.0))
                .min_h(sz(32.0))
                .px(sz(13.0))
                .py(sz(6.0))
                .rounded(sz(8.0))
                .text_size(sz(12.0))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .map(|d| {
                    if selected {
                        d.text_color(tokens.text).bg(tokens.canvas).shadow(vec![
                            BoxShadow { color: theme::hex("#00000033"), offset: point(px(0.0), px(1.0)), blur_radius: px(3.0), spread_radius: px(0.0), inset: false },
                            BoxShadow { color: tokens.line, offset: point(px(0.0), px(0.0)), blur_radius: px(0.0), spread_radius: px(1.0), inset: true },
                        ])
                    } else {
                        d.text_color(tokens.secondary).hover(|s| s.text_color(tokens.text).bg(tokens.hover))
                    }
                })
                .on_click(move |_, _, cx| {
                    target.update(cx, |s, cx| {
                        s.settings_tab = id;
                        cx.notify();
                    })
                })
                .child(div().text_color(if selected { tokens.accent } else { tokens.secondary }).opacity(if selected { 1.0 } else { 0.85 }).child(icon(glyph, 15.0)))
                .child(label)
        }));
    let pane = match active {
        "playback" => playback(state, &tokens, cx),
        "privacy" => privacy(state, &tokens, cx),
        "sharing" => {
            const PROVIDERS: &[(&str, &str)] = &[("apple-music", "Apple Music"), ("spotify", "Spotify")];
            let value = if pref(state, keys::SHARE_PROVIDER_KEY, cx) == "spotify" { "spotify" } else { "apple-music" };
            section(
                &tokens,
                "Sharing",
                vec![row(
                    &tokens,
                    "green",
                    "share2",
                    "Share destination",
                    "Apple Music looks up a matching catalog link. Spotify shares a direct provider search link.",
                    pref_select(state, "share-provider", keys::SHARE_PROVIDER_KEY, value, PROVIDERS, cx),
                )],
                "Choose where copied music links should open for people outside Volta.",
            )
        }
        "keyboard" => {
            let target = state.clone();
            section(
                &tokens,
                "Keyboard",
                vec![row(
                    &tokens,
                    "indigo",
                    "keyboard",
                    "Keyboard shortcuts",
                    "Search, review, and customize every command",
                    secondary_button("open-shortcuts", "Open shortcuts", "", cx)
                        .on_click(move |_, _, cx| {
                            target.update(cx, |s, cx| {
                                s.recording_shortcut = None;
                                s.dialog = Some(Dialog::Shortcuts);
                                cx.notify();
                            })
                        })
                        .into_any_element(),
                )],
                "Review and remap every command in Volta.",
            )
        }
        "help" => section(
            &tokens,
            "Help and privacy",
            vec![
                row(&tokens, "blue", "life-buoy", "Help and troubleshooting", "Connect Navidrome, use local files, and solve common playback issues.", link_button("open-help", "Open help", "", crate::views::connect::HELP_URL, cx)),
                row(
                    &tokens,
                    "orange",
                    "file-text",
                    "Log files",
                    "Volta keeps a detailed log of its last three sessions, including performance figures, on this device only. Attach the latest folder when you report a problem.",
                    secondary_button("open-logs-help", "Open logs folder", "folder-open", cx)
                        .on_click(|_, _, cx| open_folder(crate::logs::root(), cx))
                        .into_any_element(),
                ),
                row(&tokens, "gray", "lock", "Privacy notes", "See what stays on this device and what optional services receive.", link_button("open-privacy", "Open privacy", "", crate::views::connect::PRIVACY_URL, cx)),
            ],
            "Find setup guidance, troubleshooting, and Volta’s privacy notes.",
        ),
        "account" => account(state, &tokens, cx),
        "performance" => performance(state, &tokens, cx),
        "about" => about(state, &tokens, window, cx),
        "developer" => developer(state, &tokens, cx),
        "diagnostics" if diagnostics_enabled() => diagnostics(state, &tokens, cx),
        _ => appearance(state, &tokens, window, cx),
    };
    div()
        .w_full()
        .max_w(sz(1080.0))
        .mx_auto()
        .flex()
        .flex_col()
        .gap(sz(27.0))
        .child(hero)
        .child(tabs)
        .child(pane)
        .into_any_element()
}
