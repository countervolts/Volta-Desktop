//! Port of `src/lib/listening-stats.ts`: period summaries and timelines for
//! the Stats view. Dates are local time; labels follow en-US `Intl` output.

use std::collections::{BTreeSet, HashSet};

use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone, Timelike};
use indexmap::IndexMap;

use crate::listening_history::{EventKind, ListeningEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsPeriod {
    Day,
    Week,
    Month,
    Year,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelinePoint {
    pub key: String,
    pub label: String,
    pub plays: u32,
    pub seconds: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RankedItem {
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
    pub cover_art: Option<String>,
    pub image_url: Option<String>,
    pub plays: u32,
    pub seconds: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatsSnapshot {
    pub active_days: usize,
    pub average_listen_seconds: f64,
    pub first_played_at: f64,
    pub last_played_at: f64,
    pub longest_streak: usize,
    pub current: bool,
    pub range_label: String,
    pub sessions: usize,
    pub timeline: Vec<TimelinePoint>,
    pub top_albums: Vec<RankedItem>,
    pub top_artists: Vec<RankedItem>,
    pub top_genres: Vec<RankedItem>,
    pub top_songs: Vec<RankedItem>,
    pub total_plays: usize,
    pub total_seconds: f64,
    pub unique_albums: usize,
    pub unique_artists: usize,
    pub unique_songs: usize,
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const MONTHS_LONG: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const WEEKDAYS_LONG: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

fn date_key(date: NaiveDate) -> String {
    format!("{}-{:02}-{:02}", date.year(), date.month(), date.day())
}

fn local_date(at: f64) -> chrono::DateTime<Local> {
    Local.timestamp_millis_opt(at as i64).single().unwrap_or_else(Local::now)
}

fn millis(date: NaiveDate) -> f64 {
    date.and_hms_opt(0, 0, 0)
        .and_then(|d| d.and_local_timezone(Local).earliest())
        .map(|d| d.timestamp_millis() as f64)
        .unwrap_or(0.0)
}

fn add_months(date: NaiveDate, months: i32) -> NaiveDate {
    let total = date.year() * 12 + date.month0() as i32 + months;
    NaiveDate::from_ymd_opt(total.div_euclid(12), total.rem_euclid(12) as u32 + 1, 1).unwrap_or(date)
}

fn add_period(date: NaiveDate, period: StatsPeriod, amount: i32) -> NaiveDate {
    match period {
        StatsPeriod::Day => date + Duration::days(amount as i64),
        StatsPeriod::Week => date + Duration::days(amount as i64 * 7),
        StatsPeriod::Month => add_months(date, amount),
        StatsPeriod::Year => NaiveDate::from_ymd_opt(date.year() + amount, 1, 1).unwrap_or(date),
    }
}

fn format_hour(hour: u32) -> String {
    match hour {
        0 => "12a".into(),
        h if h < 12 => format!("{h}a"),
        12 => "12p".into(),
        h => format!("{}p", h - 12),
    }
}

fn month_day(date: NaiveDate) -> String {
    format!("{} {}", MONTHS[date.month0() as usize], date.day())
}

struct Range {
    start: NaiveDate,
    end: NaiveDate,
    label: String,
    timeline: Vec<(String, String)>,
}

fn period_range(period: StatsPeriod, offset: i32, now: NaiveDate) -> Range {
    let baseline = match period {
        StatsPeriod::Day => now,
        StatsPeriod::Week => now - Duration::days(now.weekday().num_days_from_monday() as i64),
        StatsPeriod::Month => NaiveDate::from_ymd_opt(now.year(), now.month(), 1).unwrap(),
        StatsPeriod::Year => NaiveDate::from_ymd_opt(now.year(), 1, 1).unwrap(),
    };
    let start = add_period(baseline, period, offset);
    let end = add_period(start, period, 1);
    match period {
        StatsPeriod::Day => Range {
            start,
            end,
            label: format!(
                "{}, {} {}, {}",
                WEEKDAYS_LONG[start.weekday().num_days_from_sunday() as usize],
                MONTHS_LONG[start.month0() as usize],
                start.day(),
                start.year()
            ),
            timeline: (0..24).map(|h| (h.to_string(), format_hour(h))).collect(),
        },
        StatsPeriod::Week => {
            let last = end - Duration::days(1);
            Range {
                start,
                end,
                label: format!("{} – {}, {}", month_day(start), month_day(last), last.year()),
                timeline: (0..7).map(|i| (date_key(start + Duration::days(i)), WEEKDAYS[i as usize].to_string())).collect(),
            }
        }
        StatsPeriod::Month => {
            let days = (end - start).num_days();
            Range {
                start,
                end,
                label: format!("{} {}", MONTHS_LONG[start.month0() as usize], start.year()),
                timeline: (0..days).map(|i| (date_key(start + Duration::days(i)), (i + 1).to_string())).collect(),
            }
        }
        StatsPeriod::Year => Range {
            start,
            end,
            label: start.year().to_string(),
            timeline: MONTHS.iter().enumerate().map(|(i, m)| (i.to_string(), m.to_string())).collect(),
        },
    }
}

fn normalized_key(value: &str) -> String {
    value.trim().to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn increase(target: &mut IndexMap<String, RankedItem>, id: &str, label: &str, detail: Option<String>, art: (Option<String>, Option<String>)) {
    if let Some(existing) = target.get_mut(id) {
        existing.plays += 1;
        if existing.cover_art.is_none() && art.0.is_some() {
            existing.cover_art = art.0;
        }
        if existing.image_url.is_none() && art.1.is_some() {
            existing.image_url = art.1;
        }
        return;
    }
    target.insert(
        id.to_string(),
        RankedItem { id: id.to_string(), label: label.to_string(), detail, cover_art: art.0, image_url: art.1, plays: 1, seconds: 0.0 },
    );
}

/// Stable sort by plays then seconds, top 8.
fn rank(items: IndexMap<String, RankedItem>) -> Vec<RankedItem> {
    let mut list: Vec<RankedItem> = items.into_values().collect();
    list.sort_by(|a, b| b.plays.cmp(&a.plays).then(b.seconds.partial_cmp(&a.seconds).unwrap_or(std::cmp::Ordering::Equal)));
    list.truncate(8);
    list
}

fn longest_streak(dates: &BTreeSet<String>) -> usize {
    let parsed: Vec<NaiveDate> = dates.iter().filter_map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).collect();
    if parsed.is_empty() {
        return 0;
    }
    let (mut best, mut current) = (1, 1);
    for pair in parsed.windows(2) {
        if (pair[1] - pair[0]).num_days() == 1 {
            current += 1;
            best = best.max(current);
        } else {
            current = 1;
        }
    }
    best
}

/// `summarizeListeningPeriod`.
pub fn summarize_listening_period(events: &[ListeningEvent], period: StatsPeriod, offset: i32, now: NaiveDate) -> StatsSnapshot {
    let range = period_range(period, offset, now);
    let (start_ms, end_ms) = (millis(range.start), millis(range.end));
    let in_period: Vec<&ListeningEvent> = events.iter().filter(|e| e.at >= start_ms && e.at < end_ms).collect();
    let plays: Vec<&&ListeningEvent> = in_period.iter().filter(|e| e.kind == EventKind::Start).collect();
    let (mut songs, mut artists, mut albums, mut genres) =
        (IndexMap::new(), IndexMap::new(), IndexMap::new(), IndexMap::<String, RankedItem>::new());
    let (mut song_seconds, mut artist_seconds, mut album_seconds) =
        (IndexMap::<String, f64>::new(), IndexMap::<String, f64>::new(), IndexMap::<String, f64>::new());
    let mut timeline: Vec<TimelinePoint> =
        range.timeline.iter().map(|(k, l)| TimelinePoint { key: k.clone(), label: l.clone(), plays: 0, seconds: 0.0 }).collect();
    let mut active = BTreeSet::new();
    let mut sessions = HashSet::new();
    let timeline_key = |event: &ListeningEvent| {
        let date = local_date(event.at);
        match period {
            StatsPeriod::Day => date.hour().to_string(),
            StatsPeriod::Week | StatsPeriod::Month => date_key(date.date_naive()),
            StatsPeriod::Year => date.month0().to_string(),
        }
    };
    for event in &in_period {
        let seconds = event.delta_seconds.max(0.0);
        let artist_label = event.artist.as_deref().map(str::trim).filter(|a| !a.is_empty()).unwrap_or("Unknown artist").to_string();
        let artist_id = event.artist_id.as_deref().map(str::trim).filter(|a| !a.is_empty()).map(str::to_string).unwrap_or_else(|| normalized_key(&artist_label));
        let album_label = event.album.as_deref().map(str::trim).filter(|a| !a.is_empty()).unwrap_or("Unknown album").to_string();
        let album_id = event.album_id.as_deref().map(str::trim).filter(|a| !a.is_empty()).map(str::to_string).unwrap_or_else(|| normalized_key(&album_label));
        *song_seconds.entry(event.song_id.clone()).or_insert(0.0) += seconds;
        *artist_seconds.entry(artist_id.clone()).or_insert(0.0) += seconds;
        *album_seconds.entry(album_id.clone()).or_insert(0.0) += seconds;
        let key = timeline_key(event);
        let slot = timeline.iter_mut().find(|p| p.key == key);
        let has_slot = slot.is_some();
        if let Some(slot) = slot {
            slot.seconds += seconds;
        }
        if event.kind != EventKind::Start {
            continue;
        }
        let art = (event.cover_art.clone(), event.image_url.clone());
        let detail = event.artist.clone().filter(|a| !a.is_empty());
        increase(&mut songs, &event.song_id, &event.title, detail.clone(), art.clone());
        increase(&mut artists, &artist_id, &artist_label, None, art.clone());
        increase(&mut albums, &album_id, &album_label, detail, art);
        for genre in event.genre.as_deref().unwrap_or("").split([',', ';', '|', '/']).map(str::trim).filter(|g| !g.is_empty()) {
            increase(&mut genres, &normalized_key(genre), genre, None, (None, None));
        }
        if has_slot
            && let Some(slot) = timeline.iter_mut().find(|p| p.key == key) {
                slot.plays += 1;
            }
        active.insert(date_key(local_date(event.at).date_naive()));
        sessions.insert(event.session_id.clone());
    }
    for (map, seconds) in [(&mut songs, &song_seconds), (&mut artists, &artist_seconds), (&mut albums, &album_seconds)] {
        for item in map.values_mut() {
            item.seconds = seconds.get(&item.id).copied().unwrap_or(0.0);
        }
    }
    let total_seconds: f64 = in_period.iter().map(|e| e.delta_seconds.max(0.0)).sum();
    let first = plays.iter().map(|e| e.at).fold(f64::INFINITY, f64::min);
    let last = plays.iter().map(|e| e.at).fold(f64::NEG_INFINITY, f64::max);
    let (unique_songs, unique_artists, unique_albums) = (songs.len(), artists.len(), albums.len());
    StatsSnapshot {
        active_days: active.len(),
        average_listen_seconds: if plays.is_empty() { 0.0 } else { total_seconds / plays.len() as f64 },
        first_played_at: if plays.is_empty() { 0.0 } else { first },
        last_played_at: if plays.is_empty() { 0.0 } else { last },
        longest_streak: longest_streak(&active),
        current: offset == 0,
        range_label: range.label,
        sessions: sessions.len(),
        timeline,
        top_albums: rank(albums),
        top_artists: rank(artists),
        top_genres: rank(genres),
        top_songs: rank(songs),
        total_plays: plays.len(),
        total_seconds,
        unique_albums,
        unique_artists,
        unique_songs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listening_history::{Daypart, HISTORY_VERSION};

    fn at(y: i32, m: u32, d: u32, h: u32) -> f64 {
        Local.with_ymd_and_hms(y, m, d, h, 0, 0).single().unwrap().timestamp_millis() as f64
    }

    fn event(id: &str, at: f64, kind: EventKind, song: &str, delta: f64) -> ListeningEvent {
        let third = song == "song-3";
        ListeningEvent {
            version: HISTORY_VERSION,
            id: id.into(),
            at,
            kind,
            session_id: if third { "session-2" } else { "session-1" }.into(),
            session_index: 0,
            song_id: song.into(),
            title: match song {
                "song-1" => "First Light",
                "song-2" => "Side Streets",
                _ => "Daybreak",
            }
            .into(),
            artist: Some(if third { "Nora Ellis" } else { "Miles & The City" }.into()),
            artist_id: Some(if third { "artist-2" } else { "artist-1" }.into()),
            album: Some(if third { "Soft Focus" } else { "Blue Hour" }.into()),
            album_id: Some(if third { "album-2" } else { "album-1" }.into()),
            cover_art: None,
            image_url: None,
            genre: Some(if third { "Alternative" } else { "Jazz" }.into()),
            duration_seconds: 180.0,
            delta_seconds: delta,
            total_seconds: delta,
            position_seconds: delta,
            completion_ratio: delta / 180.0,
            source: "navidrome".into(),
            queue_index: 0,
            shuffle: false,
            repeat: "off".into(),
            daypart: Daypart::Morning,
            hour: 9,
            weekday: 1,
            weekend: false,
            reason: None,
            seek_delta_seconds: None,
        }
    }

    #[test]
    fn plays_from_starts_time_from_audible_events() {
        let monday = at(2026, 9, 14, 9);
        let events = vec![
            event("start-1", monday, EventKind::Start, "song-1", 0.0),
            event("complete-1", monday + 180_000.0, EventKind::Complete, "song-1", 180.0),
            event("start-2", monday + 3_600_000.0, EventKind::Start, "song-2", 0.0),
            event("stop-2", monday + 3_720_000.0, EventKind::Stop, "song-2", 120.0),
            event("start-3", monday + 86_400_000.0, EventKind::Start, "song-3", 0.0),
            event("complete-3", monday + 86_580_000.0, EventKind::Complete, "song-3", 180.0),
        ];
        let stats = summarize_listening_period(&events, StatsPeriod::Week, 0, NaiveDate::from_ymd_opt(2026, 9, 16).unwrap());
        assert_eq!(stats.total_plays, 3);
        assert_eq!(stats.total_seconds, 480.0);
        assert_eq!((stats.unique_songs, stats.unique_artists, stats.unique_albums), (3, 2, 2));
        assert_eq!((stats.sessions, stats.active_days, stats.longest_streak), (2, 2, 2));
        assert_eq!(stats.timeline.iter().find(|d| d.label == "Mon").unwrap().plays, 2);
        assert_eq!(stats.timeline.iter().find(|d| d.label == "Tue").unwrap().plays, 1);
        assert_eq!((stats.top_songs[0].label.as_str(), stats.top_songs[0].plays, stats.top_songs[0].seconds), ("First Light", 1, 180.0));
        assert!(stats.top_genres.iter().any(|g| g.label == "Jazz" && g.plays == 2));
        assert!(stats.top_genres.iter().any(|g| g.label == "Alternative" && g.plays == 1));
        assert_eq!(stats.range_label, "Sep 14 – Sep 20, 2026");
    }

    #[test]
    fn period_boundaries_exclude_adjacent_data() {
        let events = vec![
            event("august", at(2026, 8, 31, 22), EventKind::Start, "song-1", 0.0),
            event("september", at(2026, 9, 1, 9), EventKind::Start, "song-2", 0.0),
        ];
        let now = NaiveDate::from_ymd_opt(2026, 9, 16).unwrap();
        let september = summarize_listening_period(&events, StatsPeriod::Month, 0, now);
        let august = summarize_listening_period(&events, StatsPeriod::Month, -1, now);
        assert!(september.range_label.contains("September"));
        assert_eq!(september.total_plays, 1);
        assert!(august.range_label.contains("August"));
        assert_eq!(august.total_plays, 1);
    }
}
