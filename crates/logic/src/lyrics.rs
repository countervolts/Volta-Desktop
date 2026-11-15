//! Lyrics model and parsers. Port of `src/lib/lyrics.ts`, plus
//! `pickSyncedLrclibResult` from `src/lib/navidrome.ts`.
//!
//! All times are seconds from the start of the track.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::model::Song;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LyricsWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub space: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub background: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LyricsLine {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<LyricsWord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backing: Option<Vec<LyricsWord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<Align>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pronunciation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    pub lines: Vec<LyricsLine>,
    pub synced: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_by_word: Option<bool>,
}

/// A word with no timing of its own gets this much time so its fill completes.
const WORD_TAIL_SECONDS: f64 = 0.4;
/// How far apart two timestamps may be and still be treated as the same line.
const SUPPORTING_MATCH_SECONDS: f64 = 1.5;

static LRC_LINE_TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(\d{1,3}):(\d{2})(?:[.:](\d{1,3}))?\]").unwrap());
static LRC_WORD_TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<(\d{1,3}):(\d{2})(?:[.:](\d{1,3}))?>").unwrap());
static LRC_BRACKETED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[^\]]*\]").unwrap());
static WHITESPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static TTML_OPEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<\s*tt[\s>]").unwrap());

fn lrc_stamp(minutes: &str, seconds: &str, fraction: Option<&str>) -> f64 {
    let fraction = fraction.unwrap_or("");
    let mut padded: String = fraction.chars().take(3).collect();
    while padded.len() < 3 {
        padded.push('0');
    }
    minutes.parse::<f64>().unwrap_or(0.0) * 60.0
        + seconds.parse::<f64>().unwrap_or(0.0)
        + padded.parse::<f64>().unwrap_or(0.0) / 1000.0
}

fn make_word(text: &str, start: f64, end: f64, space: bool, background: bool) -> LyricsWord {
    LyricsWord {
        text: text.to_string(),
        start,
        end,
        space,
        background,
    }
}

/// `hh:mm:ss`, `mm:ss`, or a bare second count.
pub fn clock_seconds(value: Option<&str>) -> Option<f64> {
    let text = value?.trim().replacen(',', ".", 1);
    if text.is_empty() {
        return None;
    }
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut seconds = 0.0;
    for part in parts {
        let amount = crate::js::js_number(part);
        if !amount.is_finite() {
            return None;
        }
        seconds = seconds * 60.0 + amount;
    }
    Some(seconds)
}

/// Fill in missing or degenerate end times so a word's fill always completes.
pub fn complete_word_times(words: Vec<LyricsWord>, line_end: Option<f64>) -> Vec<LyricsWord> {
    let starts: Vec<f64> = words.iter().map(|w| w.start).collect();
    words
        .into_iter()
        .enumerate()
        .map(|(index, mut word)| {
            if word.end > word.start {
                return word;
            }
            let fallback = match starts.get(index + 1) {
                Some(&next) if next > word.start => next,
                _ => match line_end {
                    Some(end) if end != 0.0 && end > word.start => end,
                    _ => word.start + WORD_TAIL_SECONDS,
                },
            };
            word.end = fallback;
            word
        })
        .collect()
}

fn words_to_text(words: &[LyricsWord]) -> String {
    let mut text = String::new();
    for word in words {
        text.push_str(&word.text);
        if word.space {
            text.push(' ');
        }
    }
    text.trim().to_string()
}

fn parse_lrc_words(body: &str) -> Vec<LyricsWord> {
    let tags: Vec<_> = LRC_WORD_TIME.captures_iter(body).collect();
    if tags.is_empty() {
        return Vec::new();
    }
    let stamp = |c: &regex::Captures| {
        lrc_stamp(&c[1], &c[2], c.get(3).map(|m| m.as_str()))
    };
    let mut words = Vec::new();
    let mut closing = None;
    for (index, tag) in tags.iter().enumerate() {
        let whole = tag.get(0).unwrap();
        let from = whole.end();
        let to = tags
            .get(index + 1)
            .map(|next| next.get(0).unwrap().start())
            .unwrap_or(body.len());
        let raw = &body[from..to];
        let start = stamp(tag);
        if raw.trim().is_empty() {
            // A tag with no text behind it only closes the word before it.
            closing = Some(start);
            continue;
        }
        let next = tags.get(index + 1).map(stamp);
        let text = raw.trim_end();
        words.push(make_word(
            text,
            start,
            next.unwrap_or(start + WORD_TAIL_SECONDS),
            raw.len() > text.len(),
            false,
        ));
    }
    complete_word_times(words, closing)
}

