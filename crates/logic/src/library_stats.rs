//! Totals and breakdowns for the whole library (Stats → Library).
//!
//! Pure: takes the loaded songs, returns numbers. Tags are often missing, so
//! every count ignores empty values rather than inventing an "Unknown" bucket.

use std::collections::{HashMap, HashSet};

use crate::model::Song;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LibraryStats {
    pub songs: usize,
    pub albums: usize,
    pub artists: usize,
    pub genres: usize,
    pub total_seconds: f64,
    pub total_bytes: f64,
    pub favorites: usize,
    pub lossless: usize,
    pub hi_res: usize,
    /// Mean of the tracks that report a bit rate, in kbps.
    pub average_bitrate: Option<f64>,
    pub longest: Option<(String, f64)>,
    pub oldest_year: Option<i32>,
    pub newest_year: Option<i32>,
    /// Most common first, at most [`TOP`] entries.
    pub top_genres: Vec<(String, usize)>,
    pub top_artists: Vec<(String, usize)>,
    pub formats: Vec<(String, usize)>,
    /// Ascending, e.g. ("1990s", 120).
    pub decades: Vec<(String, usize)>,
}

pub const TOP: usize = 8;

const LOSSLESS: &[&str] = &["flac", "alac", "wav", "aiff", "aif", "ape", "wv", "dsf", "dff"];

fn clean(value: &Option<String>) -> Option<String> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(str::to_string)
}

fn ranked(counts: HashMap<String, usize>) -> Vec<(String, usize)> {
    let mut list: Vec<_> = counts.into_iter().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    list.truncate(TOP);
    list
}

pub fn summarize(songs: &[Song]) -> LibraryStats {
    let mut stats = LibraryStats { songs: songs.len(), ..Default::default() };
    let mut albums = HashSet::new();
    let mut artists = HashSet::new();
    let mut genre_counts: HashMap<String, usize> = HashMap::new();
    let mut artist_counts: HashMap<String, usize> = HashMap::new();
    let mut format_counts: HashMap<String, usize> = HashMap::new();
    let mut decade_counts: HashMap<i32, usize> = HashMap::new();
    let (mut bitrate_sum, mut bitrate_n) = (0.0, 0usize);

    for song in songs {
        let seconds = song.duration.filter(|d| d.is_finite() && *d > 0.0).unwrap_or(0.0);
        stats.total_seconds += seconds;
        stats.total_bytes += song.size.filter(|s| s.is_finite() && *s > 0.0).unwrap_or(0.0);
        if song.starred.is_some() {
            stats.favorites += 1;
        }
        if let Some(id) = song.album_id.clone().or_else(|| clean(&song.album)) {
            albums.insert(id);
        }
        let artist = clean(&song.album_artist).or_else(|| clean(&song.artist));
        if let Some(name) = artist {
            artists.insert(name.to_lowercase());
            *artist_counts.entry(name).or_default() += 1;
        }
        if let Some(genre) = clean(&song.genre) {
            *genre_counts.entry(genre).or_default() += 1;
        }
        let suffix = clean(&song.suffix).or_else(|| clean(&song.codec)).map(|s| s.to_lowercase());
        let lossless = suffix.as_deref().is_some_and(|s| LOSSLESS.contains(&s));
        if let Some(suffix) = &suffix {
            *format_counts.entry(suffix.to_uppercase()).or_default() += 1;
        }
        if lossless {
            stats.lossless += 1;
            if song.bit_depth.is_some_and(|b| b > 16.0) || song.sampling_rate.is_some_and(|r| r > 48_000.0) {
                stats.hi_res += 1;
            }
        }
        if let Some(rate) = song.bit_rate.filter(|r| *r > 0.0) {
            bitrate_sum += rate;
            bitrate_n += 1;
        }
        if stats.longest.as_ref().is_none_or(|(_, best)| seconds > *best) && seconds > 0.0 {
            stats.longest = Some((song.title.clone(), seconds));
        }
        if let Some(year) = song.year.map(|y| y as i32).filter(|y| (1000..=2999).contains(y)) {
            stats.oldest_year = Some(stats.oldest_year.map_or(year, |o| o.min(year)));
            stats.newest_year = Some(stats.newest_year.map_or(year, |n| n.max(year)));
            *decade_counts.entry(year / 10 * 10).or_default() += 1;
        }
    }

    stats.albums = albums.len();
    stats.artists = artists.len();
    stats.genres = genre_counts.len();
    stats.average_bitrate = (bitrate_n > 0).then(|| bitrate_sum / bitrate_n as f64);
    stats.top_genres = ranked(genre_counts);
    stats.top_artists = ranked(artist_counts);
    stats.formats = ranked(format_counts);
    let mut decades: Vec<_> = decade_counts.into_iter().collect();
    decades.sort_by_key(|(decade, _)| *decade);
    stats.decades = decades.into_iter().map(|(d, n)| (format!("{d}s"), n)).collect();
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, album: &str, artist: &str, secs: f64, suffix: &str, year: f64) -> Song {
        Song {
            id: id.into(),
            title: id.into(),
            album: Some(album.into()),
            artist: Some(artist.into()),
            duration: Some(secs),
            suffix: Some(suffix.into()),
            year: Some(year),
            size: Some(1000.0),
            ..Default::default()
        }
    }

    #[test]
    fn totals_and_breakdowns() {
        let songs = vec![
            song("a", "X", "Ann", 100.0, "flac", 1994.0),
            song("b", "X", "Ann", 200.0, "mp3", 1996.0),
            song("c", "Y", "Bo", 50.0, "mp3", 2011.0),
        ];
        let stats = summarize(&songs);
        assert_eq!((stats.songs, stats.albums, stats.artists), (3, 2, 2));
        assert_eq!(stats.total_seconds, 350.0);
        assert_eq!(stats.total_bytes, 3000.0);
        assert_eq!(stats.lossless, 1);
        assert_eq!(stats.longest, Some(("b".into(), 200.0)));
        assert_eq!((stats.oldest_year, stats.newest_year), (Some(1994), Some(2011)));
        assert_eq!(stats.decades, vec![("1990s".into(), 2), ("2010s".into(), 1)]);
        assert_eq!(stats.formats[0], ("MP3".into(), 2));
        assert_eq!(stats.top_artists[0], ("Ann".into(), 2));
    }

    #[test]
    fn empty_library_is_all_zero() {
        assert_eq!(summarize(&[]), LibraryStats::default());
    }
}
