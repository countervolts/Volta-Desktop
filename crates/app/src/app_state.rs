//! Application state: session, routing, persisted preferences, and the
//! library data the current page shows. This is the state half of the web's
//! `App()` monolith; the views render from it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{AppContext, Context, Entity, RenderImage};
use volta_logic::model::{AlbumRecord, Artist, Genre, Playlist, Song};
use volta_logic::server::server_candidates;
use volta_logic::storage::{self as keys, AuthMaterial, SavedAccount, StoredCredentials};
use volta_services::{Credentials, Navidrome, NavidromeError, Storage};

use crate::components::notifications::{Notice, NoticeKind};
use crate::player::Player;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Page {
    Home,
    Recent,
    Played,
    Albums,
    Artists,
    Songs,
    Folders,
    Favorites,
    Playlists,
    Search,
    Genre,
    Album,
    Artist,
    Playlist,
    Stats,
    Settings,
}

impl Page {
    /// The web's route segment name (`/<n>/albums`, …); screenshots route by it.
    #[cfg(feature = "shots")]
    pub fn from_name(name: &str) -> Option<Page> {
        use Page::*;
        [Home, Recent, Played, Albums, Artists, Songs, Folders, Favorites, Playlists, Search, Genre, Album, Artist, Playlist, Stats, Settings]
            .into_iter()
            .find(|page| page.name() == name)
    }

    /// The route segment (`from_name`'s inverse).
    pub fn name(self) -> &'static str {
        match self {
            Page::Home => "home",
            Page::Recent => "recent",
            Page::Played => "played",
            Page::Albums => "albums",
            Page::Artists => "artists",
            Page::Songs => "songs",
            Page::Folders => "folders",
            Page::Favorites => "favorites",
            Page::Playlists => "playlists",
            Page::Search => "search",
            Page::Genre => "genre",
            Page::Album => "album",
            Page::Artist => "artist",
            Page::Playlist => "playlist",
            Page::Stats => "stats",
            Page::Settings => "settings",
        }
    }

    /// `TITLES` in App.tsx.
    pub fn title(self) -> &'static str {
        match self {
            Page::Home => "Home",
            Page::Recent => "Recently Added",
            Page::Played => "Recently Played",
            Page::Albums => "Albums",
            Page::Artists => "Artists",
            Page::Songs => "Songs",
            Page::Folders => "Folders",
            Page::Favorites => "Favorites",
            Page::Playlists => "Playlists",
            Page::Search => "Search",
            Page::Genre => "Genre",
            Page::Album => "Album",
            Page::Artist => "Artist",
            Page::Playlist => "Playlist",
            Page::Stats => "Stats",
            Page::Settings => "Settings",
        }
    }

    pub fn is_detail(self) -> bool {
        matches!(self, Page::Album | Page::Artist | Page::Playlist | Page::Genre)
    }
}

/// `SongCreditsPanel` state.
#[derive(Clone)]
pub struct CreditsLookup {
    pub song_id: String,
    pub loading: bool,
    pub looked_up: bool,
    pub credits: Option<volta_logic::song_credits::SongCredits>,
}

/// Modal dialogs (app-dialogs.tsx).
#[derive(Clone)]
pub enum Dialog {
    SongDetails(Song),
    AlbumDetails(AlbumRecord),
    AddToPlaylist(Vec<Song>),
    /// New / Edit Playlist (`playlistDraft`); `song` is added on create.
    PlaylistDraft { playlist: Option<Playlist>, song: Option<Song> },
    DeletePlaylist(Playlist),
    /// Keyboard shortcuts reference and editor.
    Shortcuts,
    /// Listening profile (`ListeningHistoryView`).
    ListeningHistory,
    /// "Tune engine" (recommendation-engine-dialog.tsx).
    RecommendationEngine,
}

/// A text field the shell should focus on its next render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusTarget {
    Search,
    Filter,
}

/// The inspector panel (`panel` in App.tsx).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Queue,
    Lyrics,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub page: Page,
    pub id: Option<String>,
    pub title: Option<String>,
}

impl Route {
    pub fn page(page: Page) -> Self {
        Self { page, id: None, title: None }
    }
}

pub struct Session {
    pub client: Arc<Navidrome>,
}

#[derive(Default)]
pub struct HomeData {
    pub loading: bool,
    pub error: Option<String>,
    /// "From Your Collection": the first 8 of `getAlbumList2?type=random&size=32`.
    pub albums: Vec<AlbumRecord>,
    /// "Recently Added": `newest`, 12.
    pub recent: Vec<AlbumRecord>,
    /// "Recently Played": `recent`, 12 (the shelf shows 8).
    pub recently_played: Vec<AlbumRecord>,
    /// "Heavy Rotation": `frequent`, 12.
    pub frequent: Vec<AlbumRecord>,
    /// Ranking inputs for "Made for you" (`recommendationCandidates`,
    /// `recommendationPool`, `favoriteAlbumIds`).
    pub recommendation_candidates: Vec<AlbumRecord>,
    pub recommendation_pool: Vec<AlbumRecord>,
    pub favorite_album_ids: Vec<String>,
    /// Bumped on every successful load, so the slate re-ranks.
    pub generation: u64,
}

/// Data for the current non-Home route (`PageData` in app-model.ts).
#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PageData {
    pub albums: Vec<AlbumRecord>,
    pub artists: Vec<Artist>,
    pub songs: Vec<Song>,
    pub playlists: Vec<Playlist>,
    pub album: Option<AlbumRecord>,
    pub artist: Option<Artist>,
    pub playlist: Option<Playlist>,
    pub similar_albums: Vec<AlbumRecord>,
    pub similar_artists: Vec<Artist>,
    pub artist_songs: Vec<Song>,
}

#[derive(Clone, Copy)]
pub struct SidebarMotion {
    /// 0 = full width, 1 = icon rail.
    pub rail: f32,
    /// Label opacity, 0 – 1.
    pub labels: f32,
    pub rail_layout: bool,
    pub animating: bool,
}

