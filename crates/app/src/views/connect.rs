//! Sign-in screen. Port of `src/app/connect.tsx` and the `.connect-*` rules
//! in `theme.css`.

use gpui::{
    App, Context, Entity, FontWeight, IntoElement, ParentElement, Render, SharedString, Styled, Subscription,
    Window, div, img, prelude::*, px,
};
use gpui_component::input::{Input, InputEvent, InputState};
use volta_logic::server::display_server;
use volta_logic::storage::account_key;

use crate::app_state::{AppState, read_login_prefill, save_login_draft};
use crate::components::ui::{IconButton, icon, icon_with};
use crate::theme::{self, mix, primary_fill, primary_fill_hover, sz, transparent};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Navidrome,
    Local,
}

pub const HELP_URL: &str = "https://player.voltamusic.xyz/help.html";
pub const PRIVACY_URL: &str = "https://player.voltamusic.xyz/privacy.html";

pub struct ConnectView {
    state: Entity<AppState>,
    server: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    show_password: bool,
    remember: bool,
    source: Source,
    _subscriptions: Vec<Subscription>,
}

impl ConnectView {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let storage = state.read(cx).storage.clone();
        let (server_value, username_value, remember) = read_login_prefill(&storage);
        let server = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://music.example.com")
                .default_value(server_value)
        });
        let username = cx.new(|cx| InputState::new(window, cx).placeholder("Username").default_value(username_value));
        let password = cx.new(|cx| InputState::new(window, cx).placeholder("Password").masked(true));
        let mut subscriptions = Vec::new();
        for input in [&server, &username, &password] {
            subscriptions.push(cx.subscribe_in(input, window, |this: &mut Self, _, event: &InputEvent, window, cx| {
                match event {
                    InputEvent::Change => this.save_draft(cx),
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    _ => {}
                }
            }));
        }
        Self {
            state,
            server,
            username,
            password,
            show_password: false,
            remember,
            source: Source::Navidrome,
            _subscriptions: subscriptions,
        }
    }

    fn save_draft(&mut self, cx: &mut Context<Self>) {
        let storage = self.state.read(cx).storage.clone();
        save_login_draft(&storage, &self.server.read(cx).value(), &self.username.read(cx).value());
    }

    fn submit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let server = self.server.read(cx).value().to_string();
        let username = self.username.read(cx).value().to_string();
        let password = self.password.read(cx).value().to_string();
        // The web form marks all three fields `required`.
        if server.trim().is_empty() || username.trim().is_empty() || password.is_empty() {
            self.state.update(cx, |state, cx| {
                state.connect_error = "Fill in the server, username and password.".into();
                cx.notify();
            });
            return;
        }
        let remember = self.remember;
        self.state.update(cx, |state, cx| state.connect(server, username, password, None, remember, false, cx));
    }

    fn try_demo(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.connect(
                "https://demo.navidrome.org".into(),
                "demo".into(),
                "demo".into(),
                None,
                false,
                false,
                cx,
            )
        });
    }

    fn field_row(&self, label: &'static str, input: impl IntoElement, last: bool, cx: &App) -> impl IntoElement {
        let tokens = theme::tokens(cx);
        div()
            .flex()
            .items_center()
            .gap(sz(12.0))
            .min_h(sz(48.0))
            .px(sz(12.0))
            .when(!last, |row| row.border_b(sz(1.0)).border_color(tokens.line))
            .text_size(sz(12.0))
            .text_color(tokens.secondary)
            .child(div().flex_none().child(label))
            .child(div().flex_1().min_w_0().flex().items_center().child(input))
    }

    fn text_input(&self, state: &Entity<InputState>, cx: &App) -> impl IntoElement {
        let tokens = theme::tokens(cx);
        Input::new(state)
            .appearance(false)
            .bordered(false)
            .focus_bordered(false)
            .w_full()
            .px_0()
            .py(sz(10.0))
            .text_size(sz(13.0))
            .text_color(tokens.text)
    }
}