pub fn parse_lrc(value: &str) -> Option<Lyrics> {
    let mut lines: Vec<LyricsLine> = Vec::new();
    for raw_line in value.split('\n') {
        let raw_line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let stamps: Vec<_> = LRC_LINE_TIME.captures_iter(raw_line).collect();
        if stamps.is_empty() {
            continue;
        }
        let without_times = LRC_LINE_TIME.replace_all(raw_line, "");
        let body = LRC_BRACKETED.replace_all(&without_times, "");
        let words = parse_lrc_words(&body);
        let text = if words.is_empty() {
            body.trim().to_string()
        } else {
            words_to_text(&words)
        };
        for stamp in &stamps {
            let mut line = LyricsLine {
                text: text.clone(),
                start: Some(lrc_stamp(&stamp[1], &stamp[2], stamp.get(3).map(|m| m.as_str()))),
                ..Default::default()
            };
            // Repeating one line at several timestamps makes its inline word
            // times ambiguous, so only a single-stamp line keeps them.
            if stamps.len() == 1 && !words.is_empty() {
                line.words = Some(words.clone());
            }
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return None;
    }
    // Array.prototype.sort is stable, as is sort_by.
    lines.sort_by(|a, b| {
        a.start
            .unwrap_or(0.0)
            .partial_cmp(&b.start.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let word_by_word = lines.iter().any(|l| l.words.as_ref().is_some_and(|w| !w.is_empty()));
    Some(Lyrics {
        lines,
        synced: true,
        language: None,
        word_by_word: Some(word_by_word),
    })
}

pub fn parse_plain_lyrics(value: &str) -> Option<Lyrics> {
    if value.trim().is_empty() {
        return None;
    }
    Some(Lyrics {
        synced: false,
        lines: value
            .split('\n')
            .map(|text| LyricsLine {
                text: text.strip_suffix('\r').unwrap_or(text).to_string(),
                ..Default::default()
            })
            .collect(),
        language: None,
        word_by_word: None,
    })
}

/* ------------------------------------------------------------------ TTML -- */

type Node<'a, 'i> = roxmltree::Node<'a, 'i>;

fn read_attribute<'a>(element: Node<'a, '_>, name: &str) -> Option<&'a str> {
    element
        .attributes()
        .find(|attribute| attribute.name() == name)
        .map(|attribute| attribute.value())
}

fn read_inline_text(element: Node) -> String {
    let mut text = String::new();
    for node in element.children() {
        if node.is_text() {
            text.push_str(node.text().unwrap_or(""));
        } else if node.is_element() {
            if node.tag_name().name() == "br" {
                text.push(' ');
            } else {
                text.push_str(&read_inline_text(node));
            }
        }
    }
    text
}

fn collapse(text: &str) -> String {
    WHITESPACE.replace_all(text, " ").trim().to_string()
}

fn read_text(element: Node) -> String {
    collapse(&read_inline_text(element))
}

fn read_references(document: &roxmltree::Document, container: &str, item: &str) -> HashMap<String, String> {
    let mut references = HashMap::new();
    for scope in document
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == container)
    {
        for child in scope
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == item && *n != scope)
        {
            let key = read_attribute(child, "for");
            let text = read_text(child);
            if let Some(key) = key.filter(|k| !k.is_empty())
                && !text.is_empty() {
                    references.insert(key.to_string(), text);
                }
        }
    }
    references
}

fn read_ttml_words(paragraph: Node, line_end: Option<f64>) -> (Vec<LyricsWord>, Vec<LyricsWord>) {
    fn collect(scope: Node, background: bool, words: &mut Vec<LyricsWord>, backing: &mut Vec<LyricsWord>) {
        for element in scope.children().filter(|n| n.is_element()) {
            if element.tag_name().name() != "span" {
                continue;
            }
            if read_attribute(element, "role") == Some("x-bg") {
                collect(element, true, words, backing);
                continue;
            }
            let Some(start) = clock_seconds(read_attribute(element, "begin")) else {
                collect(element, background, words, backing);
                continue;
            };
            let raw = read_inline_text(element);
            let text = collapse(&raw);
            if text.is_empty() {
                continue;
            }
            let end = clock_seconds(read_attribute(element, "end")).unwrap_or(start);
            let space = raw.chars().last().is_some_and(char::is_whitespace);
            let word = make_word(&text, start, end, space, background);
            if background {
                backing.push(word);
            } else {
                words.push(word);
            }
        }
    }
    let mut words = Vec::new();
    let mut backing = Vec::new();
    collect(paragraph, false, &mut words, &mut backing);
    (
        complete_word_times(words, line_end),
        complete_word_times(backing, line_end),
    )
}

/// Parse TTML lyrics. Returns `None` when the text is not TTML.
pub fn parse_ttml(value: &str) -> Option<Lyrics> {
    if !TTML_OPEN.is_match(value) {
        return None;
    }
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let document = roxmltree::Document::parse_with_options(value, options).ok()?;
    let paragraphs: Vec<_> = document
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "p")
        .collect();
    if paragraphs.is_empty() {
        return None;
    }
    let translations = read_references(&document, "translations", "translation");
    let transliterations = read_references(&document, "transliterations", "transliteration");
    let mut lines = Vec::new();
    let mut word_by_word = false;
    for paragraph in paragraphs {
        let start = clock_seconds(read_attribute(paragraph, "begin"));
        let end = clock_seconds(read_attribute(paragraph, "end"));
        let key = read_attribute(paragraph, "key").filter(|k| !k.is_empty());
        let agent = read_attribute(paragraph, "agent").filter(|a| !a.is_empty());
        let (words, backing) = read_ttml_words(paragraph, end);
        if !words.is_empty() {
            word_by_word = true;
        }
        let mut line = LyricsLine {
            text: read_text(paragraph),
            start,
            end,
            ..Default::default()
        };
        if !words.is_empty() {
            line.words = Some(words);
        }
        if !backing.is_empty() {
            line.backing = Some(backing);
        }
        if agent.is_some_and(|a| a != "v1") {
            line.align = Some(Align::Right);
        }
        if let Some(key) = key {
            line.translation = translations.get(key).cloned();
            line.pronunciation = transliterations.get(key).cloned();
        }
        lines.push(line);
    }
    let tt = document
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "tt")
        .unwrap_or_else(|| document.root_element());
    let language = read_attribute(tt, "lang").filter(|l| !l.is_empty());
    let synced = lines.iter().any(|l| l.start.is_some());
    Some(Lyrics {
        lines,
        synced,
        language: language.map(str::to_string),
        word_by_word: Some(word_by_word),
    })
}

