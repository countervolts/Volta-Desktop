//! Port of `src/lib/folder-tree.ts`: the folder hierarchy derived from each
//! song's `path` / `localPath`, with search and size totals.

use std::collections::HashMap;

use crate::library::locale_compare;
use crate::model::Song;

#[derive(Debug, Clone, Default)]
pub struct FolderNode {
    pub path: String,
    pub name: String,
    /// Child folder paths, sorted by name.
    pub folders: Vec<String>,
    pub songs: Vec<Song>,
    pub total_songs: usize,
    pub total_bytes: u64,
    pub modified: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct FolderTree {
    pub by_path: HashMap<String, FolderNode>,
    pub has_paths: bool,
}

impl FolderTree {
    pub fn root(&self) -> &FolderNode {
        &self.by_path[""]
    }

    pub fn get(&self, path: &str) -> Option<&FolderNode> {
        self.by_path.get(path)
    }

    pub fn children(&self, node: &FolderNode) -> Vec<&FolderNode> {
        node.folders.iter().filter_map(|p| self.by_path.get(p)).collect()
    }
}

/// `songFolderPath`: the folder a song lives in, without a drive letter.
pub fn song_folder_path(song: &Song) -> String {
    let raw = song.local_path.as_deref().filter(|p| !p.is_empty()).or(song.path.as_deref()).unwrap_or("").trim();
    if raw.is_empty() {
        return String::new();
    }
    let mut segments: Vec<&str> = raw.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    segments.pop();
    if let Some(first) = segments.first() {
        let bytes = first.as_bytes();
        if bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            segments.remove(0);
        }
    }
    segments.join("/")
}

fn compare_songs(a: &Song, b: &Song) -> std::cmp::Ordering {
    let disc = |s: &Song| s.disc_number.unwrap_or(1.0);
    let track = |s: &Song| s.track.unwrap_or(f64::MAX);
    disc(a)
        .partial_cmp(&disc(b))
        .unwrap_or(std::cmp::Ordering::Equal)
        .then(track(a).partial_cmp(&track(b)).unwrap_or(std::cmp::Ordering::Equal))
        .then_with(|| locale_compare(&a.title, &b.title))
        .then_with(|| locale_compare(a.local_path.as_deref().unwrap_or(""), b.local_path.as_deref().unwrap_or("")))
}

/// `buildFolderTree`.
pub fn build_folder_tree(songs: &[Song]) -> FolderTree {
    let mut by_path: HashMap<String, FolderNode> = HashMap::new();
    by_path.insert(String::new(), FolderNode::default());
    let mut has_paths = false;

    fn ensure(by_path: &mut HashMap<String, FolderNode>, path: &str) {
        if by_path.contains_key(path) {
            return;
        }
        let (parent, name) = match path.rsplit_once('/') {
            Some((parent, name)) => (parent.to_string(), name.to_string()),
            None => (String::new(), path.to_string()),
        };
        ensure(by_path, &parent);
        by_path.get_mut(&parent).unwrap().folders.push(path.to_string());
        by_path.insert(path.to_string(), FolderNode { path: path.to_string(), name, ..Default::default() });
    }

    for song in songs {
        let path = song_folder_path(song);
        if !path.is_empty() {
            has_paths = true;
        }
        ensure(&mut by_path, &path);
        by_path.get_mut(&path).unwrap().songs.push(song.clone());
    }

    fn finalize(by_path: &mut HashMap<String, FolderNode>, path: &str) -> (usize, u64, Option<f64>) {
        let mut children = by_path[path].folders.clone();
        children.sort_by(|a, b| locale_compare(&by_path[a].name, &by_path[b].name));
        let mut totals = (0usize, 0u64, None::<f64>);
        for child in &children {
            let (songs, bytes, modified) = finalize(by_path, child);
            totals.0 += songs;
            totals.1 += bytes;
            totals.2 = match (totals.2, modified) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
        }
        let node = by_path.get_mut(path).unwrap();
        node.folders = children;
        node.songs.sort_by(compare_songs);
        totals.0 += node.songs.len();
        for song in &node.songs {
            totals.1 += song.size.unwrap_or(0.0).max(0.0) as u64;
            if let Some(m) = song.local_modified {
                totals.2 = Some(totals.2.map_or(m, |t: f64| t.max(m)));
            }
        }
        node.total_songs = totals.0;
        node.total_bytes = totals.1;
        node.modified = totals.2;
        totals
    }
    finalize(&mut by_path, "");
    FolderTree { by_path, has_paths }
}

/// `folderAncestors`: root first, down to `path`.
pub fn folder_ancestors<'a>(tree: &'a FolderTree, path: &str) -> Vec<&'a FolderNode> {
    let mut chain = Vec::new();
    let mut current = tree.get(path).unwrap_or(tree.root());
    loop {
        chain.insert(0, current);
        if current.path.is_empty() {
            break;
        }
        let parent = current.path.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        current = tree.get(parent).unwrap_or(tree.root());
    }
    chain
}

