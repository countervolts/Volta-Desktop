//! Subsonic/Navidrome client. Port of the `Navidrome` class in
//! `src/lib/navidrome.ts`.
//!
//! Calls are blocking; the UI runs them on gpui's background executor.
//! URLs carry the token and salt, so they are never logged. Use
//! [`volta_audio::redact_url`] for anything that has to be printed.

use std::collections::HashMap;
use std::time::Duration;

use md5::{Digest, Md5};
use parking_lot::Mutex;
use reqwest::blocking::Client;
use serde::de::DeserializeOwned;
use serde_json::Value;
use volta_logic::model::{AlbumRecord, Artist, Genre, Playlist, Song};
use volta_logic::lyrics::{Lyrics, StructuredLyrics, lyrics_from_structured, parse_plain_lyrics};
use volta_logic::server::normalize_server;
use volta_logic::storage::AuthMaterial;

/// The Subsonic `c=` client name. The web sends `VoltaWeb`.
pub const CLIENT_NAME: &str = "VoltaDesktop";
pub const API_VERSION: &str = "1.16.1";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, thiserror::Error)]
pub enum NavidromeError {
    /// Could not reach the host at all (the web's `TypeError` / timeout path).
    #[error("{0}")]
    Connection(String),
    #[error("{message}")]
    Server {
        message: String,
        status: Option<u16>,
        code: Option<i64>,
    },
    #[error("{0}")]
    Other(String),
}

impl NavidromeError {
    pub fn is_connection_failure(&self) -> bool {
        matches!(self, Self::Connection(_))
    }
}

pub type Result<T> = std::result::Result<T, NavidromeError>;

pub struct Credentials {
    pub server: String,
    pub username: String,
    pub password: String,
    pub auth: Option<AuthMaterial>,
}