/// TTML first, then LRC, then plain.
pub fn parse_lyrics_text(value: &str) -> Option<Lyrics> {
    parse_ttml(value)
        .or_else(|| parse_lrc(value))
        .or_else(|| parse_plain_lyrics(value))
}

/* ------------------------------------------- OpenSubsonic structured lyrics -- */

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StructuredCue {
    pub start: Option<f64>,
    pub end: Option<f64>,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuredCueLine {
    pub index: Option<f64>,
    pub start: Option<f64>,
    pub end: Option<f64>,
    pub value: Option<String>,
    pub agent_id: Option<String>,
    pub cue: Option<Vec<StructuredCue>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StructuredAgent {
    pub id: Option<String>,
    pub role: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StructuredLine {
    pub value: Option<String>,
    pub start: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuredLyrics {
    pub lang: Option<String>,
    pub offset: Option<f64>,
    pub synced: Option<bool>,
    pub kind: Option<String>,
    pub line: Option<Vec<StructuredLine>>,
    pub cue_line: Option<Vec<StructuredCueLine>>,
    pub agents: Option<Vec<StructuredAgent>>,
}

fn shifter(offset: f64) -> impl Fn(f64) -> f64 {
    let seconds = if offset.is_finite() { offset } else { 0.0 } / 1000.0;
    move |value| (value - seconds).max(0.0)
}

fn role_of<'a>(line: &StructuredCueLine, roles: &'a HashMap<String, String>) -> &'a str {
    match &line.agent_id {
        Some(id) if !id.is_empty() => roles.get(id).map(String::as_str).unwrap_or("main"),
        _ => "main",
    }
}

fn sort_by_start(words: &mut [LyricsWord]) {
    words.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
}

fn structured_words(
    group: &[&StructuredCueLine],
    roles: &HashMap<String, String>,
    shift: &dyn Fn(f64) -> f64,
    line_end: Option<f64>,
) -> (Vec<LyricsWord>, Vec<LyricsWord>) {
    let mut words = Vec::new();
    let mut backing = Vec::new();
    for cue_line in group {
        let background = role_of(cue_line, roles) == "bg";
        for cue in cue_line.cue.iter().flatten() {
            let raw = cue.value.as_deref().unwrap_or("");
            let text = raw.trim_end();
            if text.is_empty() {
                continue;
            }
            let start = shift(cue.start.or(cue_line.start).unwrap_or(0.0) / 1000.0);
            let end = shift(cue.end.unwrap_or(0.0) / 1000.0);
            let word = make_word(text, start, end, raw.len() > text.len(), background);
            if background {
                backing.push(word);
            } else {
                words.push(word);
            }
        }
    }
    sort_by_start(&mut words);
    sort_by_start(&mut backing);
    (
        complete_word_times(words, line_end),
        complete_word_times(backing, line_end),
    )
}

fn attach_supporting(lines: &mut [LyricsLine], supporting: Option<Vec<(f64, String)>>, translation: bool) {
    let Some(supporting) = supporting.filter(|s| !s.is_empty()) else {
        return;
    };
    let mut used = vec![false; supporting.len()];
    for line in lines.iter_mut() {
        let Some(start) = line.start else { continue };
        let mut best: Option<usize> = None;
        let mut best_distance = f64::INFINITY;
        for (index, (item_start, _)) in supporting.iter().enumerate() {
            if used[index] {
                continue;
            }
            let distance = (item_start - start).abs();
            if distance < best_distance {
                best_distance = distance;
                best = Some(index);
            }
        }
        if let Some(best) = best.filter(|_| best_distance <= SUPPORTING_MATCH_SECONDS) {
            used[best] = true;
            let value = supporting[best].1.clone();
            if translation {
                line.translation = Some(value);
            } else {
                line.pronunciation = Some(value);
            }
        }
    }
}

/// Turn OpenSubsonic `structuredLyrics` into the shared model.
pub fn lyrics_from_structured(entries: &[StructuredLyrics]) -> Option<Lyrics> {
    if entries.is_empty() {
        return None;
    }
    let has_lines = |e: &&StructuredLyrics| e.line.as_ref().is_some_and(|l| !l.is_empty());
    let mains: Vec<&StructuredLyrics> = entries
        .iter()
        .filter(|e| e.kind.as_deref().unwrap_or("main") == "main")
        .filter(has_lines)
        .collect();
    let main = mains
        .iter()
        .find(|e| e.synced == Some(true))
        .or(mains.first())
        .copied()?;
    let shift = shifter(main.offset.unwrap_or(0.0));
    let roles: HashMap<String, String> = main
        .agents
        .iter()
        .flatten()
        .map(|a| {
            (
                a.id.clone().unwrap_or_default(),
                a.role.clone().unwrap_or_else(|| "main".into()),
            )
        })
        .collect();
    let mut groups: HashMap<i64, Vec<&StructuredCueLine>> = HashMap::new();
    for cue_line in main.cue_line.iter().flatten() {
        let index = cue_line.index.map(|i| i as i64).unwrap_or(-1);
        groups.entry(index).or_default().push(cue_line);
    }
    let mut lines = Vec::new();
    let mut word_by_word = false;
    for (index, line) in main.line.iter().flatten().enumerate() {
        let start = line.start.filter(|s| s.is_finite()).map(|s| shift(s / 1000.0));
        let group = groups.get(&(index as i64)).cloned().unwrap_or_default();
        let lead = group
            .iter()
            .find(|c| role_of(c, &roles) == "main")
            .or(group.first())
            .copied();
        let end = lead.and_then(|l| l.end).map(|e| shift(e / 1000.0));
        let (words, backing) = structured_words(&group, &roles, &shift, end);
        let secondary = group.iter().any(|c| {
            let role = role_of(c, &roles);
            role != "main" && role != "bg"
        });
        if !words.is_empty() {
            word_by_word = true;
        }
        lines.push(LyricsLine {
            text: line.value.clone().unwrap_or_default(),
            start,
            end,
            words: (!words.is_empty()).then_some(words),
            backing: (!backing.is_empty()).then_some(backing),
            align: secondary.then_some(Align::Right),
            translation: None,
            pronunciation: None,
        });
    }
    let supporting = |kind: &str| {
        let entry = entries
            .iter()
            .find(|e| e.kind.as_deref() == Some(kind) && e.line.as_ref().is_some_and(|l| !l.is_empty()))?;
        let shift_entry = shifter(entry.offset.unwrap_or(0.0));
        Some(
            entry
                .line
                .iter()
                .flatten()
                .filter_map(|l| {
                    l.start
                        .filter(|s| s.is_finite())
                        .map(|s| (shift_entry(s / 1000.0), l.value.clone().unwrap_or_default()))
                })
                .collect::<Vec<_>>(),
        )
    };
    attach_supporting(&mut lines, supporting("translation"), true);
    attach_supporting(&mut lines, supporting("pronunciation"), false);
    let synced = main.synced == Some(true) && lines.iter().any(|l| l.start.is_some());
    let language = main
        .lang
        .clone()
        .filter(|l| !l.is_empty() && l != "und" && l != "xxx");
    Some(Lyrics {
        lines,
        synced,
        language,
        word_by_word: Some(word_by_word),
    })
}

/* ---------------------------------------------------------------- LRCLIB -- */

static NON_ALNUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}]+").unwrap());
static ARTIST_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new("[;\u{0}•]").unwrap());
static FEATURING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s+(?:feat\.?|ft\.?|featuring|with)\s+").unwrap());