/// `collectFolderSongs`: a folder's songs then each descendant's, in tree order.
pub fn collect_folder_songs(tree: &FolderTree, node: &FolderNode) -> Vec<Song> {
    let mut songs = node.songs.clone();
    for child in tree.children(node) {
        songs.extend(collect_folder_songs(tree, child));
    }
    songs
}

pub struct FolderSearchResults<'a> {
    pub folders: Vec<&'a FolderNode>,
    pub songs: Vec<(&'a Song, &'a FolderNode)>,
}

/// `searchFolderTree`: folder names, song tags and paths, case-insensitive.
pub fn search_folder_tree<'a>(tree: &'a FolderTree, query: &str, limit: usize) -> FolderSearchResults<'a> {
    let needle = query.trim().to_lowercase();
    let mut results = FolderSearchResults { folders: Vec::new(), songs: Vec::new() };
    if needle.is_empty() {
        return results;
    }
    fn visit<'a>(tree: &'a FolderTree, node: &'a FolderNode, needle: &str, out: &mut FolderSearchResults<'a>) {
        if !node.path.is_empty() && node.name.to_lowercase().contains(needle) {
            out.folders.push(node);
        }
        for song in &node.songs {
            let haystack = format!(
                "{} {} {} {}",
                song.title,
                song.artist.as_deref().unwrap_or(""),
                song.album.as_deref().unwrap_or(""),
                song_folder_path(song)
            )
            .to_lowercase();
            if haystack.contains(needle) {
                out.songs.push((song, node));
            }
        }
        for child in tree.children(node) {
            visit(tree, child, needle, out);
        }
    }
    visit(tree, tree.root(), &needle, &mut results);
    results.folders.truncate(limit);
    results.songs.truncate(limit);
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::format_bytes;

    fn song(id: &str, title: &str, path: Option<&str>, size: f64) -> Song {
        Song { id: id.into(), title: title.into(), path: path.map(str::to_string), size: Some(size), ..Default::default() }
    }

    #[test]
    fn groups_songs_by_path() {
        let tree = build_folder_tree(&[
            song("a", "One", Some("/Music/Artist/Album/01 - One.flac"), 100.0),
            song("b", "Two", Some("/Music/Artist/Album/02 - Two.flac"), 200.0),
            song("c", "Three", Some("/Music/Artist/Other/01 - Three.flac"), 300.0),
            song("d", "Loose", Some("/Music/loose.flac"), 50.0),
        ]);
        assert!(tree.has_paths);
        let root = tree.root();
        assert_eq!(tree.children(root).iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Music"]);
        assert!(root.songs.is_empty());
        let music = tree.get("Music").unwrap();
        assert_eq!(music.songs.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["Loose"]);
        assert_eq!(music.total_songs, 4);
        assert_eq!(music.total_bytes, 650);
        let artist = tree.get("Music/Artist").unwrap();
        assert_eq!(tree.children(artist).iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Album", "Other"]);
        assert_eq!(artist.total_songs, 3);
        let album = tree.get("Music/Artist/Album").unwrap();
        assert_eq!(album.songs.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["One", "Two"]);
    }

    #[test]
    fn windows_separators_and_missing_paths() {
        assert_eq!(song_folder_path(&song("a", "A", Some("D:\\Music\\Artist\\Album\\a.mp3"), 0.0)), "Music/Artist/Album");
        let mut b = song("b", "B", None, 0.0);
        b.local_path = Some("Artist/Album/b.flac".into());
        assert_eq!(song_folder_path(&b), "Artist/Album");
        assert_eq!(song_folder_path(&song("c", "C", None, 0.0)), "");
        let tree = build_folder_tree(&[song("c", "C", None, 0.0)]);
        assert!(!tree.has_paths);
        assert_eq!(tree.root().songs.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["C"]);
    }

    #[test]
    fn descendants_ancestors_and_search() {
        let mut a = song("a", "Blue Hour", Some("/Music/Miles/Blue Hour/01.flac"), 0.0);
        a.artist = Some("Miles".into());
        let mut b = song("b", "Soft Focus", Some("/Music/Nora/Soft Focus/01.flac"), 0.0);
        b.artist = Some("Nora".into());
        let tree = build_folder_tree(&[a, b]);
        let music = tree.get("Music").unwrap();
        assert_eq!(collect_folder_songs(&tree, music).iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["Blue Hour", "Soft Focus"]);
        assert_eq!(
            folder_ancestors(&tree, "Music/Miles/Blue Hour").iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
            ["", "Music", "Miles", "Blue Hour"]
        );
        assert_eq!(search_folder_tree(&tree, "miles", 200).folders.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["Miles"]);
        assert_eq!(search_folder_tree(&tree, "soft focus", 200).songs.iter().map(|(s, _)| s.title.as_str()).collect::<Vec<_>>(), ["Soft Focus"]);
        assert!(search_folder_tree(&tree, "  ", 200).songs.is_empty());
    }

    #[test]
    fn sizes_format() {
        assert_eq!(format_bytes(0), "—");
        assert_eq!(format_bytes(512), "512 bytes");
        assert_eq!(format_bytes(2048), "2 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5 MB");
    }
}
