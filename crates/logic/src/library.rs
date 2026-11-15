//! Library helpers from App.tsx: `similarAlbumsFor`, `similarArtistsFor`,
//! `sortArtistSongsByPopularity`, plus the collation used by sort controls.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use crate::format::album_name;
use crate::model::{AlbumRecord, Artist, Song};

fn genre_key(genre: Option<&str>) -> String {
    genre.map(|g| g.trim().to_lowercase()).unwrap_or_default()
}

/// Approximates `String.prototype.localeCompare` / `Intl.Collator` with
/// `sensitivity: "base", numeric: true`: case-insensitive, digit runs compared
/// by value.
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    let mut left = a.chars().peekable();
    let mut right = b.chars().peekable();
    loop {
        match (left.peek().copied(), right.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        it.next();
                    }
                    digits
                };
                let (dx, dy) = (take(&mut left), take(&mut right));
                let (tx, ty) = (dx.trim_start_matches('0'), dy.trim_start_matches('0'));
                let order = tx.len().cmp(&ty.len()).then_with(|| tx.cmp(ty));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let (fx, fy) = (fold(x), fold(y));
                if fx != fy {
                    return fx.cmp(&fy);
                }
                left.next();
                right.next();
            }
        }
    }
}

/// Base-letter folding for the collator (case and common accents).
fn fold(c: char) -> String {
    use unicode_normalization::UnicodeNormalization;
    c.to_lowercase()
        .collect::<String>()
        .nfd()
        .filter(|ch| !unicode_normalization::char::is_combining_mark(*ch))
        .collect()
}

/// `similarAlbumsFor`: same genre (+100), same artist (+25), year closeness
/// (up to +10); top 6.
pub fn similar_albums_for(source: &AlbumRecord, library: &[AlbumRecord]) -> Vec<AlbumRecord> {
    let source_genre = genre_key(source.genre.as_deref());
    let mut scored: Vec<(&AlbumRecord, f64)> = library
        .iter()
        .filter(|album| album.id != source.id)
        .map(|album| {
            let same_genre = !source_genre.is_empty() && genre_key(album.genre.as_deref()) == source_genre;
            let same_artist = matches!((&source.artist_id, &album.artist_id), (Some(a), Some(b)) if !a.is_empty() && a == b);
            let year_distance = match (source.year, album.year) {
                (Some(a), Some(b)) if a != 0.0 && b != 0.0 => (a - b).abs(),
                _ => 100.0,
            };
            let score = if same_genre { 100.0 } else { 0.0 }
                + if same_artist { 25.0 } else { 0.0 }
                + (10.0 - year_distance.min(10.0)).max(0.0);
            (album, score)
        })
        .filter(|(album, score)| *score > 10.0 || genre_key(album.genre.as_deref()) == source_genre)
        .collect();
    scored.sort_by(|(a, sa), (b, sb)| {
        sb.partial_cmp(sa)
            .unwrap_or(Ordering::Equal)
            .then_with(|| locale_compare(album_name(a), album_name(b)))
    });
    scored.into_iter().take(6).map(|(album, _)| album.clone()).collect()
}

/// `similarArtistsFor`: artists sharing a genre with the source's albums,
/// ranked by how many such albums they have; top 6.
pub fn similar_artists_for(source: &Artist, library: &[AlbumRecord]) -> Vec<Artist> {
    let source_genres: HashSet<String> = source
        .album
        .iter()
        .flatten()
        .map(|album| genre_key(album.genre.as_deref()))
        .filter(|g| !g.is_empty())
        .collect();
    let mut order: Vec<String> = Vec::new();
    let mut candidates: HashMap<String, (Artist, u32)> = HashMap::new();
    for album in library {
        let Some(name) = album.artist.as_deref().map(str::trim).filter(|n| !n.is_empty()) else { continue };
        let id = album.artist_id.clone().filter(|id| !id.is_empty()).unwrap_or_else(|| name.to_string());
        if id == source.id {
            continue;
        }
        let genre = genre_key(album.genre.as_deref());
        if genre.is_empty() || !source_genres.contains(&genre) {
            continue;
        }
        let entry = candidates.entry(id.clone()).or_insert_with(|| {
            order.push(id.clone());
            (
                Artist {
                    id: id.clone(),
                    name: name.to_string(),
                    cover_art: album.cover_art.clone(),
                    album_count: Some(0.0),
                    ..Default::default()
                },
                0,
            )
        });
        entry.0.album_count = Some(entry.0.album_count.unwrap_or(0.0) + 1.0);
        entry.1 += 1;
    }
    let mut ranked: Vec<(Artist, u32)> = order.into_iter().filter_map(|id| candidates.remove(&id)).collect();
    ranked.sort_by(|(a, sa), (b, sb)| sb.cmp(sa).then_with(|| locale_compare(&a.name, &b.name)));
    ranked.into_iter().take(6).map(|(artist, _)| artist).collect()
}