fn normalize_lrclib_value(value: &str) -> String {
    NON_ALNUM.replace_all(&value.to_lowercase(), "").into_owned()
}

/// The primary artist LRCLIB indexes a track under.
pub fn lrclib_artist_name(value: &str) -> String {
    let first = ARTIST_SPLIT.split(value).next().unwrap_or("");
    FEATURING.split(first).next().unwrap_or("").trim().to_string()
}

fn lrclib_value_matches(left: &str, right: &str) -> bool {
    let a = normalize_lrclib_value(left);
    let b = normalize_lrclib_value(right);
    !a.is_empty() && !b.is_empty() && (a == b || a.contains(&b) || b.contains(&a))
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LrclibRecord {
    pub track_name: Option<String>,
    pub artist_name: Option<String>,
    pub album_name: Option<String>,
    pub duration: Option<f64>,
    pub plain_lyrics: Option<String>,
    pub synced_lyrics: Option<String>,
    pub instrumental: Option<bool>,
}

/// Versions of a track this close in length count as the same recording.
const LRCLIB_DURATION_TOLERANCE: f64 = 3.0;

/// Pick the synced LRCLIB search result that best fits the song.
pub fn pick_synced_lrclib_result(records: &serde_json::Value, song: &Song) -> Option<Lyrics> {
    let items = records.as_array()?;
    let title = normalize_lrclib_value(&song.title);
    let artist = normalize_lrclib_value(&lrclib_artist_name(
        song.album_artist
            .as_deref()
            .filter(|s| !s.is_empty())
            .or(song.artist.as_deref())
            .unwrap_or(""),
    ));
    let duration = song.duration.unwrap_or(0.0);
    let mut candidates: Vec<LrclibRecord> = items
        .iter()
        .filter(|v| v.is_object())
        .filter_map(|v| serde_json::from_value::<LrclibRecord>(v.clone()).ok())
        .filter(|item| {
            lrclib_value_matches(item.track_name.as_deref().unwrap_or(""), &title)
                && lrclib_value_matches(item.artist_name.as_deref().unwrap_or(""), &artist)
                && item
                    .synced_lyrics
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .and_then(parse_lrc)
                    .is_some()
        })
        .collect();
    let distance = |item: &LrclibRecord| match item.duration {
        Some(d) if duration > 0.0 => (d - duration).abs(),
        _ => f64::INFINITY,
    };
    // Native ranking (the web sorts by duration alone): an exact title beats
    // a containing one ("Memory Merge" vs "Memory Merge (Paused)"), and among
    // versions within a few seconds of the song, word-timed lyrics win; then
    // the closest duration.
    let exact = |item: &LrclibRecord| normalize_lrclib_value(item.track_name.as_deref().unwrap_or("")) == title;
    let close = |item: &LrclibRecord| distance(item) <= LRCLIB_DURATION_TOLERANCE;
    let word_timed = |item: &LrclibRecord| {
        item.synced_lyrics.as_deref().and_then(parse_lrc).is_some_and(|l| l.word_by_word == Some(true))
    };
    let rank = |item: &LrclibRecord| (!exact(item), !close(item), !word_timed(item));
    candidates.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then(distance(a).partial_cmp(&distance(b)).unwrap_or(std::cmp::Ordering::Equal))
    });
    candidates.first()?.synced_lyrics.as_deref().and_then(parse_lrc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lrclib_prefers_exact_title_and_word_timing() {
        let records = json!([
            {"trackName": "Memory Merge (Paused)", "artistName": "YonKaGor", "duration": 272.608, "syncedLyrics": "[00:31.56] Did it really happen"},
            {"trackName": "Memory Merge", "artistName": "YonKaGor", "duration": 273.0, "syncedLyrics": "[00:31.56] Did it really happen"},
            {"trackName": "Memory Merge", "artistName": "YonKaGor", "duration": 273.0, "syncedLyrics": "[00:31.61]<00:31.61>Did <00:31.96>it"},
            {"trackName": "Memory Merge", "artistName": "YonKaGor", "duration": 300.0, "syncedLyrics": "[00:31.61]<00:31.61>Far <00:31.96>off"}
        ]);
        let song = Song { title: "Memory Merge".into(), artist: Some("YonKaGor".into()), duration: Some(272.6087), ..Default::default() };
        let picked = pick_synced_lrclib_result(&records, &song).unwrap();
        assert_eq!(picked.word_by_word, Some(true));
        assert_eq!(picked.lines[0].text, "Did it");
    }

    fn word(text: &str, start: f64, end: f64, space: bool) -> LyricsWord {
        make_word(text, start, end, space, false)
    }

    // "clipboard lyrics support synced LRC and plain text"
    #[test]
    fn clipboard_lrc_and_plain() {
        let synced = parse_lyrics_text("[00:01.50]First line\n[00:03]Second line").unwrap();
        assert!(synced.synced);
        assert_eq!(
            synced.lines[0],
            LyricsLine {
                text: "First line".into(),
                start: Some(1.5),
                ..Default::default()
            }
        );
        let plain = parse_lyrics_text("First line\nSecond line").unwrap();
        assert_eq!(
            serde_json::to_value(&plain).unwrap(),
            json!({"synced": false, "lines": [{"text": "First line"}, {"text": "Second line"}]})
        );
    }

    // "word-level timing parses from enhanced LRC and cue lines"
    #[test]
    fn enhanced_lrc_words() {
        let lrc = parse_lyrics_text("[00:10.00]<00:10.00>Hold <00:10.60>the <00:11.20>line<00:12.00>").unwrap();
        assert_eq!(lrc.word_by_word, Some(true));
        assert_eq!(
            lrc.lines[0],
            LyricsLine {
                text: "Hold the line".into(),
                start: Some(10.0),
                words: Some(vec![
                    word("Hold", 10.0, 10.6, true),
                    word("the", 10.6, 11.2, true),
                    word("line", 11.2, 12.0, false),
                ]),
                ..Default::default()
            }
        );
    }

    #[test]
    fn structured_cue_lines() {
        let entries: Vec<StructuredLyrics> = serde_json::from_value(json!([{
            "synced": true,
            "line": [{"start": 1000, "value": "Hello echo"}],
            "agents": [{"id": "lead", "role": "main"}, {"id": "backing", "role": "bg"}],
            "cueLine": [
                {"index": 0, "agentId": "lead", "start": 1000, "end": 3000, "value": "Hello",
                 "cue": [{"start": 1000, "end": 1800, "value": "He"}, {"start": 1800, "end": 3000, "value": "llo"}]},
                {"index": 0, "agentId": "backing", "start": 1000, "end": 3000, "value": "echo",
                 "cue": [{"start": 2000, "end": 2600, "value": "echo"}]}
            ]
        }]))
        .unwrap();
        let structured = lyrics_from_structured(&entries).unwrap();
        assert_eq!(structured.word_by_word, Some(true));
        assert_eq!(structured.lines[0].start, Some(1.0));
        assert_eq!(
            structured.lines[0].words,
            Some(vec![word("He", 1.0, 1.8, false), word("llo", 1.8, 3.0, false)])
        );
        assert_eq!(
            structured.lines[0].backing,
            Some(vec![make_word("echo", 2.0, 2.6, false, true)])
        );
    }

    const WORD_TTML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<tt xmlns="http://www.w3.org/ns/ttml"
    xmlns:itunes="http://music.apple.com/lyric-ttml-internal"
    xmlns:ttm="http://www.w3.org/ns/ttml#metadata"
    itunes:timing="Word" xml:lang="en">
  <body dur="00:12.000">
    <div begin="00:00.000" end="00:12.000" itunes:songPart="verse">
      <p begin="00:00.000" end="00:06.000" itunes:key="L1" ttm:agent="v1">
        <span begin="00:00.000" end="00:01.400">Hold </span>
        <span begin="00:01.400" end="00:02.200">the </span>
        <span begin="00:02.200" end="00:06.000">line</span>
      </p>
      <p begin="00:06.000" end="00:12.000" itunes:key="L2" ttm:agent="v2">
        <span begin="00:06.000" end="00:08.000">Answer </span>
        <span begin="00:08.000" end="00:12.000">me</span>
      </p>
    </div>
  </body>
</tt>"#;

    // "TTML parses into word offsets and duet alignment"
    #[test]
    fn ttml_words_and_duet() {
        let parsed = parse_ttml(WORD_TTML).unwrap();
        assert_eq!(parsed.word_by_word, Some(true));
        assert_eq!(parsed.language.as_deref(), Some("en"));
        assert_eq!(
            parsed.lines,
            vec![
                LyricsLine {
                    text: "Hold the line".into(),
                    start: Some(0.0),
                    end: Some(6.0),
                    words: Some(vec![
                        word("Hold", 0.0, 1.4, true),
                        word("the", 1.4, 2.2, true),
                        word("line", 2.2, 6.0, false),
                    ]),
                    ..Default::default()
                },
                LyricsLine {
                    text: "Answer me".into(),
                    start: Some(6.0),
                    end: Some(12.0),
                    align: Some(Align::Right),
                    words: Some(vec![word("Answer", 6.0, 8.0, true), word("me", 8.0, 12.0, false)]),
                    ..Default::default()
                },
            ]
        );
    }

    // "LRCLIB matches featured artists and annotated bilingual titles"
    #[test]
    fn lrclib_featured_and_bilingual() {
        let heart = Song {
            id: "heart".into(),
            title: "The Heart Pt. 2 (Feat. Dash Snow)".into(),
            artist: Some("Kendrick Lamar feat. Dash Snow".into()),
            duration: Some(294.0),
            ..Default::default()
        };
        let lyrics = pick_synced_lrclib_result(
            &json!([
                {"trackName": "The Heart, Pt. 2 (feat. Dash Snow)", "artistName": "Kendrick Lamar",
                 "duration": 294, "syncedLyrics": "[00:00.00] Heart lyrics"},
                {"trackName": "惑星ウェブスター [Planet Webstar}", "artistName": "Atena",
                 "duration": 163, "syncedLyrics": "[00:08.14] Planet lyrics"}
            ]),
            &heart,
        )
        .unwrap();
        assert_eq!(
            lyrics.lines[0],
            LyricsLine { text: "Heart lyrics".into(), start: Some(0.0), ..Default::default() }
        );
        let planet = Song {
            id: "planet".into(),
            title: "Planet Webstar".into(),
            artist: Some("Atena • Aoki Lapis • Merli".into()),
            duration: Some(163.0),
            ..Default::default()
        };
        let lyrics = pick_synced_lrclib_result(
            &json!([{"trackName": "惑星ウェブスター [Planet Webstar}", "artistName": "Atena",
                     "duration": 163, "syncedLyrics": "[00:08.14] Planet lyrics"}]),
            &planet,
        )
        .unwrap();
        assert_eq!(
            lyrics.lines[0],
            LyricsLine { text: "Planet lyrics".into(), start: Some(8.14), ..Default::default() }
        );
    }

    #[test]
    fn repeated_stamps_drop_word_times_and_sort() {
        let lyrics = parse_lrc("[00:05.00][00:01.00]<00:01.00>A <00:02.00>B").unwrap();
        assert_eq!(lyrics.lines.len(), 2);
        assert_eq!(lyrics.lines[0].start, Some(1.0));
        assert!(lyrics.lines[0].words.is_none());
        assert_eq!(lyrics.word_by_word, Some(false));
    }
}