pub struct AppState {
    pub storage: Storage,
    pub accounts: Vec<SavedAccount>,
    pub session: Option<Session>,
    pub history: Vec<Route>,
    pub sidebar_collapsed: bool,
    /// `flex-basis`/padding: 0.32s cubic-bezier(0.33, 1, 0.68, 1).
    sidebar_width_motion: crate::components::motion::Transition,
    /// Label `opacity 0.16s ease`.
    sidebar_label_motion: crate::components::motion::Transition,
    /// The ≤640px navigation drawer (`mobileSidebar`).
    pub mobile_sidebar: bool,
    /// The open inspector panel.
    pub panel: Option<Panel>,
    /// The full-window Now Playing view.
    pub full_player_open: bool,
    /// The open dropdown / context menu.
    pub menu: Option<crate::components::menu::OpenMenu>,
    pub dialog: Option<Dialog>,
    /// Favorite changes made this session, by song / album / artist id.
    pub favorite_overrides: HashMap<String, bool>,
    /// `volta-pins:<server>:<user>`.
    pub pins: Vec<keys::Pin>,
    /// The pin that just appeared, for the pin pop.
    pub pin_added: Option<(String, std::time::Instant)>,
    /// `volta-separate-pinned-entities`.
    pub separate_pins: bool,
    /// `volta-keyboard-shortcuts`, in definition order.
    pub shortcuts: Vec<(&'static str, volta_logic::shortcuts::Binding)>,
    pub focus_request: Option<FocusTarget>,
    /// The shortcut waiting for a new key combination.
    pub recording_shortcut: Option<&'static str>,
    pub settings_tab: &'static str,
    /// Interface scale preview: (scale before the preview, previewed scale).
    pub scale_preview: Option<(f32, f32)>,
    pub clearing_caches: bool,
    /// The stored listening history (`listeningEvents`).
    pub listening_events: Vec<volta_logic::listening_history::ListeningEvent>,
    /// Engagement, ranker and the cached recommendation slate.
    pub learning: crate::recommend::Learning,
    pub infinite: crate::infinite::InfinitePlay,
    /// `connectionState`.
    pub connection: crate::components::connection_banner::Connection,
    /// A newer release found by the update check (shown as a banner).
    pub update: Option<volta_logic::update::ReleaseInfo>,
    pub update_checking: bool,
    /// AutoMix track analysis queue.
    pub analyzer: crate::automix_analysis::Analyzer,
    /// Local music ("This device").
    pub local: crate::local::LocalState,
    /// `pageMemory`: the two most recent non-Home pages, by load key.
    pub page_memory: indexmap::IndexMap<String, std::rc::Rc<PageData>>,
    /// `lastLoadKey`: loading the same key again is a refresh.
    pub last_load_key: String,
    /// `loadMore` state.
    pub paging: crate::paging::Paging,
    /// Song multi-select.
    pub selection: crate::views::selection::Selection,
    /// OS media controls, once the window exists.
    pub media: Option<crate::media_controls::MediaSession>,
    /// The last playback snapshot written (`volta-last-played:…`).
    pub last_snapshot: String,
    pub stats_period: volta_logic::listening_stats::StatsPeriod,
    pub stats_offset: i32,
    /// Folders: the tree for the loaded songs and the browser state.
    pub folder_tree: Option<std::sync::Arc<volta_logic::folder_tree::FolderTree>>,
    pub folder_path: String,
    /// Set when a folder search result opens; the shell clears the field.
    pub folder_clear_search: bool,
    pub folder_history: Vec<String>,
    pub folder_history_index: usize,
    /// `volta-folder-view`: columns | list | icons.
    pub folder_view: &'static str,
    /// `volta-folder-sort`: name | kind | size | date.
    pub folder_sort: &'static str,
    pub scroll_top_request: bool,
    pub floating_sidebar: bool,
    pub connecting: bool,
    pub connect_error: String,
    /// False until a stored session has been restored or rejected.
    pub auth_ready: bool,
    pub playlists: Vec<Playlist>,
    pub home: HomeData,
    /// Shared so views can hold it for a frame without deep-copying the
    /// page's albums and songs.
    pub page: std::rc::Rc<PageData>,
    pub page_loading: bool,
    pub page_error: Option<String>,
    pub(crate) page_generation: u64,
    /// Album sort (`volta` web: `sort` state), a `getAlbumList2` type.
    pub album_sort: &'static str,
    /// `default` | `title` | `artist` | `album`.
    pub song_sort: &'static str,
    /// `name` | `albums`.
    pub artist_sort: &'static str,
    /// Recently Played tab: `albums` | `songs`.
    pub played_tab: &'static str,
    /// Favorites tab: `songs` | `albums` | `artists`.
    pub favorites_tab: &'static str,
    /// The open `Select` menu, if any.
    pub open_select: Option<&'static str>,
    /// Search: live text, the 120 ms debounced query, the result tab.
    pub search_query: String,
    pub search_debounced: String,
    search_generation: u64,
    pub search_tab: &'static str,
    /// `volta-search-history` (newest first, max 8).
    pub search_history: Vec<String>,
    pub genres: Vec<Genre>,
    pub genres_loading: bool,
    pub genres_error: Option<String>,
    /// Genre → cover art id of its first song (`None` = no artwork).
    pub genre_covers: HashMap<String, Option<String>>,
    genre_cover_pending: HashSet<String>,
    /// Luminosity-blended genre tile art, by genre.
    genre_tiles: HashMap<String, Arc<RenderImage>>,
    genre_tile_pending: HashSet<String>,
    /// The artist whose Top Songs list is expanded past 8.
    pub expanded_artist_songs: Option<String>,
    pub player: Entity<Player>,
    pub(crate) covers: crate::covers::CoverCache,
    cover_notify_pending: bool,
    pub notice: Option<crate::components::notifications::Notice>,
    notice_id: u64,
    pub playlist_busy: bool,
    /// Song credits lookup for the details dialog: (song id, loading, result, looked up).
    pub credits: Option<CreditsLookup>,
    /// The toolbar filter text (`filter` in App.tsx).
    pub filter: String,
}

pub enum ConnectOutcome {
    Ok(Arc<Navidrome>),
    Err(String),
}

/// `<data dir>/artwork-cache`.
fn artwork_cache_dir() -> Option<std::path::PathBuf> {
    volta_services::storage::data_dir().map(|dir| dir.join("artwork-cache"))
}

/// Mirrors the connect loop in App.tsx: try each candidate, fall back from
/// HTTPS to HTTP only on a connection failure, and validate with `ping`.
fn connect_blocking(server: &str, username: &str, password: &str, auth: Option<AuthMaterial>) -> ConnectOutcome {
    let candidates = match server_candidates(server) {
        Ok(c) => c,
        Err(e) => return ConnectOutcome::Err(e.to_string()),
    };
    let mut last_error: Option<NavidromeError> = None;
    for (index, candidate) in candidates.iter().enumerate() {
        let client = match Navidrome::new(Credentials {
            server: candidate.clone(),
            username: username.to_string(),
            password: password.to_string(),
            auth: auth.clone(),
        }) {
            Ok(c) => c,
            Err(e) => return ConnectOutcome::Err(e.to_string()),
        };
        match client.ping() {
            Ok(()) => return ConnectOutcome::Ok(Arc::new(client)),
            Err(error) => {
                let fall_through = index == 0 && candidates.len() > 1 && error.is_connection_failure();
                last_error = Some(error);
                if !fall_through {
                    break;
                }
            }
        }
    }
    match last_error {
        // Native apps have no CORS or mixed-content blocking, so an
        // unreachable host is the only connection diagnosis left.
        Some(error) if error.is_connection_failure() => {
            ConnectOutcome::Err("Cannot reach Navidrome. Check the server address and that it is online.".into())
        }
        Some(error) => ConnectOutcome::Err(error.to_string()),
        None => ConnectOutcome::Err("Connection failed.".into()),
    }
}

impl AppState {
    pub fn new(storage: Storage, cx: &mut Context<Self>) -> Self {
        #[cfg(feature = "shots")]
        crate::shot::STATE.with(|s| *s.borrow_mut() = Some(cx.weak_entity()));
        let folder_view = match storage.get("volta-folder-view").as_str() {
            "list" => "list",
            "icons" => "icons",
            _ => "columns",
        };
        let folder_sort = match storage.get("volta-folder-sort").as_str() {
            "kind" => "kind",
            "size" => "size",
            "date" => "date",
            _ => "name",
        };
        let shortcuts = volta_logic::shortcuts::read_bindings(&storage.get(volta_logic::shortcuts::SHORTCUT_STORAGE_KEY));
        let separate_pins = storage.get("volta-separate-pinned-entities") == "true";
        let search_history = keys::read_search_history(&storage.get(keys::SEARCH_HISTORY_KEY));
        let accounts = keys::read_accounts(&storage.get(keys::ACCOUNTS_KEY));
        let player = cx.new(Player::new);
        // Keep the listening history (when Personalized recommendations is on).
        cx.subscribe(&player, |state: &mut AppState, _, event: &volta_logic::listening_history::ListeningEvent, cx| {
            state.record_listening_event(event.clone(), cx);
            state.learn_from_playback(event);
        })
        .detach();
        // Persist volume per account whenever the player changes it.
        cx.observe(&player, |state: &mut AppState, player, cx| {
            if let Some(session) = state.session.as_ref() {
                let key = match state.local.library.as_ref().filter(|_| state.local.active) {
                    Some(library) => keys::local_volume_key(&library.directory_name),
                    None => keys::volume_key(&session.client.server, &session.client.username),
                };
                let value = keys::write_volume(&player.read(cx).volume_preference());
                if state.storage.get(&key) != value {
                    state.storage.set(&key, &value);
                }
            }
            state.check_infinite_play(cx);
            state.save_playback_snapshot(cx);
            state.sync_media_session(cx);
            state.prime_automix_analysis(cx);
        })
        .detach();
        let mut state = Self {
            sidebar_collapsed: storage.get(keys::SIDEBAR_COLLAPSED_KEY) == "true",
            sidebar_width_motion: crate::components::motion::Transition::new_at(320, storage.get(keys::SIDEBAR_COLLAPSED_KEY) == "true")
                .with_curve(0.33, 1.0, 0.68, 1.0),
            sidebar_label_motion: crate::components::motion::Transition::new_at(160, storage.get(keys::SIDEBAR_COLLAPSED_KEY) != "true"),
            floating_sidebar: storage.get(keys::FLOATING_SIDEBAR_KEY) != "false",
            storage,
            accounts,
            session: None,
            history: vec![Route::page(Page::Home)],
            connecting: false,
            connect_error: String::new(),
            auth_ready: false,
            playlists: Vec::new(),
            home: HomeData::default(),
            page: Default::default(),
            page_loading: false,
            page_error: None,
            page_generation: 0,
            album_sort: "alphabeticalByName",
            song_sort: "default",
            artist_sort: "name",
            played_tab: "albums",
            favorites_tab: "songs",
            open_select: None,
            search_query: String::new(),
            search_debounced: String::new(),
            search_generation: 0,
            search_tab: "all",
            search_history,
            genres: Vec::new(),
            genres_loading: false,
            genres_error: None,
            genre_covers: HashMap::new(),
            genre_cover_pending: HashSet::new(),
            genre_tiles: HashMap::new(),
            genre_tile_pending: HashSet::new(),
            mobile_sidebar: false,
            panel: None,
            full_player_open: false,
            menu: None,
            dialog: None,
            favorite_overrides: HashMap::new(),
            pins: Vec::new(),
            pin_added: None,
            separate_pins,
            shortcuts,
            focus_request: None,
            recording_shortcut: None,
            settings_tab: "appearance",
            scale_preview: None,
            clearing_caches: false,
            listening_events: Vec::new(),
            learning: Default::default(),
            infinite: Default::default(),
            last_snapshot: String::new(),
            media: None,
            selection: Default::default(),
            paging: Default::default(),
            page_memory: Default::default(),
            last_load_key: String::new(),
            local: Default::default(),
            analyzer: Default::default(),
            connection: Default::default(),
            update: None,
            update_checking: false,
            stats_period: volta_logic::listening_stats::StatsPeriod::Week,
            stats_offset: 0,
            folder_tree: None,
            folder_path: String::new(),
            folder_clear_search: false,
            folder_history: vec![String::new()],
            folder_history_index: 0,
            folder_view,
            folder_sort,
            scroll_top_request: false,
            expanded_artist_songs: None,
            player,
            covers: Default::default(),
            cover_notify_pending: false,
            notice: None,
            notice_id: 0,
            playlist_busy: false,
            credits: None,
            filter: String::new(),
        };
        state.restore_session(cx);
        // Never phone home from screenshot runs or when explicitly disabled.
        if std::env::var_os("VOLTA_SHOT").is_none() && std::env::var_os("VOLTA_NO_UPDATE_CHECK").is_none() {
            state.check_for_updates(false, cx);
        }
        state
    }