impl Render for ConnectView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::tokens(cx).clone();
        let state = self.state.read(cx);
        let busy = state.connecting;
        let error = state.connect_error.clone();
        let accounts = state.accounts.clone();
        let local = self.source == Source::Local;

        let brand = div()
            .flex()
            .self_start()
            .items_center()
            .gap(sz(8.0))
            .text_size(sz(21.0))
            .font_weight(FontWeight::BOLD)
            .letter_spacing(sz(-0.7))
            .child(img("images/volta-bolt.svg").w(sz(23.0)).h(sz(23.0)).flex_none())
            .child("Volta");

        // `.connect-source-switch`: segmented control with a sliding pill.
        let switch_button = |id: &'static str, text: &'static str, selected: bool, source: Source| {
            div()
                .id(id)
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .min_h(sz(31.0))
                .rounded(sz(5.0))
                .text_size(sz(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if selected { tokens.text } else { tokens.secondary })
                // The selected segment carries the `::before` pill (canvas fill + shadow).
                .when(selected, |b| {
                    b.bg(tokens.canvas).shadow(vec![gpui::BoxShadow {
                        color: theme::hex("#0003"),
                        offset: gpui::point(px(0.0), px(1.0)),
                        blur_radius: px(4.0),
                        spread_radius: px(0.0),
                        inset: false,
                    }])
                })
                .cursor_pointer()
                .hover(|s| s.text_color(tokens.text))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.source = source;
                    cx.notify();
                }))
                .child(text)
        };
        let switch = div()
            .mt(sz(22.0))
            .p(sz(3.0))
            .border(sz(1.0))
            .border_color(tokens.line)
            .rounded(sz(8.0))
            .bg(tokens.sidebar)
            .grid()
            .grid_cols(2)
            .gap(sz(2.0))
            .child(switch_button("source-navidrome", "Navidrome", !local, Source::Navidrome))
            .child(switch_button("source-local", "This device", local, Source::Local));

        let primary = |id: &'static str, label: SharedString, enabled: bool| {
            div()
                .id(id)
                .w_full()
                .h(sz(39.0))
                .flex()
                .items_center()
                .justify_center()
                .gap(sz(8.0))
                .rounded(sz(7.0))
                .text_size(sz(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(gpui::white())
                .bg(primary_fill())
                .when(!enabled, |b| b.opacity(0.38))
                .when(enabled, |b| b.cursor_pointer().hover(|s| s.bg(primary_fill_hover())))
                .child(label)
        };

        let demo_button = |id: &'static str, label: &'static str, with_icon: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .gap(sz(6.0))
                .mt(sz(16.0))
                .mx_auto()
                .px(sz(8.0))
                .py(sz(5.0))
                .text_size(sz(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(tokens.secondary)
                .when(busy, |b| b.opacity(0.38))
                .when(!busy, |b| b.cursor_pointer().hover(|s| s.text_color(tokens.text)))
                .when(with_icon, |b| b.child(icon_with("play", 14.0, None, true)))
                .child(label)
        };

        let (library_name, track_count, local_busy, local_error) = {
            let l = &self.state.read(cx).local;
            (
                l.library.as_ref().map(|lib| lib.directory_name.clone()),
                l.library.as_ref().map(|lib| lib.tracks.len()),
                l.busy,
                l.error.clone(),
            )
        };
        let panel = if local {
            let choose_state = self.state.clone();
            let open_state = self.state.clone();
            let can_open = !busy && !local_busy && track_count.unwrap_or(0) > 0;
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(sz(12.0))
                        .mt(sz(16.0))
                        .px(sz(12.0))
                        .py(sz(11.0))
                        .border(sz(1.0))
                        .border_color(tokens.line)
                        .rounded(sz(9.0))
                        .bg(tokens.sidebar)
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .gap(sz(9.0))
                                .min_w_0()
                                .child(div().mt(sz(1.0)).text_color(tokens.accent).child(icon("folder-open", 18.0)))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(sz(4.0))
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_size(sz(12.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child(library_name.clone().unwrap_or_else(|| "No folder selected".into())),
                                        )
                                        .child(
                                            div()
                                                .text_size(sz(10.0))
                                                .line_height(gpui::relative(1.4))
                                                .text_color(tokens.muted)
                                                .child(match track_count {
                                                    Some(count) => format!("{count} supported tracks ready"),
                                                    None => "Choose your music folder to continue. (no music is uploaded)".into(),
                                                }),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .id("choose-folder")
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap(sz(8.0))
                                .min_h(sz(30.0))
                                .px(sz(10.0))
                                .py(sz(6.0))
                                .rounded(sz(7.0))
                                .bg(tokens.surface)
                                .text_color(tokens.accent)
                                .text_size(sz(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .when(busy || local_busy, |b| b.opacity(0.38))
                                .when(!busy && !local_busy, |b| {
                                    b.cursor_pointer()
                                        .hover(|s| s.bg(tokens.hover))
                                        .on_click(move |_, _, cx| choose_state.update(cx, |s, cx| s.choose_local_folder(cx)))
                                })
                                .child(icon("folder-open", 14.0))
                                .child(if local_busy {
                                    "Reading…"
                                } else if library_name.is_some() {
                                    "Change folder"
                                } else {
                                    "Choose folder"
                                }),
                        ),
                )
                .when_some(local_error, |d, error| {
                    d.child(div().mt(sz(9.0)).mx(sz(2.0)).text_size(sz(12.0)).text_color(tokens.accent).child(error))
                })
                .child(div().mt(sz(16.0)).child(primary("open-local", "Open local library".into(), can_open).when(can_open, |b| {
                    b.on_click(move |_, _, cx| open_state.update(cx, |s, cx| s.open_local_library(cx)))
                })))
                .child(demo_button("use-navidrome", "Use Navidrome instead", false).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.source = Source::Navidrome;
                        cx.notify();
                    },
                )))
                .into_any_element()
        } else {
            let show = self.show_password;
            let password_row = div()
                .flex()
                .items_center()
                .min_w_0()
                .flex_1()
                .child(div().flex_1().min_w_0().child(self.text_input(&self.password, cx)))
                .child(
                    IconButton::new("password-toggle", if show { "Hide password" } else { "Show password" }, icon(if show { "eye-off" } else { "eye" }, 16.0))
                        .width(25.0)
                        .color(tokens.muted)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.show_password = !this.show_password;
                            let masked = !this.show_password;
                            this.password.update(cx, |input, cx| input.set_masked(masked, window, cx));
                            cx.notify();
                        })),
                );
            let fields = div()
                .mt(sz(18.0))
                .mb(sz(20.0))
                .overflow_hidden()
                .border(sz(1.0))
                .border_color(tokens.line)
                .rounded(sz(9.0))
                .bg(tokens.sidebar)
                .child(self.field_row("Server", self.text_input(&self.server, cx), false, cx))
                .child(self.field_row("Username", self.text_input(&self.username, cx), false, cx))
                .child(self.field_row("Password", password_row, true, cx));
            let remember = self.remember;
            let checkbox = div()
                .size(sz(13.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(sz(3.0))
                .border(sz(1.0))
                .border_color(if remember { tokens.accent } else { tokens.muted })
                .when(remember, |c| c.bg(tokens.accent).text_color(gpui::white()).child(icon_with("check", 11.0, Some(3.0), false)));
            let remember_row = div()
                .id("remember")
                .flex()
                .items_center()
                .gap(sz(8.0))
                .mx(sz(2.0))
                .mb(sz(18.0))
                .text_size(sz(12.0))
                .text_color(tokens.secondary)
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.remember = !this.remember;
                    cx.notify();
                }))
                .child(checkbox)
                .child("Remember this sign-in on this device");
            let connect_label: SharedString = if busy { "Connecting…".into() } else { "Connect".into() };
            let mut submit = primary("connect", connect_label, !busy);
            if busy {
                submit = submit.child(crate::components::motion::spinner(17.0));
            } else {
                submit = submit.on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)));
            }
            let saved = (!accounts.is_empty()).then(|| {
                let title = div()
                    .flex()
                    .items_center()
                    .gap(sz(8.0))
                    .mb(sz(5.0))
                    .text_size(sz(10.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .letter_spacing(sz(10.0 * 0.1))
                    .text_color(tokens.muted)
                    .child("SAVED LIBRARIES")
                    .child(div().h(sz(1.0)).flex_1().bg(tokens.line));
                let rows = accounts.iter().enumerate().map(|(index, account)| {
                    let use_account = account.clone();
                    let forget = account.clone();
                    let key = account_key(&account.server, &account.username);
                    div()
                        .id(SharedString::from(format!("account-{key}")))
                        .relative()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .id(("saved-account", index))
                                .w_full()
                                .flex()
                                .items_center()
                                .gap(sz(11.0))
                                .min_h(sz(70.0))
                                .pl(sz(12.0))
                                .pr(sz(48.0))
                                .py(sz(10.0))
                                .border(sz(1.0))
                                .border_color(tokens.line)
                                .rounded(sz(11.0))
                                .bg(mix(tokens.surface, 42.0, transparent()))
                                .when(busy, |b| b.opacity(0.38))
                                .when(!busy, |b| {
                                    b.cursor_pointer()
                                        .hover(|s| {
                                            s.border_color(mix(tokens.accent, 34.0, tokens.line))
                                                .bg(mix(tokens.accent, 7.0, tokens.surface))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            let account = use_account.clone();
                                            this.state.update(cx, |state, cx| {
                                                state.connect(
                                                    account.server,
                                                    account.username,
                                                    String::new(),
                                                    Some(account.auth),
                                                    this.remember,
                                                    false,
                                                    cx,
                                                )
                                            });
                                        }))
                                })
                                .child(
                                    div()
                                        .size(sz(34.0))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .border(sz(1.0))
                                        .border_color(mix(tokens.accent, 34.0, tokens.line))
                                        .rounded(sz(10.0))
                                        .bg(mix(tokens.accent, 10.0, tokens.surface))
                                        .text_color(tokens.text)
                                        .child(icon_with("circle-user-round", 19.0, Some(1.7), false)),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(sz(2.0))
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_size(sz(13.0))
                                                .font_weight(FontWeight(650.0))
                                                .whitespace_nowrap()
                                                .overflow_hidden()
                                                .text_ellipsis()
                                                .child(account.username.clone()),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .text_size(sz(11.0))
                                                .line_height(gpui::relative(1.6))
                                                .text_color(tokens.muted)
                                                .child("Navidrome")
                                                .child(" · ")
                                                .child(display_server(&account.server).to_string()),
                                        ),
                                ),
                        )
                        .child(
                            div().absolute().right(sz(10.0)).child(
                                IconButton::new(
                                    ("forget", index),
                                    format!("Forget {} on {}", account.username, account.server),
                                    icon("x", 14.0),
                                )
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let (server, username) = (forget.server.clone(), forget.username.clone());
                                    this.state.update(cx, |state, cx| state.forget_account(&server, &username, cx));
                                })),
                            ),
                        )
                });
                div()
                    .flex()
                    .flex_col()
                    .gap(sz(8.0))
                    .mt(sz(22.0))
                    .pt(sz(19.0))
                    .border_t(sz(1.0))
                    .border_color(mix(tokens.line, 85.0, transparent()))
                    .child(title)
                    .children(rows)
            });
            div()
                .flex()
                .flex_col()
                .child(fields)
                .child(remember_row)
                .when(!error.is_empty(), |panel| {
                    panel.child(
                        div()
                            .mb(sz(13.0))
                            .text_size(sz(13.0))
                            .text_color(tokens.accent)
                            .child(error.clone()),
                    )
                })
                .child(submit)
                .children(saved)
                .child(demo_button("demo", "Try the Navidrome demo", true).when(!busy, |b| {
                    b.on_click(cx.listener(|this, _, _, cx| this.try_demo(cx)))
                }))
                .into_any_element()
        };

        let form = div()
            .w_full()
            .max_w(sz(354.0))
            .my_auto()
            .py(sz(60.0))
            .flex()
            .flex_col()
            .child(
                div()
                    .size(sz(72.0))
                    .mx_auto()
                    .mb(sz(24.0))
                    .child(img("images/volta-bolt.svg").size_full()),
            )
            .child(
                div()
                    .text_center()
                    .text_size(sz(34.0))
                    .font_weight(FontWeight::BOLD)
                    .letter_spacing(sz(-0.9))
                    .line_height(gpui::relative(1.1))
                    .mb(sz(13.0))
                    .child("Volta"),
            )
            .child(
                div()
                    .text_center()
                    .text_size(sz(14.0))
                    .line_height(gpui::relative(1.5))
                    .text_color(tokens.secondary)
                    .child(if local {
                        "Play music from a folder on this device."
                    } else {
                        "Connect to Navidrome to start listening."
                    }),
            )
            .child(switch)
            .child(panel);

        let link = |id: &'static str, text: &'static str, url: &'static str| {
            div()
                .id(id)
                .underline()
                .cursor_pointer()
                .hover(|s| s.text_color(tokens.text))
                .on_click(move |_, _, cx| cx.open_url(url))
                .child(text)
        };
        let footer = div()
            .flex()
            .items_center()
            .gap(sz(8.0))
            .mt_auto()
            .text_size(sz(11.0))
            .text_color(tokens.muted)
            .child(link("help", "Help and troubleshooting", HELP_URL))
            .child("·")
            .child(link("privacy", "Privacy", PRIVACY_URL));

        let _ = window;
        div()
            .id("connect-screen")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .bg(tokens.canvas)
            .text_color(tokens.text)
            .px(sz(35.0))
            .py(sz(27.0))
            .child(brand)
            .child(form)
            .child(footer)
    }
}
