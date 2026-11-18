//! Local music ("This device"): choosing and scanning a folder, opening it as
//! the library (`openLocalLibrary` with `localClient`), local page data
//! (`localPageData`), local favorites, metadata edits, and restore on launch.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{AppContext, Context};
use volta_logic::local_music::{LocalLibrary, MetadataPatch, apply_metadata_edits, local_album_order, similar_albums_for, similar_artists_for};
use volta_logic::model::{AlbumRecord, Artist, Song};
use volta_logic::storage as keys;
use volta_services::Navidrome;
use volta_services::local::{LOCAL_DIRECTORY_KEY, LOCAL_METADATA_KEY, LOCAL_OVERRIDES_KEY, read_cache, read_overrides, scan_directory};

use crate::app_state::{AppState, Page, PageData, Route, Session};
use crate::components::notifications::NoticeKind;

/// The folder chosen and its scan.
#[derive(Default)]
pub struct LocalState {
    pub library: Option<Arc<LocalLibrary>>,
    pub busy: bool,
    pub error: Option<String>,
    /// `sourceMode === "local"`.
    pub active: bool,
    /// The details dialog shows the metadata editor.
    pub editing: bool,
}

fn artwork_dir() -> PathBuf {
    volta_services::storage::data_dir().unwrap_or_else(std::env::temp_dir).join("local-artwork")
}

impl AppState {
    pub fn is_local(&self) -> bool {
        self.local.active
    }

    /// `chooseLocalDirectory`: the OS folder picker, then a scan.
    pub fn choose_local_folder(&mut self, cx: &mut Context<Self>) {
        if self.local.busy {
            return;
        }
        let pick = rfd::AsyncFileDialog::new().set_title("Choose your music folder").pick_folder();
        cx.spawn(async move |this, cx| {
            if let Some(folder) = pick.await {
                let path = folder.path().to_path_buf();
                let _ = this.update(cx, |state, cx| state.scan_local(path, false, cx));
            }
        })
        .detach();
    }

