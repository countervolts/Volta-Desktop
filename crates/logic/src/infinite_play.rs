//! Port of `src/lib/infinite-play.ts`.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::model::Song;

#[derive(Debug, Clone)]
pub struct AlbumSongGroup {
    pub album_id: String,
    pub songs: Vec<Song>,
}

/// `infinitePlayAlbumTarget`.
pub fn infinite_play_album_target(song_count: usize) -> usize {
    song_count.max(1).clamp(24, 64)
}

const MAX_SONG_PAGE: usize = 100;

fn album_order(left: &Song, right: &Song) -> std::cmp::Ordering {
    let disc = left.disc_number.filter(|d| *d != 0.0).unwrap_or(1.0) - right.disc_number.filter(|d| *d != 0.0).unwrap_or(1.0);
    if disc != 0.0 {
        return disc.partial_cmp(&0.0).unwrap();
    }
    let track = left.track.unwrap_or(0.0) - right.track.unwrap_or(0.0);
    if track != 0.0 {
        return track.partial_cmp(&0.0).unwrap();
    }
    left.id.cmp(&right.id)
}

fn unique(songs: &[Song]) -> Vec<Song> {
    let mut seen = HashSet::new();
    songs.iter().filter(|s| !s.id.is_empty() && seen.insert(s.id.clone())).cloned().collect()
}

/// `interleaveAlbumSongs`: one track per album per round, in album order.
pub fn interleave_album_songs(groups: &[AlbumSongGroup], count: usize, excluded: &HashSet<String>) -> Vec<Song> {
    if count == 0 {
        return Vec::new();
    }
    let mut claimed = HashSet::new();
    let mut albums: Vec<Vec<Song>> = Vec::new();
    for group in groups {
        let mut songs: Vec<Song> = unique(&group.songs).into_iter().filter(|s| !excluded.contains(&s.id) && !claimed.contains(&s.id)).collect();
        songs.sort_by(album_order);
        if songs.is_empty() {
            continue;
        }
        claimed.extend(songs.iter().map(|s| s.id.clone()));
        albums.push(songs);
    }
    let longest = albums.iter().map(Vec::len).max().unwrap_or(0);
    let mut output = Vec::new();
    'rounds: for round in 0..longest {
        for album in &albums {
            if let Some(song) = album.get(round) {
                output.push(song.clone());
                if output.len() >= count {
                    break 'rounds;
                }
            }
        }
    }
    output
}

/// `rankInfinitePlayAlbums`: fresh shelf then pool albums, then queued ones.
pub fn rank_infinite_play_albums(shelf: &[String], pool: &[String], queued: &HashSet<String>, limit: Option<usize>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut fresh = Vec::new();
    let mut again = Vec::new();
    let mut add = |ids: &mut dyn Iterator<Item = &String>, target: &mut Vec<String>| {
        for raw in ids {
            let id = raw.trim();
            if !id.is_empty() && seen.insert(id.to_string()) {
                target.push(id.to_string());
            }
        }
    };
    add(&mut shelf.iter().filter(|id| !queued.contains(id.trim())), &mut fresh);
    add(&mut pool.iter().filter(|id| !queued.contains(id.trim())), &mut fresh);
    add(&mut shelf.iter(), &mut again);
    add(&mut pool.iter(), &mut again);
    let mut ordered = fresh;
    ordered.extend(again);
    if let Some(limit) = limit.filter(|l| *l > 0) {
        ordered.truncate(limit);
    }
    ordered
}

/// `loadRankedAlbumSongs` (blocking): pages `fetch(offset, size)` until enough
/// albums and songs are held, then orders the groups by `ranked`.
pub fn load_ranked_album_songs<E>(
    ranked: &[String],
    wanted_albums: usize,
    wanted_songs: usize,
    exclude_album_ids: &HashSet<String>,
    mut fetch: impl FnMut(usize, usize) -> Result<Vec<Song>, E>,
) -> Result<Vec<AlbumSongGroup>, E> {
    let target = wanted_albums.max(1);
    let song_target = wanted_songs.max(1);
    let (page_size, max_pages) = (200usize, 25usize);
    let excluded: HashSet<String> = exclude_album_ids.iter().map(|id| id.trim().to_string()).filter(|id| !id.is_empty()).collect();
    let mut by_album: IndexMap<String, Vec<Song>> = IndexMap::new();
    let (mut offset, mut pages, mut held, mut fresh) = (0usize, 0usize, 0usize, 0usize);
    while pages < max_pages && (by_album.len() < target || held < song_target || fresh < target) {
        let page = fetch(offset, page_size)?;
        if page.is_empty() {
            break;
        }
        let length = page.len();
        offset += length;
        for song in page {
            let id = song.album_id.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(song.id.trim()).to_string();
            if id.is_empty() || song.id.is_empty() {
                continue;
            }
            match by_album.get_mut(&id) {
                Some(bucket) => bucket.push(song),
                None => {
                    if !excluded.contains(&id) {
                        fresh += 1;
                    }
                    by_album.insert(id, vec![song]);
                }
            }
            held += 1;
        }
        pages += 1;
        if length < page_size.min(MAX_SONG_PAGE) {
            break;
        }
    }
    let mut rank_of: HashMap<String, usize> = HashMap::new();
    for (index, raw) in ranked.iter().enumerate() {
        let id = raw.trim();
        if !id.is_empty() {
            rank_of.entry(id.to_string()).or_insert(index);
        }
    }
    let mut groups: Vec<AlbumSongGroup> = by_album.into_iter().map(|(album_id, songs)| AlbumSongGroup { album_id, songs }).collect();
    groups.sort_by(|a, b| {
        let (ra, rb) = (rank_of.get(&a.album_id).copied().unwrap_or(usize::MAX), rank_of.get(&b.album_id).copied().unwrap_or(usize::MAX));
        ra.cmp(&rb).then_with(|| a.album_id.cmp(&b.album_id))
    });
    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, album: &str, track: f64) -> Song {
        Song { id: id.into(), album_id: Some(album.into()), track: Some(track), ..Default::default() }
    }

    #[test]
    fn interleaves_one_track_per_album_per_round() {
        let groups = vec![
            AlbumSongGroup { album_id: "a".into(), songs: vec![song("a2", "a", 2.0), song("a1", "a", 1.0)] },
            AlbumSongGroup { album_id: "b".into(), songs: vec![song("b1", "b", 1.0)] },
        ];
        let ids: Vec<String> = interleave_album_songs(&groups, 3, &HashSet::new()).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["a1", "b1", "a2"]);
    }

    #[test]
    fn ranks_fresh_albums_before_queued() {
        let queued: HashSet<String> = ["s1".to_string()].into_iter().collect();
        let ranked = rank_infinite_play_albums(&["s1".into(), "s2".into()], &["p1".into(), "s2".into()], &queued, None);
        assert_eq!(ranked, ["s2", "p1", "s1"]);
        assert_eq!(infinite_play_album_target(50), 50);
        assert_eq!(infinite_play_album_target(3), 24);
    }

    #[test]
    fn loads_and_orders_groups_by_rank() {
        let songs = vec![song("x1", "x", 1.0), song("y1", "y", 1.0), song("y2", "y", 2.0)];
        let groups =
            load_ranked_album_songs::<()>(&["y".into()], 1, 1, &HashSet::new(), |offset, _| Ok(if offset == 0 { songs.clone() } else { vec![] }))
                .unwrap();
        assert_eq!(groups[0].album_id, "y");
        assert_eq!(groups[1].album_id, "x");
    }
}