/// `sortArtistSongsByPopularity`: most-played first, stable for ties; search
/// order when the server does not report play counts.
pub fn sort_artist_songs_by_popularity(songs: Vec<Song>) -> Vec<Song> {
    if !songs.iter().any(|s| s.play_count.is_some_and(f64::is_finite)) {
        return songs;
    }
    let mut songs = songs;
    songs.sort_by(|a, b| {
        b.play_count
            .unwrap_or(0.0)
            .partial_cmp(&a.play_count.unwrap_or(0.0))
            .unwrap_or(Ordering::Equal)
    });
    songs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(id: &str, name: &str, artist_id: &str, genre: &str, year: f64) -> AlbumRecord {
        AlbumRecord {
            id: id.into(),
            name: Some(name.into()),
            artist: Some(format!("Artist {artist_id}")),
            artist_id: Some(artist_id.into()),
            genre: Some(genre.into()),
            year: Some(year),
            ..Default::default()
        }
    }

    #[test]
    fn similar_albums_rank_genre_then_artist_then_year() {
        let source = album("s", "Source", "a", "Jazz", 2020.0);
        let library = vec![
            source.clone(),
            album("1", "Zed", "b", "jazz ", 2010.0),
            album("2", "Alpha", "a", "Rock", 2020.0),
            album("3", "Beta", "c", "Jazz", 2021.0),
            album("4", "Far", "d", "Rock", 1990.0),
        ];
        let ids: Vec<_> = similar_albums_for(&source, &library).into_iter().map(|a| a.id).collect();
        // Jazz 2021 (109) > Jazz 2010 (100) > same artist, same year (35); the
        // unrelated 1990 rock album scores 0 and is dropped.
        assert_eq!(ids, ["3", "1", "2"]);
    }

    #[test]
    fn similar_artists_count_shared_genre_albums() {
        let source = Artist {
            id: "a".into(),
            name: "A".into(),
            album: Some(vec![album("x", "X", "a", "Jazz", 2000.0)]),
            ..Default::default()
        };
        let library = vec![
            album("1", "One", "b", "Jazz", 2000.0),
            album("2", "Two", "c", "Jazz", 2000.0),
            album("3", "Three", "c", "Jazz", 2000.0),
            album("4", "Four", "d", "Rock", 2000.0),
            album("5", "Five", "a", "Jazz", 2000.0),
        ];
        let artists = similar_artists_for(&source, &library);
        let ids: Vec<_> = artists.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["c", "b"]);
        assert_eq!(artists[0].album_count, Some(2.0));
    }

    #[test]
    fn popularity_sort_is_stable_and_optional() {
        let song = |id: &str, plays: Option<f64>| Song { id: id.into(), play_count: plays, ..Default::default() };
        let plain = vec![song("a", None), song("b", None)];
        assert_eq!(sort_artist_songs_by_popularity(plain.clone()), plain);
        let ids: Vec<_> = sort_artist_songs_by_popularity(vec![song("a", Some(1.0)), song("b", Some(5.0)), song("c", Some(1.0))])
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, ["b", "a", "c"]);
    }

    #[test]
    fn collation_is_case_and_number_aware() {
        assert_eq!(locale_compare("apple", "Banana"), Ordering::Less);
        assert_eq!(locale_compare("Track 2", "Track 10"), Ordering::Less);
        assert_eq!(locale_compare("Émile", "emile").then(Ordering::Equal), locale_compare("Émile", "emile"));
        assert_eq!(locale_compare("Éa", "eb"), Ordering::Less);
    }
}