    /// Scans `root` into a library; with `open`, opens it when done.
    pub fn scan_local(&mut self, root: PathBuf, open: bool, cx: &mut Context<Self>) {
        self.local.busy = true;
        self.local.error = None;
        cx.notify();
        let loading = std::time::Instant::now();
        let raw_cache = self.storage.get(LOCAL_METADATA_KEY);
        let cache = read_cache(&raw_cache);
        let overrides = read_overrides(&self.storage.get(LOCAL_OVERRIDES_KEY));
        log::info!(target: crate::logs::PERF, "local metadata cache read: {} entries, {} KB, {:.0} ms", cache.len(), raw_cache.len() / 1024, loading.elapsed().as_secs_f64() * 1000.0);
        log::info!("scanning local folder {}", root.display());
        let scan_started = std::time::Instant::now();
        let task = cx.background_spawn(async move {
            if !root.is_dir() {
                return Err(root);
            }
            Ok((root.clone(), scan_directory(&root, &cache, &overrides, &artwork_dir())))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |state, cx| {
                state.local.busy = false;
                match result {
                    Ok((root, scan)) => {
                        state.storage.set(LOCAL_DIRECTORY_KEY, &root.to_string_lossy());
                        let saving = std::time::Instant::now();
                        let json = serde_json::to_string(&scan.cache).unwrap_or_default();
                        state.storage.set(LOCAL_METADATA_KEY, &json);
                        log::info!(target: crate::logs::PERF, "local metadata cache saved: {} entries, {} KB, {:.0} ms", scan.cache.len(), json.len() / 1024, saving.elapsed().as_secs_f64() * 1000.0);
                        let library = scan.library;
                        log::info!(
                            "local library ready: {} tracks, {} albums, {} artists",
                            library.tracks.len(),
                            library.albums.len(),
                            library.artists.len()
                        );
                        crate::logs::timed("local library load (scan + save)", scan_started);
                        crate::logs::mark("local library ready");
                        let message = if library.tracks.is_empty() {
                            format!("No supported audio files found in {}.", library.directory_name)
                        } else {
                            format!("Loaded {} tracks from {}.", library.tracks.len(), library.directory_name)
                        };
                        let has_tracks = !library.tracks.is_empty();
                        state.local.library = Some(Arc::new(library));
                        if !open {
                            state.notify(message, NoticeKind::Info, cx);
                        }
                        if open && has_tracks {
                            state.open_local_library(cx);
                        } else if open {
                            state.auth_ready = true;
                        }
                    }
                    Err(_) => {
                        log::warn!("local folder could not be read");
                        state.local.error = Some("That folder could not be read. Choose it again.".into());
                        state.auth_ready = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// `openLocalLibrary`.
    pub fn open_local_library(&mut self, cx: &mut Context<Self>) {
        let Some(library) = self.local.library.clone().filter(|l| !l.tracks.is_empty()) else { return };
        let client = Arc::new(Navidrome::offline("This device", "Local Library"));
        let volume = keys::read_stored_volume(&self.storage.get(&keys::local_volume_key(&library.directory_name))).unwrap_or_default();
        self.player.update(cx, |p, cx| {
            p.stop(cx);
            p.set_client(None);
            p.restore_volume(volume, cx);
        });
        self.storage.set(keys::LOCAL_SOURCE_MODE_KEY, "local");
        self.local.active = true;
        self.auth_ready = true;
        self.session = Some(Session { client });
        self.connection = Default::default();
        self.page = Default::default();
        self.page_error = None;
        self.playlists.clear();
        self.panel = None;
        self.full_player_open = false;
        self.filter.clear();
        self.history = vec![Route::page(Page::Home)];
        self.load_listening_history();
        self.load_learning();
        self.sync_transition(cx);
        self.last_snapshot.clear();
        self.restore_playback(cx);
        self.load_home(cx);
        #[cfg(feature = "shots")]
        if let Some(route) = crate::shot::initial_route() {
            self.history.push(route);
            self.load_page(cx);
            if std::env::var("VOLTA_SHOT_FULL").is_ok() {
                self.full_player_open = true;
                if let Some(id) = self.route().id.clone() {
                    self.play_album(id, cx);
                }
            }
            if std::env::var("VOLTA_SHOT_PANEL").is_ok() {
                self.panel = Some(crate::app_state::Panel::Queue);
                if let Some(id) = self.route().id.clone() {
                    self.play_album(id, cx);
                }
            }
            if std::env::var("VOLTA_SHOT_DIALOG").as_deref() == Ok("edit") {
                if let Some(song) = self.page.songs.first().cloned() {
                    self.dialog = Some(crate::app_state::Dialog::SongDetails(song));
                    self.local.editing = true;
                }
            }
        }
        cx.notify();
    }

    /// On launch: reopen the remembered folder when the last source was local.
    pub fn restore_local(&mut self, cx: &mut Context<Self>) -> bool {
        if self.storage.get(keys::LOCAL_SOURCE_MODE_KEY) != "local" {
            return false;
        }
        let dir = self.storage.get(LOCAL_DIRECTORY_KEY);
        if dir.is_empty() {
            return false;
        }
        self.scan_local(PathBuf::from(dir), true, cx);
        true
    }

    /// Scans the remembered folder for the sign-in screen (`restoreLocalMusicDirectory`).
    pub fn prepare_local_folder(&mut self, cx: &mut Context<Self>) {
        let dir = self.storage.get(LOCAL_DIRECTORY_KEY);
        if !dir.is_empty() && self.local.library.is_none() && !self.local.busy {
            self.scan_local(PathBuf::from(dir), false, cx);
        }
    }

    /// `localPageData`.
    pub fn local_page_data(&self, route: &Route) -> PageData {
        let Some(library) = self.local.library.as_ref() else { return PageData::default() };
        let songs = || library.tracks.clone();
        match route.page {
            // List pages get albums and artists without their nested song
            // and album lists: cards copy their record into click handlers on
            // every frame, and actions look the full record up by id.
            Page::Recent => PageData { albums: shallow_albums(&library.recent_albums), ..Default::default() },
            Page::Albums => PageData { albums: shallow_albums(&local_album_order(library, self.album_sort)), ..Default::default() },
            Page::Artists => PageData { artists: library.artists.iter().map(shallow_artist).collect(), ..Default::default() },
            Page::Songs | Page::Folders | Page::Favorites => PageData { songs: songs(), ..Default::default() },
            Page::Album => {
                let album = library.albums.iter().find(|a| Some(&a.id) == route.id.as_ref()).cloned();
                PageData {
                    songs: album.as_ref().and_then(|a| a.song.clone()).unwrap_or_default(),
                    similar_albums: album.as_ref().map(|a| similar_albums_for(a, &library.albums)).unwrap_or_default(),
                    album,
                    ..Default::default()
                }
            }
            Page::Artist => {
                let artist = library.artists.iter().find(|a| Some(&a.id) == route.id.as_ref()).cloned();
                let mut seen = std::collections::HashSet::new();
                let artist_songs: Vec<Song> = artist
                    .iter()
                    .flat_map(|a| a.album.iter().flatten())
                    .flat_map(|a| a.song.iter().flatten())
                    .filter(|s| seen.insert(s.id.clone()))
                    .cloned()
                    .collect();
                PageData {
                    albums: artist.as_ref().and_then(|a| a.album.clone()).unwrap_or_default(),
                    similar_artists: artist.as_ref().map(|a| similar_artists_for(a, &library.albums)).unwrap_or_default(),
                    artist,
                    artist_songs,
                    ..Default::default()
                }
            }
            _ => PageData::default(),
        }
    }

    /// Local search over titles, artists and albums.
    pub fn local_search(&self, query: &str) -> PageData {
        let Some(library) = self.local.library.as_ref() else { return PageData::default() };
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return PageData::default();
        }
        PageData {
            songs: library
                .tracks
                .iter()
                .filter(|s| {
                    format!("{} {} {}", s.title, s.artist.as_deref().unwrap_or(""), s.album.as_deref().unwrap_or("")).to_lowercase().contains(&needle)
                })
                .cloned()
                .collect(),
            albums: library
                .albums
                .iter()
                .filter(|a| {
                    format!("{} {}", volta_logic::format::album_name(a), a.artist.as_deref().unwrap_or("")).to_lowercase().contains(&needle)
                })
                .cloned()
                .collect(),
            artists: library.artists.iter().filter(|a| a.name.to_lowercase().contains(&needle)).cloned().collect(),
            ..Default::default()
        }
    }

    /// Local Home: 8 shuffled albums and the 12 newest.
    pub fn local_home(&mut self) {
        let Some(library) = self.local.library.clone() else { return };
        use rand::seq::SliceRandom;
        let mut albums: Vec<&AlbumRecord> = library.albums.iter().collect();
        albums.shuffle(&mut rand::rng());
        albums.truncate(8);
        self.home.albums = albums.into_iter().map(shallow_album).collect();
        self.home.recent = library.recent_albums.iter().take(12).map(shallow_album).collect();
        self.home.recently_played.clear();
        self.home.frequent.clear();
        self.home.recommendation_candidates = library.albums.clone();
        self.home.recommendation_pool.clear();
        self.home.favorite_album_ids.clear();
        self.home.loading = false;
        self.home.error = None;
        self.home.generation += 1;
    }

    /// Favorite for local items lives in storage (`writeLocalFavoriteSet`).
    pub fn toggle_local_favorite(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        let (key, prefix) = if id.starts_with("local-album:") {
            (keys::LOCAL_ALBUM_FAVORITES_KEY, "local-album:")
        } else if id.starts_with("local-artist:") {
            (keys::LOCAL_ARTIST_FAVORITES_KEY, "local-artist:")
        } else if id.starts_with("local:") {
            (keys::LOCAL_FAVORITES_KEY, "local:")
        } else {
            return false;
        };
        let mut map: HashMap<String, bool> = serde_json::from_str(&self.storage.get(key)).unwrap_or_default();
        map.retain(|k, v| k.starts_with(prefix) && *v);
        let active = !map.contains_key(id);
        if active {
            map.insert(id.to_string(), true);
        } else {
            map.remove(id);
        }
        self.storage.set(key, &serde_json::to_string(&map).unwrap_or_default());
        self.favorite_overrides.insert(id.to_string(), active);
        cx.notify();
        true
    }

    /// Whether a local item is a favorite.
    pub fn is_local_favorite(&self, id: &str) -> Option<bool> {
        let key = if id.starts_with("local-album:") {
            keys::LOCAL_ALBUM_FAVORITES_KEY
        } else if id.starts_with("local-artist:") {
            keys::LOCAL_ARTIST_FAVORITES_KEY
        } else if id.starts_with("local:") {
            keys::LOCAL_FAVORITES_KEY
        } else {
            return None;
        };
        let map: HashMap<String, bool> = serde_json::from_str(&self.storage.get(key)).unwrap_or_default();
        Some(map.get(id).copied().unwrap_or(false))
    }

    /// `persistLocalMetadata`: save edits, re-index, refresh the page.
    pub fn save_local_metadata(&mut self, edits: Vec<(String, MetadataPatch)>, cx: &mut Context<Self>) {
        let Some(library) = self.local.library.clone() else { return };
        if edits.is_empty() {
            self.notify("This local file has no editable path.", NoticeKind::Error, cx);
            return;
        }
        let mut overrides = read_overrides(&self.storage.get(LOCAL_OVERRIDES_KEY));
        for (path, patch) in edits {
            overrides.entry(path).or_default().merge(&patch);
        }
        self.storage.set(LOCAL_OVERRIDES_KEY, &serde_json::to_string(&overrides).unwrap_or_default());
        let updated = apply_metadata_edits(&library, &overrides);
        let by_id: HashMap<String, Song> = updated.tracks.iter().map(|s| (s.id.clone(), s.clone())).collect();
        self.player.update(cx, |p, cx| p.update_songs(&by_id, cx));
        self.local.library = Some(Arc::new(updated));
        self.local.editing = false;
        self.load_page(cx);
        self.notify("Local metadata saved on this device.", NoticeKind::Success, cx);
    }

    /// `saveLocalSongMetadata`: the dialog and an open album follow the edit.
    pub fn save_local_song(&mut self, song: &Song, patch: MetadataPatch, cx: &mut Context<Self>) {
        let Some(path) = song.local_path.clone() else {
            self.notify("This local file has no editable path.", NoticeKind::Error, cx);
            return;
        };
        self.save_local_metadata(vec![(path, patch)], cx);
        let Some(saved) = self.local.library.as_ref().and_then(|l| l.tracks.iter().find(|t| t.id == song.id).cloned()) else { return };
        if self.route().page == Page::Album && self.route().id == song.album_id && saved.album_id != song.album_id {
            let route = Route { page: Page::Album, id: saved.album_id.clone(), title: saved.album.clone() };
            self.navigate(route, cx);
        }
        self.dialog = Some(crate::app_state::Dialog::SongDetails(saved));
    }

    /// `saveLocalAlbumMetadata`: the patch applies to every track of the album.
    pub fn save_local_album(&mut self, album: &volta_logic::model::AlbumRecord, patch: MetadataPatch, cx: &mut Context<Self>) {
        let Some(library) = self.local.library.clone() else { return };
        let songs: Vec<&Song> = library.tracks.iter().filter(|s| s.album_id.as_deref() == Some(album.id.as_str())).collect();
        let edits: Vec<(String, MetadataPatch)> = songs.iter().filter_map(|s| Some((s.local_path.clone()?, patch.clone()))).collect();
        let first = songs.first().map(|s| s.id.clone());
        self.save_local_metadata(edits, cx);
        let Some(updated) = self.local.library.clone() else { return };
        let saved = first
            .and_then(|id| updated.tracks.iter().find(|t| t.id == id).and_then(|t| t.album_id.clone()))
            .and_then(|id| updated.albums.iter().find(|a| a.id == id).cloned());
        let Some(saved) = saved else { return };
        if self.route().page == Page::Album && self.route().id.as_deref() == Some(album.id.as_str()) && saved.id != album.id {
            let route = Route { page: Page::Album, id: Some(saved.id.clone()), title: saved.name.clone() };
            self.navigate(route, cx);
        }
        self.dialog = Some(crate::app_state::Dialog::AlbumDetails(saved));
    }

    /// Leaving local mode (logout).
    pub fn close_local(&mut self) {
        self.local.active = false;
        self.storage.remove(keys::LOCAL_SOURCE_MODE_KEY);
    }
}

/// An album record without its song list (see `local_page_data`). A
/// one-time copy per navigation; the per-frame copies are what it saves.
pub(crate) fn shallow_album(album: &AlbumRecord) -> AlbumRecord {
    AlbumRecord { song: None, ..album.clone() }
}

fn shallow_albums(albums: &[AlbumRecord]) -> Vec<AlbumRecord> {
    albums.iter().map(shallow_album).collect()
}

/// An artist without its nested albums (and their songs).
fn shallow_artist(artist: &Artist) -> Artist {
    Artist { album: None, ..artist.clone() }
}