    /// Remembered credentials persist; session credentials last for the
    /// process (sessionStorage on the web). Restores validate with `ping`.
    fn restore_session(&mut self, cx: &mut Context<Self>) {
        let remembered = keys::read_stored_credentials(&self.storage.get(keys::REMEMBERED_CREDENTIALS_KEY));
        let session = keys::read_stored_credentials(&self.storage.session_get(keys::SESSION_CREDENTIALS_KEY));
        let Some(stored) = remembered.clone().or(session) else {
            #[cfg(feature = "shots")]
            if let Some(dir) = std::env::var_os("VOLTA_SHOT_LOCAL") {
                self.scan_local(std::path::PathBuf::from(dir), true, cx);
                return;
            }
            // The last source was a local folder: reopen it.
            if !self.restore_local(cx) {
                self.auth_ready = true;
                self.prepare_local_folder(cx);
            }
            return;
        };
        self.connect(
            stored.server,
            stored.username,
            String::new(),
            Some(stored.auth),
            remembered.is_some(),
            true,
            cx,
        );
    }

    pub fn route(&self) -> &Route {
        self.history.last().expect("route history is never empty")
    }

    pub fn navigate(&mut self, route: Route, cx: &mut Context<Self>) {
        if self.route() == &route {
            return;
        }
        self.record_route_engagement(&route);
        self.reset_selection();
        self.history.push(route);
        self.mobile_sidebar = false;
        self.filter.clear();
        // `pageRef.current?.scrollTo({ top: 0 })`.
        self.scroll_top_request = true;
        self.load_page(cx);
        cx.notify();
    }

    /// The engagement half of `navigate`: album, artist and genre views, and
    /// recommendation clicks.
    fn record_route_engagement(&mut self, route: &Route) {
        use volta_logic::interactions::{EngagementEntity, EngagementKind, EntityType};
        let Some(id) = route.id.clone().filter(|_| route.page != Page::Search) else { return };
        let entity = |kind| EngagementEntity { kind: Some(kind), id: Some(id.clone()), name: route.title.clone(), ..Default::default() };
        match route.page {
            Page::Album => {
                self.engage(EngagementKind::AlbumView, entity(EntityType::Album));
                if self.is_recommended(&id) {
                    self.engage(EngagementKind::RecommendationClick, entity(EntityType::Album));
                }
            }
            Page::Artist => self.engage(EngagementKind::ArtistView, entity(EntityType::Artist)),
            Page::Genre => self.engage(
                EngagementKind::GenreView,
                EngagementEntity { name: route.title.clone().or_else(|| Some(id.clone())), ..entity(EntityType::Genre) },
            ),
            _ => {}
        }
    }

    pub fn back(&mut self, cx: &mut Context<Self>) {
        if self.history.len() > 1 {
            self.reset_selection();
            self.history.pop();
            self.filter.clear();
            self.scroll_top_request = true;
            self.load_page(cx);
            cx.notify();
        }
    }