/// 16 random bytes as lowercase hex, like the web's salt.
pub fn random_salt() -> String {
    let mut bytes = [0u8; 16];
    rand::fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `md5(password + salt)`
pub fn auth_token(password: &str, salt: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(password.as_bytes());
    hasher.update(salt.as_bytes());
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub struct Navidrome {
    pub server: String,
    pub username: String,
    pub auth: AuthMaterial,
    http: Client,
    covers: Mutex<HashMap<String, String>>,
}

impl std::fmt::Debug for Navidrome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Navidrome")
            .field("server", &self.server)
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

fn redact(error: &reqwest::Error) -> String {
    volta_audio::redact_error(error)
}

impl Navidrome {
    pub fn new(credentials: Credentials) -> Result<Self> {
        let server = normalize_server(&credentials.server).map_err(|e| NavidromeError::Other(e.to_string()))?;
        let auth = match credentials.auth {
            Some(auth) if !auth.token.is_empty() => auth,
            other => {
                let salt = other.map(|a| a.salt).filter(|s| !s.is_empty()).unwrap_or_else(random_salt);
                let token = auth_token(&credentials.password, &salt);
                AuthMaterial { salt, token }
            }
        };
        let http = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("Volta/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| NavidromeError::Other(e.to_string()))?;
        Ok(Self {
            server,
            username: credentials.username.trim().to_string(),
            auth,
            http,
            covers: Mutex::new(HashMap::new()),
        })
    }

    /// The `localClient` identity for "This device": no server; every
    /// request fails as a connection error, which local mode never makes.
    pub fn offline(label: &str, username: &str) -> Self {
        Self {
            server: label.to_string(),
            username: username.to_string(),
            auth: AuthMaterial { salt: String::new(), token: String::new() },
            http: Client::new(),
            covers: Mutex::new(HashMap::new()),
        }
    }

    fn url(&self, endpoint: &str, query: &[(&str, String)]) -> String {
        let mut url = url::Url::parse(&format!("{}/rest/{endpoint}.view", self.server))
            .unwrap_or_else(|_| url::Url::parse("http://invalid/").unwrap());
        {
            let mut pairs = url.query_pairs_mut();
            pairs
                .append_pair("u", &self.username)
                .append_pair("t", &self.auth.token)
                .append_pair("s", &self.auth.salt)
                .append_pair("v", API_VERSION)
                .append_pair("c", CLIENT_NAME)
                .append_pair("f", "json");
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        url.to_string()
    }

    /// The offline local-library identity.
    pub fn is_offline(&self) -> bool {
        self.auth.token.is_empty() && self.auth.salt.is_empty()
    }

    fn request<T: DeserializeOwned>(&self, endpoint: &str, query: &[(&str, String)]) -> Result<T> {
        let started = std::time::Instant::now();
        let result = self.request_inner(endpoint, query);
        if !self.is_offline() {
            crate::metrics::record_request(endpoint, started.elapsed(), result.is_ok());
        }
        if let Err(error) = &result {
            log::warn!("request {endpoint} failed: {error}");
        }
        result
    }

    fn request_inner<T: DeserializeOwned>(&self, endpoint: &str, query: &[(&str, String)]) -> Result<T> {
        if self.is_offline() {
            return Err(NavidromeError::Other("Not available for music on this device.".into()));
        }
        let response = self.http.get(self.url(endpoint, query)).send().map_err(|error| {
            if error.is_timeout() {
                NavidromeError::Connection("Navidrome took too long to respond. Try again.".into())
            } else if error.is_connect() || error.is_request() {
                NavidromeError::Connection(redact(&error))
            } else {
                NavidromeError::Other(redact(&error))
            }
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(NavidromeError::Server {
                message: format!("Navidrome returned HTTP {}.", status.as_u16()),
                status: Some(status.as_u16()),
                code: None,
            });
        }
        let payload: Value = response.json().map_err(|_| {
            NavidromeError::Other("This address did not return a Navidrome response. Check the server URL.".into())
        })?;
        let body = payload.get("subsonic-response").cloned().unwrap_or(Value::Null);
        if body.get("status").and_then(Value::as_str) != Some("ok") {
            let error = body.get("error");
            return Err(NavidromeError::Server {
                message: error
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .filter(|m| !m.is_empty())
                    .unwrap_or("Navidrome did not accept this request.")
                    .to_string(),
                status: None,
                code: error.and_then(|e| e.get("code")).and_then(Value::as_i64),
            });
        }
        serde_json::from_value(body).map_err(|e| NavidromeError::Other(format!("Unexpected response: {e}")))
    }

    fn pick<T: DeserializeOwned + Default>(value: &Value, path: &[&str]) -> T {
        let mut current = value;
        for key in path {
            match current.get(key) {
                Some(next) => current = next,
                None => return T::default(),
            }
        }
        serde_json::from_value(current.clone()).unwrap_or_default()
    }

    pub fn ping(&self) -> Result<()> {
        self.request::<Value>("ping", &[]).map(|_| ())
    }

    pub fn albums(&self, kind: &str, size: u32, offset: u32) -> Result<Vec<AlbumRecord>> {
        let data: Value = self.request(
            "getAlbumList2",
            &[("type", kind.into()), ("size", size.to_string()), ("offset", offset.to_string())],
        )?;
        Ok(Self::pick(&data, &["albumList2", "album"]))
    }

    pub fn random_songs(&self, size: u32) -> Result<Vec<Song>> {
        let data: Value = self.request("getRandomSongs", &[("size", size.to_string())])?;
        Ok(Self::pick(&data, &["randomSongs", "song"]))
    }

    pub fn playlists(&self) -> Result<Vec<Playlist>> {
        let data: Value = self.request("getPlaylists", &[])?;
        Ok(Self::pick(&data, &["playlists", "playlist"]))
    }

    pub fn album(&self, id: &str) -> Result<AlbumRecord> {
        let data: Value = self.request("getAlbum", &[("id", id.into())])?;
        let mut album: AlbumRecord = data
            .get("album")
            .and_then(|a| serde_json::from_value(a.clone()).ok())
            .ok_or_else(|| NavidromeError::Other("This album is no longer available.".into()))?;
        album.song.get_or_insert_with(Vec::new);
        Ok(album)
    }

    /// `getPlaylist`; the entry list is always present.
    pub fn playlist(&self, id: &str) -> Result<Playlist> {
        let data: Value = self.request("getPlaylist", &[("id", id.into())])?;
        let mut playlist: Playlist = data
            .get("playlist")
            .and_then(|p| serde_json::from_value(p.clone()).ok())
            .ok_or_else(|| NavidromeError::Other("This playlist is no longer available.".into()))?;
        playlist.entry.get_or_insert_with(Vec::new);
        Ok(playlist)
    }

    /// `updatePlaylist`: only the given fields change.
    pub fn update_playlist(&self, id: &str, changes: &PlaylistChanges) -> Result<()> {
        let mut query: Vec<(&str, String)> = vec![("playlistId", id.into())];
        if let Some(name) = &changes.name {
            query.push(("name", name.clone()));
        }
        if let Some(comment) = &changes.comment {
            query.push(("comment", comment.clone()));
        }
        if let Some(public) = changes.public {
            query.push(("public", public.to_string()));
        }
        for song in &changes.song_ids_to_add {
            query.push(("songIdToAdd", song.clone()));
        }
        for index in &changes.song_indexes_to_remove {
            query.push(("songIndexToRemove", index.to_string()));
        }
        self.request::<Value>("updatePlaylist", &query).map(|_| ())
    }

    /// `createPlaylist(name, songIds)`.
    pub fn create_playlist(&self, name: &str, song_ids: &[String]) -> Result<Option<Playlist>> {
        let mut query: Vec<(&str, String)> = vec![("name", name.into())];
        for song in song_ids {
            query.push(("songId", song.clone()));
        }
        let data: Value = self.request("createPlaylist", &query)?;
        Ok(data.get("playlist").and_then(|p| serde_json::from_value(p.clone()).ok()))
    }

    pub fn delete_playlist(&self, id: &str) -> Result<()> {
        self.request::<Value>("deletePlaylist", &[("id", id.into())]).map(|_| ())
    }

    pub fn song(&self, id: &str) -> Result<Song> {
        let data: Value = self.request("getSong", &[("id", id.into())])?;
        data.get("song")
            .and_then(|s| serde_json::from_value(s.clone()).ok())
            .ok_or_else(|| NavidromeError::Other("This song is no longer available.".into()))
    }

    /// `getArtists`, flattened across the index letters.
    pub fn artists(&self) -> Result<Vec<Artist>> {
        let data: Value = self.request("getArtists", &[])?;
        let index: Vec<Value> = Self::pick(&data, &["artists", "index"]);
        Ok(index
            .iter()
            .flat_map(|letter| Self::pick::<Vec<Artist>>(letter, &["artist"]))
            .collect())
    }

    pub fn artist(&self, id: &str) -> Result<Artist> {
        let data: Value = self.request("getArtist", &[("id", id.into())])?;
        let mut artist: Artist = data
            .get("artist")
            .filter(|a| !a.is_null())
            .and_then(|a| serde_json::from_value(a.clone()).ok())
            .ok_or_else(|| NavidromeError::Other("This artist is no longer available.".into()))?;
        artist.album.get_or_insert_with(Vec::new);
        Ok(artist)
    }

    /// `search3` with the web's counts (artists 30, albums 50, songs 100).
    pub fn search(&self, query: &str) -> Result<SearchResults> {
        self.search_page(query, 0, 0, 0)
    }

    /// `search(query, {artistOffset, albumOffset, songOffset})`: pages of
    /// 30 artists, 50 albums and 100 songs.
    pub fn search_page(&self, query: &str, artist_offset: usize, album_offset: usize, song_offset: usize) -> Result<SearchResults> {
        let data: Value = self.request(
            "search3",
            &[
                ("query", query.into()),
                ("artistCount", "30".into()),
                ("artistOffset", artist_offset.to_string()),
                ("albumCount", "50".into()),
                ("albumOffset", album_offset.to_string()),
                ("songCount", "100".into()),
                ("songOffset", song_offset.to_string()),
            ],
        )?;
        Ok(SearchResults {
            artist: Self::pick(&data, &["searchResult3", "artist"]),
            album: Self::pick(&data, &["searchResult3", "album"]),
            song: Self::pick(&data, &["searchResult3", "song"]),
        })
    }

    /// Paginated library songs: `search3` with an empty query (OpenSubsonic).
    pub fn songs(&self, offset: u32, size: u32) -> Result<Vec<Song>> {
        let data: Value = self.request(
            "search3",
            &[
                ("query", String::new()),
                ("artistCount", "0".into()),
                ("albumCount", "0".into()),
                ("songCount", size.to_string()),
                ("songOffset", offset.to_string()),
            ],
        )?;
        Ok(Self::pick(&data, &["searchResult3", "song"]))
    }

    /// `getGenres`.
    pub fn genres(&self) -> Result<Vec<Genre>> {
        let data: Value = self.request("getGenres", &[])?;
        Ok(Self::pick(&data, &["genres", "genre"]))
    }

    /// `getSongsByGenre`.
    pub fn songs_by_genre(&self, genre: &str, offset: u32, count: u32) -> Result<Vec<Song>> {
        let data: Value = self.request(
            "getSongsByGenre",
            &[("genre", genre.into()), ("offset", offset.to_string()), ("count", count.to_string())],
        )?;
        Ok(Self::pick(&data, &["songsByGenre", "song"]))
    }

    /// `lyrics(song)`: OpenSubsonic structured lyrics (enhanced, then plain),
    /// then legacy `getLyrics`. Unsupported endpoints and unknown songs give
    /// `None` rather than an error, so an external fallback can run.
    pub fn lyrics(&self, song: &Song) -> Result<Option<Lyrics>> {
        for enhanced in [true, false] {
            let mut query = vec![("id", song.id.clone())];
            if enhanced {
                query.push(("enhanced", "true".into()));
            }
            match self.request::<Value>("getLyricsBySongId", &query) {
                Ok(data) => {
                    let entries: Vec<StructuredLyrics> = Self::pick(&data, &["lyricsList", "structuredLyrics"]);
                    return Ok(lyrics_from_structured(&entries).filter(|l| !l.lines.is_empty()).or_else(|| {
                        self.legacy_lyrics(song).ok().flatten()
                    }));
                }
                Err(NavidromeError::Connection(_)) => return Ok(None),
                Err(NavidromeError::Server { status, code, .. })
                    if matches!(status, Some(404 | 405 | 501)) || matches!(code, Some(0 | 20 | 70)) =>
                {
                    break;
                }
                Err(NavidromeError::Server { .. }) if enhanced => continue,
                Err(NavidromeError::Server { .. }) => break,
                Err(error) => return Err(error),
            }
        }
        self.legacy_lyrics(song)
    }

    fn legacy_lyrics(&self, song: &Song) -> Result<Option<Lyrics>> {
        match self.request::<Value>(
            "getLyrics",
            &[("artist", song.artist.clone().unwrap_or_default()), ("title", song.title.clone())],
        ) {
            Ok(data) => {
                let value: String = Self::pick(&data, &["lyrics", "value"]);
                Ok(parse_plain_lyrics(&value))
            }
            Err(NavidromeError::Connection(_) | NavidromeError::Server { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// `allSongs`: every song, page by page, de-duplicated by id. Stops on a
    /// short page or a page that adds nothing (a server ignoring offsets).
    /// `allAlbums`: every album, paged by `alphabeticalByName` (500 a page),
    /// deduped by id; stops when a page adds nothing new.
    pub fn all_albums(&self) -> Result<Vec<AlbumRecord>> {
        let started = std::time::Instant::now();
        let page_size = 500u32;
        let mut seen = std::collections::HashSet::new();
        let mut albums = Vec::new();
        let mut offset = 0u32;
        loop {
            let page = self.albums("alphabeticalByName", page_size, offset)?;
            if page.is_empty() {
                break;
            }
            let length = page.len() as u32;
            let mut added = 0;
            for album in page {
                let id = album.id.trim().to_string();
                if id.is_empty() || !seen.insert(id) {
                    continue;
                }
                albums.push(album);
                added += 1;
            }
            if added == 0 || length < page_size {
                break;
            }
            offset += page_size;
        }
        log::info!(target: "perf", "fetched all albums: {} albums, {:.0} ms", albums.len(), started.elapsed().as_secs_f64() * 1000.0);
        Ok(albums)
    }

    pub fn all_songs(&self, page_size: u32, max: Option<usize>) -> Result<Vec<Song>> {
        let started = std::time::Instant::now();
        let mut pages = 0u32;
        let mut seen = std::collections::HashSet::new();
        let mut songs = Vec::new();
        let mut offset = 0u32;
        while max.is_none_or(|m| songs.len() < m) {
            let page = self.songs(offset, page_size)?;
            pages += 1;
            if page.is_empty() {
                break;
            }
            let length = page.len() as u32;
            let mut added = 0;
            for song in page {
                let id = song.id.trim().to_string();
                if id.is_empty() || !seen.insert(id) {
                    continue;
                }
                songs.push(song);
                added += 1;
            }
            if added == 0 {
                break;
            }
            offset += length;
            if length < page_size {
                break;
            }
        }
        if let Some(max) = max {
            songs.truncate(max);
        }
        log::info!(target: "perf", "fetched all songs: {} songs in {pages} requests of {page_size}, {:.0} ms", songs.len(), started.elapsed().as_secs_f64() * 1000.0);
        Ok(songs)
    }

    /// `getStarred2`.
    pub fn favorites(&self) -> Result<SearchResults> {
        let data: Value = self.request("getStarred2", &[])?;
        Ok(SearchResults {
            artist: Self::pick(&data, &["starred2", "artist"]),
            album: Self::pick(&data, &["starred2", "album"]),
            song: Self::pick(&data, &["starred2", "song"]),
        })
    }

    /// Stars or unstars a song, album or artist (`id` / `albumId` / `artistId`).
    pub fn star_item(&self, kind: &str, id: &str, active: bool) -> Result<()> {
        let key = match kind {
            "album" => "albumId",
            "artist" => "artistId",
            _ => "id",
        };
        self.request::<Value>(if active { "unstar" } else { "star" }, &[(key, id.into())]).map(|_| ())
    }

    /// `active` is the current favorite state; true removes the favorite.
    pub fn star(&self, song_id: &str, active: bool) -> Result<()> {
        self.request::<Value>(if active { "unstar" } else { "star" }, &[("id", song_id.into())])
            .map(|_| ())
    }

    pub fn scrobble(&self, song_id: &str, submission: bool) -> Result<()> {
        self.request::<Value>(
            "scrobble",
            &[("id", song_id.into()), ("submission", submission.to_string())],
        )
        .map(|_| ())
    }

    /// `stream(song, original)`: `format=raw`, or MP3 at 320 kbps when not original.
    pub fn stream_url(&self, song_id: &str, original: bool) -> String {
        let mut query = vec![("id", song_id.to_string()), ("format", if original { "raw" } else { "mp3" }.into())];
        if !original {
            query.push(("maxBitRate", "320".into()));
        }
        self.url("stream", &query)
    }

    /// `cover(id, size)`, memoized per id and size.
    pub fn cover_url(&self, id: &str, size: Option<u32>) -> String {
        let key = format!("{id}:{}", size.map(|s| s.to_string()).unwrap_or_else(|| "original".into()));
        let mut covers = self.covers.lock();
        covers
            .entry(key)
            .or_insert_with(|| {
                let mut query = vec![("id", id.to_string())];
                if let Some(size) = size {
                    query.push(("size", size.to_string()));
                }
                self.url("getCoverArt", &query)
            })
            .clone()
    }

    /// Fetches cover art bytes.
    pub fn cover_bytes(&self, id: &str, size: Option<u32>) -> Result<Vec<u8>> {
        // Local covers are files on disk (`local-file:<path>`).
        if let Some(path) = id.strip_prefix("local-file:") {
            return std::fs::read(path).map_err(|e| NavidromeError::Other(e.to_string()));
        }
        if self.is_offline() {
            return Err(NavidromeError::Other("No artwork".into()));
        }
        let url = self.cover_url(id, size);
        let response = self.http.get(url).send().map_err(|e| NavidromeError::Connection(redact(&e)))?;
        if !response.status().is_success() {
            return Err(NavidromeError::Other(format!("cover HTTP {}", response.status().as_u16())));
        }
        response
            .bytes()
            .map(|b| b.to_vec())
            .map_err(|e| NavidromeError::Other(redact(&e)))
    }
}

/// The fields `updatePlaylist` may change.
#[derive(Debug, Clone, Default)]
pub struct PlaylistChanges {
    pub name: Option<String>,
    pub comment: Option<String>,
    pub public: Option<bool>,
    pub song_ids_to_add: Vec<String>,
    pub song_indexes_to_remove: Vec<usize>,
}

/// `LibraryResults` in navidrome.ts.
#[derive(Debug, Clone, Default)]
pub struct SearchResults {
    pub artist: Vec<Artist>,
    pub album: Vec<AlbumRecord>,
    pub song: Vec<Song>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_md5_of_password_and_salt() {
        assert_eq!(auth_token("sesame", "c19b2d"), "26719a1196d2a940705a59634eb18eab");
        assert_eq!(random_salt().len(), 32);
    }

    #[test]
    fn url_has_subsonic_params_and_client_name() {
        let client = Navidrome::new(Credentials {
            server: "https://music.test/rest".into(),
            username: " Listener ".into(),
            password: "pw".into(),
            auth: Some(AuthMaterial { salt: "s".into(), token: "t".into() }),
        })
        .unwrap();
        assert_eq!(client.server, "https://music.test");
        assert_eq!(client.username, "Listener");
        assert_eq!(
            client.stream_url("7", false),
            "https://music.test/rest/stream.view?u=Listener&t=t&s=s&v=1.16.1&c=VoltaDesktop&f=json&id=7&format=mp3&maxBitRate=320"
        );
        assert_eq!(client.cover_url("c", Some(100)), client.cover_url("c", Some(100)));
    }
}