    /// The sidebar's collapse animation (`--sidebar-motion`): how far it is
    /// toward the rail, its labels' opacity, whether the rail layout shows,
    /// and whether it is still moving.
    pub fn sidebar_motion(&self) -> SidebarMotion {
        let (rail, moving_width) = self.sidebar_width_motion.value();
        let (labels, moving_labels) = self.sidebar_label_motion.value();
        SidebarMotion { rail, labels, rail_layout: self.sidebar_collapsed && labels <= 0.001, animating: moving_width || moving_labels }
    }

    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        self.sidebar_width_motion.set(self.sidebar_collapsed);
        self.sidebar_label_motion.set(!self.sidebar_collapsed);
        self.storage
            .set(keys::SIDEBAR_COLLAPSED_KEY, if self.sidebar_collapsed { "true" } else { "false" });
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn connect(
        &mut self,
        server: String,
        username: String,
        password: String,
        auth: Option<AuthMaterial>,
        remember: bool,
        restoring: bool,
        cx: &mut Context<Self>,
    ) {
        if self.connecting {
            return;
        }
        self.connecting = true;
        self.connect_error.clear();
        cx.notify();
        let task = cx.background_spawn(async move { connect_blocking(&server, &username, &password, auth) });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |state, cx| {
                state.connecting = false;
                state.auth_ready = true;
                match outcome {
                    ConnectOutcome::Ok(client) => state.connected(client, remember, cx),
                    ConnectOutcome::Err(message) => {
                        if restoring {
                            // A persisted token can expire while the app is closed.
                            state.storage.remove(keys::REMEMBERED_CREDENTIALS_KEY);
                            state.storage.session_remove(keys::SESSION_CREDENTIALS_KEY);
                        }
                        state.connect_error = message;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn connected(&mut self, client: Arc<Navidrome>, remember: bool, cx: &mut Context<Self>) {
        let stored = StoredCredentials {
            server: client.server.clone(),
            username: client.username.clone(),
            auth: client.auth.clone(),
        };
        self.accounts = keys::remember_account(&self.accounts, stored.clone());
        self.storage.set(keys::ACCOUNTS_KEY, &keys::write_accounts(&self.accounts));
        self.storage.remove(keys::LOCAL_SOURCE_MODE_KEY);
        self.storage.set(keys::SERVER_KEY, &client.server);
        self.storage.set(keys::USERNAME_KEY, &client.username);
        self.storage.set(keys::REMEMBER_ME_KEY, if remember { "true" } else { "false" });
        self.storage.session_remove(keys::LOGIN_DRAFT_KEY);
        let serialized = serde_json::to_string(&stored).unwrap_or_default();
        if remember {
            self.storage.set(keys::REMEMBERED_CREDENTIALS_KEY, &serialized);
        } else {
            self.storage.remove(keys::REMEMBERED_CREDENTIALS_KEY);
        }
        self.storage.session_set(keys::SESSION_CREDENTIALS_KEY, &serialized);
        let volume = keys::read_stored_volume(&self.storage.get(&keys::volume_key(&client.server, &client.username)))
            .unwrap_or_default();
        self.player.update(cx, |player, cx| {
            player.set_client(Some(client.clone()));
            player.restore_volume(volume, cx);
        });
        self.session = Some(Session { client: client.clone() });
        self.load_listening_history();
        self.load_learning();
        self.last_snapshot.clear();
        self.sync_transition(cx);
        self.restore_playback(cx);
        self.pins = keys::read_pins(&self.storage.get(&keys::pins_storage_key(&client.server, &client.username)));
        self.session = Some(Session { client: client.clone() });
        self.history = vec![Route::page(Page::Home)];
        self.load_playlists(cx);
        self.load_home(cx);
        #[cfg(feature = "shots")]
        if let Some(mut route) = crate::shot::initial_route() {
            // `settings:<tab>` opens that settings tab.
            if route.page == Page::Settings {
                if let Some(tab) = route.id.take() {
                    if let Some((id, _, _)) = crate::views::settings::TABS.iter().find(|(id, _, _)| *id == tab) {
                        self.settings_tab = id;
                    }
                }
            }
            // `search:<query>` opens Search with that query.
            if route.page == Page::Search {
                if let Some(query) = route.id.take() {
                    self.search_query = query.clone();
                    self.search_debounced = query;
                }
            }
            self.history.push(route);
            self.load_page(cx);
            // `VOLTA_SHOT_PANEL=queue|lyrics` plays the album and opens a panel.
            if std::env::var("VOLTA_SHOT_FULL").is_ok() {
                self.full_player_open = true;
                if let Some(id) = self.route().id.clone() {
                    self.play_album(id, cx);
                }
            }
            if let Ok(panel) = std::env::var("VOLTA_SHOT_PANEL") {
                self.panel = Some(if panel == "lyrics" { Panel::Lyrics } else { Panel::Queue });
                if let Some(id) = self.route().id.clone() {
                    self.play_album(id, cx);
                }
            }
        }
    }

    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.close_local();
        self.end_listening_session();
        self.listening_events.clear();
        self.learning = Default::default();
        self.player.update(cx, |player, cx| {
            player.stop(cx);
            player.set_client(None);
        });
        self.session = None;
        self.playlists.clear();
        self.home = HomeData::default();
        self.history = vec![Route::page(Page::Home)];
        self.storage.remove(keys::REMEMBERED_CREDENTIALS_KEY);
        self.storage.remove(keys::LOCAL_SOURCE_MODE_KEY);
        self.storage.session_remove(keys::SESSION_CREDENTIALS_KEY);
        self.storage.session_remove(keys::LOGIN_DRAFT_KEY);
        self.storage.set(keys::REMEMBER_ME_KEY, "false");
        cx.notify();
    }

    pub fn forget_account(&mut self, server: &str, username: &str, cx: &mut Context<Self>) {
        self.accounts = keys::forget_account(&self.accounts, server, username);
        self.storage.set(keys::ACCOUNTS_KEY, &keys::write_accounts(&self.accounts));
        self.notify(format!("Forgot {username} on {server}."), NoticeKind::Info, cx);
        cx.notify();
    }

    pub fn client_handle(&self) -> Option<Arc<Navidrome>> {
        self.client()
    }

    pub(crate) fn client(&self) -> Option<Arc<Navidrome>> {
        self.session.as_ref().map(|s| s.client.clone())
    }

    pub(crate) fn load_playlists(&mut self, cx: &mut Context<Self>) {
        if self.is_local() {
            return;
        }
        let Some(client) = self.client() else { return };
        let task = cx.background_spawn(async move { client.playlists() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |state, cx| match result {
                Ok(playlists) => {
                    state.playlists = playlists;
                    cx.notify();
                }
                Err(_) => {
                    state.notify("Playlists could not be loaded. Open Playlists to retry.", NoticeKind::Error, cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn load_home(&mut self, cx: &mut Context<Self>) {
        if self.is_local() {
            self.local_home();
            cx.notify();
            return;
        }
        let Some(client) = self.client() else { return };
        self.home.loading = true;
        self.home.error = None;
        cx.notify();
        // Same requests as App.tsx `case "home"`.
        let task = cx.background_spawn(async move {
            let random = client.albums("random", 32, 0)?;
            let recent = client.albums("newest", 12, 0)?;
            let recently_played = client.albums("recent", 12, 0)?;
            let frequent = client.albums("frequent", 12, 0)?;
            let favorites = client.favorites().unwrap_or_default();
            let pool = client.all_albums().unwrap_or_default();
            let mut favorite_ids: Vec<String> = Vec::new();
            for id in favorites
                .album
                .iter()
                .map(|a| a.id.clone())
                .chain(volta_logic::recommendations::favorite_album_ids_from_songs(&favorites.song))
            {
                if !favorite_ids.contains(&id) {
                    favorite_ids.push(id);
                }
            }
            let candidates = volta_logic::recommendations::merge_albums(&[
                &random,
                &recent,
                &recently_played,
                &frequent,
                &favorites.album,
            ]);
            let mut albums = random;
            albums.truncate(8);
            Ok::<_, NavidromeError>((albums, recent, recently_played, frequent, candidates, pool, favorite_ids))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |state, cx| {
                state.home.loading = false;
                state.note_connection(result.as_ref().err(), cx);
                match result {
                    Ok((albums, recent, recently_played, frequent, candidates, pool, favorite_ids)) => {
                        state.home.albums = albums;
                        state.home.recent = recent;
                        state.home.recently_played = recently_played;
                        state.home.frequent = frequent;
                        state.home.recommendation_candidates = candidates;
                        // The last real pool survives an empty reload (`homeRecommendationPool`).
                        if !pool.is_empty() {
                            state.home.recommendation_pool = pool;
                        }
                        state.home.favorite_album_ids = favorite_ids;
                        state.home.generation += 1;
                        #[cfg(feature = "shots")]
                        match std::env::var("VOLTA_SHOT_DIALOG").as_deref() {
                            Ok("history") => state.dialog = Some(Dialog::ListeningHistory),
                            Ok("engine") => state.dialog = Some(Dialog::RecommendationEngine),
                            Ok("offline") => state.connection = crate::components::connection_banner::Connection::Offline,
                            _ => {}
                        }
                    }
                    Err(error) => state.home.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn history_key(&self) -> Option<String> {
        self.client().map(|c| volta_logic::listening_history::listening_history_key(&c.server, &c.username))
    }

    /// Loads the stored listening history for the signed-in account.
    pub fn load_listening_history(&mut self) {
        let Some(key) = self.history_key() else { return };
        self.listening_events = if self.storage.get(keys::LISTENING_HISTORY_ENABLED_KEY) == "false" {
            Vec::new()
        } else {
            volta_logic::listening_history::read_listening_history(&self.lget(&key), volta_logic::listening_history::now_ms())
        };
    }

    /// `onPlaybackEvent`: append to the history unless it is turned off.
    pub fn record_listening_event(&mut self, event: volta_logic::listening_history::ListeningEvent, cx: &mut Context<Self>) {
        if self.storage.get(keys::LISTENING_HISTORY_ENABLED_KEY) == "false" {
            return;
        }
        let Some(key) = self.history_key() else { return };
        let mut events = std::mem::take(&mut self.listening_events);
        if !events.iter().any(|e| e.id == event.id) {
            events.push(event);
        }
        let (next, json) = volta_logic::listening_history::write_listening_history(events, volta_logic::listening_history::now_ms());
        self.lset(&key, &json);
        self.listening_events = next;
        cx.notify();
    }

    /// "N playback events · N interactions · N learning samples".
    pub fn profile_summary(&self) -> String {
        let (events, interactions, samples) = self.profile_counts();
        format!("{events} playback events · {interactions} interactions · {samples} learning samples")
    }

    /// Counts from the stored listening log, engagement log and ranker model.
    pub fn profile_counts(&self) -> (usize, usize, usize) {
        let Some(client) = self.client() else { return (0, 0, 0) };
        let suffix = format!(
            "{}:{}",
            volta_logic::js::encode_uri_component(&client.server),
            volta_logic::js::encode_uri_component(&client.username)
        );
        let count = |key: String, field: Option<&str>| -> usize {
            let raw = self.lget(&key);
            let value: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
            match field {
                Some(field) => value.get(field).and_then(|v| v.as_array().map(|a| a.len()).or_else(|| v.as_u64().map(|n| n as usize))).unwrap_or(0),
                None => value.as_array().map(|a| a.len()).unwrap_or(0),
            }
        };
        (
            self.listening_events.len(),
            self.learning.engagement.len().max(count(format!("volta-engagement:v1:{suffix}"), Some("events"))),
            self.learning.ranker.samples as usize,
        )
    }

    pub fn clear_listening_profile(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        let suffix = format!(
            "{}:{}",
            volta_logic::js::encode_uri_component(&client.server),
            volta_logic::js::encode_uri_component(&client.username)
        );
        for prefix in ["volta-listening-history:v1", "volta-engagement:v1", "volta-ranker-model:v1", "volta-ranker-impressions:v1"] {
            self.lremove(&format!("{prefix}:{suffix}"));
        }
        self.listening_events.clear();
        self.learning = Default::default();
        self.load_learning();
        self.notify("Your local listening profile was cleared.", NoticeKind::Info, cx);
    }

    /// "<used> used" for the storage panel: the database plus the cache dir.
    pub fn storage_summary(&self) -> String {
        let db = self.storage.path().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len()).unwrap_or(0);
        format!("{} used", volta_logic::format::format_bytes(db + self.cover_bytes()))
    }

    fn cover_bytes(&self) -> u64 {
        let disk: u64 = artwork_cache_dir()
            .and_then(|dir| std::fs::read_dir(dir).ok())
            .map(|entries| entries.filter_map(Result::ok).filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum())
            .unwrap_or(0);
        disk + self.covers.bytes() as u64
    }

    /// "Clear caches": artwork in memory, the session library cache.
    pub fn clear_caches(&mut self, cx: &mut Context<Self>) {
        self.clearing_caches = true;
        self.covers.clear(cx);
        if let Some(dir) = artwork_cache_dir() {
            let _ = std::fs::remove_dir_all(dir);
        }
        self.page_memory.clear();
        self.clearing_caches = false;
        self.notify("Caches cleared.", NoticeKind::Info, cx);
    }

    fn save_shortcuts(&self) {
        let map: serde_json::Map<String, serde_json::Value> = self
            .shortcuts
            .iter()
            .map(|(id, b)| (id.to_string(), serde_json::json!({"code": b.code, "mod": b.r#mod, "shift": b.shift, "alt": b.alt})))
            .collect();
        self.storage.set(volta_logic::shortcuts::SHORTCUT_STORAGE_KEY, &serde_json::Value::Object(map).to_string());
    }

    /// `recordShortcut`: modifiers alone and Tab are ignored; a binding
    /// already in use is refused with a toast.
    pub fn record_shortcut(&mut self, binding: volta_logic::shortcuts::Binding, cx: &mut Context<Self>) {
        let Some(id) = self.recording_shortcut else { return };
        use volta_logic::shortcuts::{binding_signature, definitions};
        if binding.code == "Tab" {
            return;
        }
        let signature = binding_signature(&binding);
        if let Some((other, _)) = self.shortcuts.iter().find(|(other, b)| *other != id && binding_signature(b) == signature) {
            let label = definitions().into_iter().find(|d| d.id == *other).map(|d| d.label).unwrap_or("");
            self.recording_shortcut = None;
            self.notify(format!("That shortcut is already assigned to “{label}”."), NoticeKind::Info, cx);
            return;
        }
        if let Some(slot) = self.shortcuts.iter_mut().find(|(other, _)| *other == id) {
            slot.1 = binding;
        }
        self.recording_shortcut = None;
        self.save_shortcuts();
        cx.notify();
    }

    pub fn reset_shortcuts(&mut self, cx: &mut Context<Self>) {
        self.shortcuts = volta_logic::shortcuts::definitions().into_iter().map(|d| (d.id, d.default)).collect();
        self.recording_shortcut = None;
        self.save_shortcuts();
        cx.notify();
    }

    /// `notify(message, kind)`: replaces any visible toast.
    pub fn notify(&mut self, message: impl Into<String>, kind: NoticeKind, cx: &mut Context<Self>) {
        self.notice_id += 1;
        let id = self.notice_id;
        self.notice = Some(Notice { id, kind, message: message.into(), leaving: None });
        let timer = cx.background_executor().timer(crate::components::notifications::VISIBLE);
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |state, cx| state.dismiss_notice(id, cx));
        })
        .detach();
        cx.notify();
    }

    pub fn dismiss_notice(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(notice) = self.notice.as_mut().filter(|n| n.id == id && n.leaving.is_none()) else { return };
        notice.leaving = Some(std::time::Instant::now());
        let timer = cx.background_executor().timer(crate::components::notifications::EXIT);
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |state, cx| {
                if state.notice.as_ref().is_some_and(|n| n.id == id) {
                    state.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// `addSongsToPlaylist`.
    pub fn add_songs_to_playlist(&mut self, playlist: Playlist, songs: Vec<Song>, cx: &mut Context<Self>) {
        if self.playlist_busy || songs.is_empty() {
            return;
        }
        let Some(client) = self.client() else { return };
        self.playlist_busy = true;
        cx.notify();
        let ids: Vec<String> = songs.iter().map(|s| s.id.clone()).collect();
        let target = playlist.id.clone();
        let task = cx.background_spawn(async move {
            client.update_playlist(&target, &volta_services::PlaylistChanges { song_ids_to_add: ids, ..Default::default() })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |state, cx| {
                state.playlist_busy = false;
                match result {
                    Ok(()) => {
                        state.dialog = None;
                        state.load_playlists(cx);
                        if state.route().page == Page::Playlist && state.route().id.as_deref() == Some(playlist.id.as_str()) {
                            state.load_page(cx);
                        }
                        let message = if songs.len() == 1 {
                            format!("Added “{}” to {}.", songs[0].title, playlist.name)
                        } else {
                            format!("Added {} songs to {}.", songs.len(), playlist.name)
                        };
                        for song in &songs {
                            state.engage(volta_logic::interactions::EngagementKind::PlaylistAdd, crate::recommend::entity_for_song(song));
                            state.learn_from_outcome(crate::recommend::target_for_song(song), 0.85, None);
                        }
                        state.notify(message, NoticeKind::Info, cx);
                    }
                    Err(error) => state.notify(format!("Could not update playlist: {error}"), NoticeKind::Error, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// "New Playlist" from the picker: a draft that adds `song` on create.
    pub fn create_playlist_with(&mut self, song: Option<Song>, cx: &mut Context<Self>) {
        self.dialog = Some(Dialog::PlaylistDraft { playlist: None, song });
        cx.notify();
    }

    pub fn edit_playlist(&mut self, playlist: Playlist, cx: &mut Context<Self>) {
        self.dialog = Some(Dialog::PlaylistDraft { playlist: Some(playlist), song: None });
        cx.notify();
    }

    /// `deletePlaylist`.
    pub fn delete_playlist(&mut self, playlist: Playlist, cx: &mut Context<Self>) {
        if self.playlist_busy {
            return;
        }
        let Some(client) = self.client() else { return };
        self.playlist_busy = true;
        cx.notify();
        let id = playlist.id.clone();
        let task = cx.background_spawn(async move { client.delete_playlist(&id) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |state, cx| {
                state.playlist_busy = false;
                match result {
                    Ok(()) => {
                        state.dialog = None;
                        state.load_playlists(cx);
                        state.navigate(Route::page(Page::Playlists), cx);
                        state.notify("Playlist deleted.", NoticeKind::Info, cx);
                    }
                    Err(error) => state.notify(format!("Could not delete playlist: {error}"), NoticeKind::Error, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// `canEditPlaylist`: not read-only and owned by this account (or ownerless).
    pub fn can_edit_playlist(&self, playlist: &Playlist) -> bool {
        playlist.readonly != Some(true)
            && playlist.owner.as_deref().is_none_or(|owner| owner.is_empty() || owner == self.account_label())
    }

    /// `savePlaylist`.
    pub fn save_playlist(&mut self, name: String, comment: String, cx: &mut Context<Self>) {
        let Some(Dialog::PlaylistDraft { playlist, song }) = self.dialog.clone() else { return };
        if self.playlist_busy {
            return;
        }
        let name = name.trim().to_string();
        if name.is_empty() {
            self.notify("Give this playlist a name first.", NoticeKind::Info, cx);
            return;
        }
        let Some(client) = self.client() else { return };
        self.playlist_busy = true;
        cx.notify();
        let comment: String = comment.trim().chars().take(494).collect();
        let editing = playlist.clone();
        let song_ids: Vec<String> = song.iter().map(|s| s.id.clone()).collect();
        let task = cx.background_spawn(async move {
            match editing {
                Some(existing) => client
                    .update_playlist(
                        &existing.id,
                        &volta_services::PlaylistChanges {
                            name: Some(name),
                            comment: Some(comment),
                            public: Some(existing.public == Some(true)),
                            ..Default::default()
                        },
                    )
                    .map(|_| None),
                None => client.create_playlist(&name, &song_ids),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |state, cx| {
                state.playlist_busy = false;
                match result {
                    Ok(created) => {
                        state.dialog = None;
                        state.load_playlists(cx);
                        if playlist.is_some() {
                            state.load_page(cx);
                            state.notify("Playlist updated.", NoticeKind::Info, cx);
                        } else {
                            match created {
                                Some(created) => state.navigate(
                                    Route { page: Page::Playlist, id: Some(created.id.clone()), title: Some(created.name.clone()) },
                                    cx,
                                ),
                                None => state.navigate(Route::page(Page::Playlists), cx),
                            }
                            let message = match &song {
                                Some(song) => format!("Added “{}” to a new playlist.", song.title),
                                None => "Playlist created.".into(),
                            };
                            state.notify(message, NoticeKind::Info, cx);
                            if let Some(song) = &song {
                                state.engage(volta_logic::interactions::EngagementKind::PlaylistAdd, crate::recommend::entity_for_song(song));
                                state.learn_from_outcome(crate::recommend::target_for_song(song), 0.9, None);
                            }
                        }
                    }
                    Err(error) => state.notify(format!("Could not save playlist: {error}"), NoticeKind::Error, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// `copyShareLink`: announce, resolve (Apple Music via iTunes Search),
    /// copy, announce the result.
    pub fn copy_share_link(&mut self, target: volta_logic::share_links::ShareTarget, cx: &mut Context<Self>) {
        let provider = keys::read_share_provider(&self.storage.get(keys::SHARE_PROVIDER_KEY));
        let apple = provider == keys::ShareProvider::AppleMusic;
        let name = if apple { "Apple Music" } else { "Spotify" };
        self.notify(if apple { "Finding an Apple Music link…" } else { "Preparing a Spotify link…" }, NoticeKind::Info, cx);
        let task = cx.background_spawn(async move { volta_services::share::resolve_share_url(provider, &target) });
        cx.spawn(async move |this, cx| {
            let link = task.await;
            let _ = this.update(cx, |state, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(link.url.clone()));
                let message = if link.exact { format!("{name} link copied.") } else { format!("{name} search link copied.") };
                state.notify(message, NoticeKind::Info, cx);
            });
        })
        .detach();
    }

    pub fn share_song(&mut self, song: &Song, cx: &mut Context<Self>) {
        use volta_logic::share_links::{ShareKind, ShareTarget};
        self.copy_share_link(
            ShareTarget {
                kind: ShareKind::Song,
                title: song.title.clone(),
                artist: song.artist.clone(),
                album: song.album.clone(),
                duration: song.duration,
            },
            cx,
        );
    }

    pub fn share_album(&mut self, album: &AlbumRecord, cx: &mut Context<Self>) {
        use volta_logic::share_links::{ShareKind, ShareTarget};
        self.copy_share_link(
            ShareTarget {
                kind: ShareKind::Album,
                title: volta_logic::format::album_name(album).to_string(),
                artist: album.artist.clone(),
                album: None,
                duration: None,
            },
            cx,
        );
    }

    pub fn share_artist(&mut self, artist: &Artist, cx: &mut Context<Self>) {
        use volta_logic::share_links::{ShareKind, ShareTarget};
        self.copy_share_link(
            ShareTarget { kind: ShareKind::Artist, title: artist.name.clone(), artist: None, album: None, duration: None },
            cx,
        );
    }

    pub fn is_pinned(&self, kind: keys::PinKind, id: &str) -> bool {
        self.pins.iter().any(|p| p.kind == kind && p.id == id)
    }

    /// `togglePin`: persists, announces, and pops new album/artist pins.
    pub fn toggle_pin(&mut self, pin: keys::Pin, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        let exists = self.is_pinned(pin.kind, &pin.id);
        if exists {
            self.pins.retain(|p| !(p.kind == pin.kind && p.id == pin.id));
        } else {
            self.pins.push(pin.clone());
        }
        self.storage.set(
            &keys::pins_storage_key(&client.server, &client.username),
            &serde_json::to_string(&self.pins).unwrap_or_else(|_| "[]".into()),
        );
        let message = if exists { format!("Unpinned {}.", pin.name) } else { format!("Pinned {}.", pin.name) };
        self.notify(message, NoticeKind::Success, cx);
        if !exists && pin.kind != keys::PinKind::Playlist {
            self.pin_added = Some((keys::pin_key(&pin), std::time::Instant::now()));
        }
        cx.notify();
    }

    pub fn is_favorite_song(&self, song: &Song) -> bool {
        self.favorite_overrides.get(&song.id).copied().unwrap_or(song.starred.is_some())
    }

    pub fn is_favorite_id(&self, id: &str, starred: bool) -> bool {
        if let Some(local) = self.favorite_overrides.get(id).copied().or_else(|| self.is_local_favorite(id)) {
            return local;
        }
        starred
    }

    /// Optimistic favorite toggle (`favorite` in App.tsx); reverts on failure.
    pub fn toggle_favorite(&mut self, kind: &'static str, id: String, starred: bool, cx: &mut Context<Self>) {
        if self.toggle_local_favorite(&id, cx) {
            return;
        }
        let active = self.is_favorite_id(&id, starred);
        self.favorite_overrides.insert(id.clone(), !active);
        cx.notify();
        let Some(client) = self.client() else { return };
        let target = id.clone();
        let task = cx.background_spawn(async move { client.star_item(kind, &target, active) });
        cx.spawn(async move |this, cx| {
            if task.await.is_err() {
                let _ = this.update(cx, |state, cx| {
                    state.favorite_overrides.insert(id, active);
                    state.notify("Favorite could not be saved. Try again.", NoticeKind::Error, cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn toggle_song_favorite(&mut self, song: &Song, cx: &mut Context<Self>) {
        let active = !self.is_favorite_id(&song.id, song.starred.is_some());
        self.toggle_favorite("song", song.id.clone(), song.starred.is_some(), cx);
        use volta_logic::interactions::EngagementKind;
        self.engage(if active { EngagementKind::FavoriteAdd } else { EngagementKind::FavoriteRemove }, crate::recommend::entity_for_song(song));
        self.learn_from_outcome(crate::recommend::target_for_song(song), if active { 1.0 } else { -0.5 }, None);
    }

    /// "Find credits": MusicBrainz, only on request.
    pub fn lookup_credits(&mut self, song: Song, cx: &mut Context<Self>) {
        if self.credits.as_ref().is_some_and(|c| c.song_id == song.id && c.loading) {
            return;
        }
        self.credits = Some(CreditsLookup { song_id: song.id.clone(), loading: true, looked_up: false, credits: None });
        cx.notify();
        let id = song.id.clone();
        let task = cx.background_spawn(async move { volta_services::musicbrainz::lookup_song_credits(&song) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |state, cx| {
                if state.credits.as_ref().is_some_and(|c| c.song_id == id) {
                    state.credits = Some(CreditsLookup { song_id: id, loading: false, looked_up: true, credits: result });
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// `queueSong`: Play Next / Play Last with the web's toast.
    pub fn queue_song(&mut self, song: Song, next: bool, cx: &mut Context<Self>) {
        let message = if next {
            format!("“{}” will play next.", song.title)
        } else {
            format!("“{}” added to the queue.", song.title)
        };
        use volta_logic::interactions::EngagementKind;
        self.engage(if next { EngagementKind::PlayNext } else { EngagementKind::QueueAdd }, crate::recommend::entity_for_song(&song));
        self.player.update(cx, |p, cx| p.append(song, next, cx));
        self.notify(message, NoticeKind::Success, cx);
    }

    /// `queueAlbum`: loads the album, then appends (reversed for Play Next so
    /// the album keeps its order).
    /// The album with its songs: from the scanned folder in local mode,
    /// otherwise `getAlbum`.
    pub(crate) fn album_task(&self, id: String, cx: &mut Context<Self>) -> Option<gpui::Task<Result<AlbumRecord, NavidromeError>>> {
        if self.is_local() {
            let album = self.local.library.as_ref()?.albums.iter().find(|a| a.id == id).cloned();
            return Some(cx.background_spawn(async move { album.ok_or_else(|| NavidromeError::Other("Album not found".into())) }));
        }
        let client = self.client()?;
        Some(cx.background_spawn(async move { client.album(&id) }))
    }

    pub fn queue_album(&mut self, album: AlbumRecord, next: bool, cx: &mut Context<Self>) {
        let player = self.player.clone();
        let Some(task) = self.album_task(album.id.clone(), cx) else { return };
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |state, cx| match result {
                Ok(loaded) => {
                    let mut songs = loaded.song.unwrap_or_default();
                    if next {
                        songs.reverse();
                    }
                    player.update(cx, |p, cx| {
                        for song in songs {
                            p.append(song, next, cx);
                        }
                    });
                    use volta_logic::interactions::EngagementKind;
                    state.engage(if next { EngagementKind::PlayNext } else { EngagementKind::QueueAdd }, crate::recommend::entity_for_album(&album));
                    let name = volta_logic::format::album_name(&album).to_string();
                    let message = if next { format!("{name} will play next.") } else { format!("{name} added to the queue.") };
                    state.notify(message, NoticeKind::Success, cx);
                }
                Err(_) => state.notify("This album could not be added to the queue. Try again.", NoticeKind::Info, cx),
            });
        })
        .detach();
    }

    pub fn shuffle_album(&mut self, album_id: String, cx: &mut Context<Self>) {
        let player = self.player.clone();
        let Some(task) = self.album_task(album_id, cx) else { return };
        cx.spawn(async move |this, cx| match task.await {
            Ok(album) => {
                use rand::seq::SliceRandom;
                let _ = this.update(cx, |state, _| {
                    state.engage(volta_logic::interactions::EngagementKind::AlbumShuffle, crate::recommend::entity_for_album(&album));
                });
                let mut songs = album.song.unwrap_or_default();
                songs.shuffle(&mut rand::rng());
                player.update(cx, |p, cx| p.play_queue(songs, 0, cx));
            }
            Err(_) => {
                let _ = this.update(cx, |state, cx| state.notify("This album could not be played. Try again.", NoticeKind::Info, cx));
            }
        })
        .detach();
    }

    pub fn open_song_details(&mut self, song: Song, cx: &mut Context<Self>) {
        self.credits = None;
        self.dialog = Some(Dialog::SongDetails(song));
        cx.notify();
    }

    pub fn open_add_to_playlist(&mut self, songs: Vec<Song>, cx: &mut Context<Self>) {
        self.dialog = Some(Dialog::AddToPlaylist(songs));
        cx.notify();
    }

    /// Dock buttons toggle a panel (`setPanel(old => old === value ? null : value)`).
    pub fn toggle_panel(&mut self, panel: Panel, cx: &mut Context<Self>) {
        self.panel = if self.panel == Some(panel) { None } else { Some(panel) };
        cx.notify();
    }

    /// Reloads the current route ("Refresh view").
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.route().page == Page::Home {
            self.load_home(cx);
        } else {
            self.load_page(cx);
        }
    }

    pub fn set_album_sort(&mut self, sort: &'static str, cx: &mut Context<Self>) {
        self.album_sort = sort;
        self.load_page(cx);
    }

    /// The per-route loader: the `switch (route.page)` in App.tsx.
    pub fn load_page(&mut self, cx: &mut Context<Self>) {
        crate::covers::bump_epoch();
        let route = self.route().clone();
        if route.page == Page::Home {
            // Exposures recorded on the last visit now count (App.tsx effect).
            self.reload_engagement();
            // "Refresh collection on Home visits" reloads the random shelf.
            let refresh = self.storage.get("volta-refresh-collection-on-visit") == "true";
            if (refresh || self.home.albums.is_empty()) && !self.home.loading {
                self.load_home(cx);
            }
            return;
        }
        if self.is_local() {
            self.page = std::rc::Rc::new(if route.page == Page::Search { self.local_search(&self.search_debounced) } else { self.local_page_data(&route) });
            if route.page == Page::Folders {
                self.folder_tree = Some(std::sync::Arc::new(volta_logic::folder_tree::build_folder_tree(&self.page.songs)));
            }
            self.page_loading = false;
            self.page_error = None;
            self.reset_paging();
            cx.notify();
            return;
        }
        let Some(client) = self.client() else { return };
        // `loadKey`: page, id, query, sort. Revisits come from memory; the
        // session cache shows a recent copy while the fresh load runs.
        let load_key = [route.page.name(), route.id.as_deref().unwrap_or(""), self.search_debounced.as_str(), self.album_sort].join("\u{1f}");
        let refresh = self.last_load_key == load_key;
        self.last_load_key = load_key.clone();
        let cache_key = format!(
            "volta-library-cache:{}:{}:{}",
            volta_logic::js::encode_uri_component(&client.server),
            volta_logic::js::encode_uri_component(&client.username),
            volta_logic::js::encode_uri_component(&load_key)
        );
        if route.page != Page::Search && route.page != Page::Folders && !refresh {
            if let Some(data) = self.page_memory.shift_remove(&load_key) {
                self.page_memory.insert(load_key, data.clone());
                self.page_generation += 1;
                self.page = data;
                self.page_loading = false;
                self.page_error = None;
                self.reset_paging();
                cx.notify();
                return;
            }
        }
        self.page_generation += 1;
        let generation = self.page_generation;
        self.page = Default::default();
        if route.page != Page::Search && !refresh {
            let cached: Option<serde_json::Value> = serde_json::from_str(&self.storage.session_get(&cache_key)).ok();
            if let Some(value) = cached.filter(|v| {
                v.get("savedAt").and_then(|t| t.as_f64()).is_some_and(|t| volta_logic::listening_history::now_ms() - t <= 15.0 * 60.0 * 1000.0)
            }) {
                if let Some(data) = value.get("data").and_then(|d| serde_json::from_value::<PageData>(d.clone()).ok()) {
                    self.page = data.into();
                }
            }
        }
        self.page_error = None;
        if route.page == Page::Search {
            self.page_loading = false;
            if self.genres.is_empty() && !self.genres_loading {
                self.load_genres(cx);
            }
            if !self.search_debounced.is_empty() {
                self.run_search(cx);
            }
            cx.notify();
            return;
        }
        let needs_data = !matches!(route.page, Page::Settings | Page::Stats);
        self.page_loading = needs_data;
        cx.notify();
        if !needs_data {
            return;
        }
        let sort = self.album_sort;
        // `recentlyPlayedSongIds`: newest first, distinct, at most 60.
        let played_ids: Vec<String> = {
            let mut seen = HashSet::new();
            self.listening_events
                .iter()
                .rev()
                .filter(|e| !e.song_id.is_empty() && seen.insert(e.song_id.clone()))
                .take(60)
                .map(|e| e.song_id.clone())
                .collect()
        };
        let (memory_key, session_key, route_page) = (load_key, cache_key, route.page);
        let task = cx.background_spawn(async move {
            let id = route.id.clone().unwrap_or_default();
            let mut data = PageData::default();
            match route.page {
                Page::Albums => data.albums = client.albums(sort, 60, 0)?,
                Page::Recent => data.albums = client.albums("newest", 60, 0)?,
                Page::Played => {
                    data.albums = client.albums("recent", 60, 0)?;
                    data.songs = played_ids.iter().filter_map(|id| client.song(id).ok()).collect();
                }
                Page::Artists => data.artists = client.artists()?,
                Page::Songs => data.songs = client.songs(0, 100)?,
                Page::Folders => data.songs = client.all_songs(100, None)?,
                Page::Favorites => {
                    let results = client.favorites()?;
                    data.songs = results.song;
                    data.albums = results.album;
                    data.artists = results.artist;
                }
                Page::Playlists => data.playlists = client.playlists()?,
                Page::Album => {
                    let album = client.album(&id)?;
                    let library = client.albums("alphabeticalByName", 500, 0)?;
                    data.similar_albums = volta_logic::library::similar_albums_for(&album, &library);
                    data.songs = album.song.clone().unwrap_or_default();
                    data.album = Some(album);
                }
                Page::Artist => {
                    let artist = client.artist(&id)?;
                    let library = client.albums("alphabeticalByName", 500, 0)?;
                    let results = client.search(&artist.name)?;
                    let name = artist.name.trim().to_lowercase();
                    let mut seen = std::collections::HashSet::new();
                    let songs: Vec<Song> = results
                        .song
                        .into_iter()
                        .filter(|song| {
                            song.artist_id.as_deref() == Some(artist.id.as_str())
                                || song.artist.as_deref().map(|a| a.trim().to_lowercase()) == Some(name.clone())
                        })
                        .filter(|song| seen.insert(song.id.clone()))
                        .collect();
                    data.artist_songs = volta_logic::library::sort_artist_songs_by_popularity(songs);
                    data.similar_artists = volta_logic::library::similar_artists_for(&artist, &library);
                    data.albums = artist.album.clone().unwrap_or_default();
                    data.artist = Some(artist);
                }
                Page::Genre => data.songs = client.songs_by_genre(&id, 0, 100)?,
                Page::Playlist => {
                    let playlist = client.playlist(&id)?;
                    data.songs = playlist.entry.clone().unwrap_or_default();
                    data.playlist = Some(playlist);
                }
                _ => {}
            }
            Ok::<_, NavidromeError>(data)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |state, cx| {
                if state.page_generation != generation {
                    return;
                }
                state.page_loading = false;
                state.note_connection(result.as_ref().err(), cx);
                match result {
                    Ok(data) => {
                        state.page = data.into();
                        state.reset_paging();
                        if route_page != Page::Folders && route_page != Page::Search {
                            state.page_memory.shift_remove(&memory_key);
                            state.page_memory.insert(memory_key.clone(), state.page.clone());
                            while state.page_memory.len() > 2 {
                                state.page_memory.shift_remove_index(0);
                            }
                            let json = serde_json::json!({"savedAt": volta_logic::listening_history::now_ms(), "data": &state.page}).to_string();
                            state.storage.session_set(&session_key, &json);
                        }
                        if state.route().page == Page::Folders {
                            state.folder_tree = Some(std::sync::Arc::new(volta_logic::folder_tree::build_folder_tree(&state.page.songs)));
                            if state.folder_tree.as_ref().is_some_and(|t| t.get(&state.folder_path).is_none()) {
                                state.folder_path.clear();
                            }
                        }
                        #[cfg(feature = "shots")]
                        if std::env::var("VOLTA_SHOT_DIALOG").as_deref() == Ok("select") {
                            state.selection.selecting = true;
                            state.selection.selected = [0, 2].into_iter().collect();
                        }
                        #[cfg(feature = "shots")]
                        if std::env::var("VOLTA_SHOT_DIALOG").as_deref() == Ok("details") {
                            if let Some(song) = state.page.songs.first().cloned() {
                                state.dialog = Some(Dialog::SongDetails(song));
                            }
                        }
                    }
                    Err(error) => state.page_error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Folder browser navigation (`goTo` with Back / Forward history).
    pub fn folder_go(&mut self, path: String, cx: &mut Context<Self>) {
        self.folder_history.truncate(self.folder_history_index + 1);
        if self.folder_history.last() != Some(&path) {
            self.folder_history.push(path.clone());
        }
        self.folder_history_index = self.folder_history.len() - 1;
        self.folder_path = path;
        cx.notify();
    }

    pub fn folder_step(&mut self, delta: i32, cx: &mut Context<Self>) {
        let next = self.folder_history_index as i32 + delta;
        if next < 0 || next as usize >= self.folder_history.len() {
            return;
        }
        self.folder_history_index = next as usize;
        self.folder_path = self.folder_history[next as usize].clone();
        cx.notify();
    }

    /// Typing in the search field; results follow after 120 ms of quiet.
    pub fn set_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        if self.search_query == query {
            return;
        }
        self.search_query = query;
        self.search_generation += 1;
        let generation = self.search_generation;
        let timer = cx.background_executor().timer(std::time::Duration::from_millis(120));
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |state, cx| {
                if state.search_generation != generation {
                    return;
                }
                let next = state.search_query.trim().to_string();
                if next != state.search_debounced {
                    state.search_debounced = next;
                    state.search_tab = "all";
                    state.run_search(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn run_search(&mut self, cx: &mut Context<Self>) {
        self.reset_selection();
        let query = self.search_debounced.clone();
        self.page = Default::default();
        self.paging = Default::default();
        if self.is_local() {
            self.page = std::rc::Rc::new(self.local_search(&query));
            self.page_loading = false;
            self.reset_paging();
            cx.notify();
            return;
        }
        self.page_error = None;
        if query.is_empty() {
            self.page_loading = false;
            cx.notify();
            return;
        }
        let Some(client) = self.client() else { return };
        self.page_generation += 1;
        let generation = self.page_generation;
        self.page_loading = true;
        let task = cx.background_spawn(async move { client.search(&query) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |state, cx| {
                if state.page_generation != generation {
                    return;
                }
                state.page_loading = false;
                match result {
                    Ok(results) => {
                        let page = std::rc::Rc::make_mut(&mut state.page);
                        page.artists = results.artist;
                        page.albums = results.album;
                        page.songs = results.song;
                        state.reset_paging();
                    }
                    Err(error) => state.page_error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// `rememberSearch` (on blur / Enter).
    pub fn remember_search(&mut self, cx: &mut Context<Self>) {
        let text = self.search_query.trim().to_string();
        if !text.is_empty() {
            self.record_engagement(volta_logic::interactions::EngagementInput {
                kind: Some(volta_logic::interactions::EngagementKind::Search),
                text: Some(text),
                ..Default::default()
            });
        }
        let next = keys::remember_search(&self.search_history, &self.search_query);
        if next != self.search_history {
            self.search_history = next;
            self.storage.set(keys::SEARCH_HISTORY_KEY, &serde_json::to_string(&self.search_history).unwrap_or_default());
            cx.notify();
        }
    }

    pub fn clear_search_history(&mut self, cx: &mut Context<Self>) {
        self.search_history.clear();
        self.storage.set(keys::SEARCH_HISTORY_KEY, "[]");
        cx.notify();
    }

    fn load_genres(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client() else { return };
        self.genres_loading = true;
        self.genres_error = None;
        let task = cx.background_spawn(async move { client.genres() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |state, cx| {
                state.genres_loading = false;
                match result {
                    Ok(genres) => {
                        // Unique by case-folded name, sorted (`localeCompare`).
                        let mut seen = HashSet::new();
                        let mut unique: Vec<Genre> = genres
                            .into_iter()
                            .filter(|g| !g.value.trim().is_empty())
                            .filter(|g| seen.insert(g.value.trim().to_lowercase()))
                            .collect();
                        unique.sort_by(|a, b| volta_logic::library::locale_compare(&a.value, &b.value));
                        state.genres = unique;
                    }
                    Err(_) => state.genres_error = Some("Genres could not be loaded. Try again.".into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn retry_genres(&mut self, cx: &mut Context<Self>) {
        self.load_genres(cx);
        cx.notify();
    }

    /// The first song's cover for a genre tile, fetched once.
    /// The genre tile background: its cover luminosity-blended over `color`.
    pub fn genre_tile_art(&mut self, genre: &str, color: gpui::Hsla, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        if let Some(image) = self.genre_tiles.get(genre) {
            return Some(image.clone());
        }
        let cover_id = self.genre_cover(genre, cx)?;
        let cover = self.cover(Some(&cover_id), 400, cx)?;
        if !self.genre_tile_pending.insert(genre.to_string()) {
            return None;
        }
        let rgb = color.to_rgb();
        let backdrop = [rgb.r, rgb.g, rgb.b];
        let pixels = crate::covers::to_rgba(&cover);
        let genre = genre.to_string();
        let task = cx.background_spawn(async move { crate::components::genre_art::luminosity_tile(pixels?, backdrop, 400, 225) });
        cx.spawn(async move |this, cx| {
            if let Some(tile) = task.await {
                let _ = this.update(cx, |state, cx| {
                    state.genre_tiles.insert(genre, Arc::new(tile));
                    cx.notify();
                });
            }
        })
        .detach();
        None
    }

    pub fn genre_cover(&mut self, genre: &str, cx: &mut Context<Self>) -> Option<String> {
        if let Some(cover) = self.genre_covers.get(genre) {
            return cover.clone();
        }
        if !self.genre_cover_pending.insert(genre.to_string()) {
            return None;
        }
        let client = self.client()?;
        let name = genre.to_string();
        let task = cx.background_spawn(async move { client.songs_by_genre(&name, 0, 1) });
        let name = genre.to_string();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |state, cx| {
                let cover = result.ok().and_then(|songs| songs.into_iter().next()).and_then(|s| s.cover_art);
                state.genre_covers.insert(name, cover);
                cx.notify();
            });
        })
        .detach();
        None
    }

    /// Plays an album from its first track (the "Play <album>" action).
    pub fn play_album(&mut self, album_id: String, cx: &mut Context<Self>) {
        let player = self.player.clone();
        let Some(task) = self.album_task(album_id, cx) else { return };
        cx.spawn(async move |this, cx| {
            let result = task.await;
            match result {
                Ok(album) => {
                    let _ = this.update(cx, |state, _| {
                        state.engage(volta_logic::interactions::EngagementKind::AlbumPlay, crate::recommend::entity_for_album(&album));
                    });
                    let songs: Vec<Song> = album.song.unwrap_or_default();
                    player.update(cx, |player, cx| player.play_queue(songs, 0, cx));
                }
                Err(error) => {
                    let _ = this.update(cx, |state, cx| {
                        state.notify(error.to_string(), NoticeKind::Error, cx);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// Cover art for `id` at `size`; fetched in the background on first use.
    pub fn cover(&mut self, id: Option<&str>, size: u32, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        let id = id.filter(|id| !id.is_empty())?;
        let key = format!("{id}:{size}");
        if let Some(image) = self.covers.get(&key) {
            return Some(image);
        }
        let client = self.client()?;
        if !self.covers.begin(&key) {
            return None;
        }
        let id = id.to_string();
        // Only `size`-pixel covers ever reach memory (local artwork is often
        // 1500px+); decoding runs on the bounded cover pool.
        let max_side = size.clamp(64, 1200);
        let task: gpui::Task<Option<RenderImage>> = if let Some(path) = id.strip_prefix("local-file:") {
            let path = path.to_string();
            let thumbs = artwork_cache_dir();
            let decoded = crate::covers::run(move || crate::covers::load_local(&path, max_side, thumbs.as_deref()));
            cx.background_spawn(async move { decoded.await.ok().flatten() })
        } else {
            // Server artwork is kept on disk (the web's Cache Storage), keyed
            // by server, cover id and size.
            let disk = artwork_cache_dir().map(|dir| {
                let fingerprint = format!("{}\u{1f}{id}\u{1f}{size}", client.server);
                let hash = fingerprint.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100000001b3));
                dir.join(format!("{hash:016x}"))
            });
            cx.background_spawn(async move {
                let bytes = match disk.as_ref().and_then(|path| std::fs::read(path).ok()) {
                    Some(bytes) => bytes,
                    None => {
                        let bytes = client.cover_bytes(&id, Some(size)).ok()?;
                        if let Some(path) = disk.as_ref() {
                            let _ = std::fs::create_dir_all(path.parent().unwrap_or(path));
                            let _ = std::fs::write(path, &bytes);
                        }
                        bytes
                    }
                };
                crate::covers::run(move || {
                    // SVG covers are rasterized here, with system fonts (svg_raster).
                    let bytes = if crate::components::svg_raster::is_svg(&bytes) {
                        crate::components::svg_raster::render_png(&bytes, max_side).unwrap_or(bytes)
                    } else {
                        bytes
                    };
                    crate::covers::decode(&bytes, max_side)
                })
                .await
                .ok()
                .flatten()
            })
        };
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |state, cx| match result {
                Some(image) => {
                    if state.covers.insert(key, Arc::new(image)) {
                        let weak = cx.weak_entity();
                        cx.defer(move |cx| {
                            let _ = weak.update(cx, |s, cx| s.covers.evict(cx));
                        });
                    }
                    state.notify_covers(cx);
                }
                None => state.covers.failed(&key),
            });
        })
        .detach();
        None
    }

    /// Covers arriving in a burst (a grid opening) re-render the window at
    /// most every 50 ms instead of once per cover.
    fn notify_covers(&mut self, cx: &mut Context<Self>) {
        if self.cover_notify_pending {
            return;
        }
        self.cover_notify_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
            let _ = this.update(cx, |state, cx| {
                state.cover_notify_pending = false;
                cx.notify();
            });
        })
        .detach();
    }

    pub fn account_label(&self) -> String {
        self.session
            .as_ref()
            .map(|s| s.client.username.clone())
            .unwrap_or_default()
    }
}

pub fn read_login_prefill(storage: &Storage) -> (String, String, bool) {
    let (draft, write) = keys::read_login_draft(&storage.session_get(keys::LOGIN_DRAFT_KEY));
    match write {
        Some(value) => storage.session_set(keys::LOGIN_DRAFT_KEY, &value),
        None => storage.session_remove(keys::LOGIN_DRAFT_KEY),
    }
    let remembered = keys::read_stored_credentials(&storage.get(keys::REMEMBERED_CREDENTIALS_KEY));
    let pick = |draft: String, key: &str, fallback: Option<String>| {
        if !draft.is_empty() {
            return draft;
        }
        let stored = storage.get(key);
        if !stored.is_empty() {
            return stored;
        }
        fallback.unwrap_or_default()
    };
    let server = pick(draft.server, keys::SERVER_KEY, remembered.as_ref().map(|c| c.server.clone()));
    let username = pick(draft.username, keys::USERNAME_KEY, remembered.map(|c| c.username));
    let remember = storage.get(keys::REMEMBER_ME_KEY) == "true";
    (server, username, remember)
}

pub fn save_login_draft(storage: &Storage, server: &str, username: &str) {
    let draft = keys::LoginDraft {
        server: server.to_string(),
        username: username.to_string(),
    };
    storage.session_set(keys::LOGIN_DRAFT_KEY, &serde_json::to_string(&draft).unwrap_or_default());
}

pub struct GlobalStorage(pub Storage);

/// Whether music is playing, for the close confirmation.
pub struct GlobalPlaying(pub bool);
impl gpui::Global for GlobalPlaying {}
impl gpui::Global for GlobalStorage {}
