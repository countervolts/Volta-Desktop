//! The periodic state snapshot in `performance.log`: what the app is holding
//! next to how big the library is, so memory and slowness can be read against
//! library size without owning a library that big.

use gpui::Context;

use crate::app_state::AppState;
use crate::logs::PERF;

impl AppState {
    pub fn log_snapshot(&self, cx: &mut Context<Self>) {
        let route = self.route();
        let library = if let Some(local) = self.local.library.as_ref().filter(|_| self.is_local()) {
            format!("local {} tracks / {} albums / {} artists", local.tracks.len(), local.albums.len(), local.artists.len())
        } else if let Some((_, Ok(stats))) = &self.library_stats {
            format!("server {} songs / {} albums / {} artists", stats.songs, stats.albums, stats.artists)
        } else {
            "size not counted yet".to_string()
        };
        let player = self.player.read(cx);
        log::info!(
            target: PERF,
            "state: route={}{} | library: {library} | page: {} songs, {} albums, {} artists | playlists={} history_events={} remembered_pages={} | covers decoded={} ({:.1} MB) | queue={} underruns={} | connection={:?}",
            route.page.name(),
            route.id.as_deref().map(|id| format!(":{id}")).unwrap_or_default(),
            self.page.songs.len(),
            self.page.albums.len(),
            self.page.artists.len(),
            self.playlists.len(),
            self.listening_events.len(),
            self.page_memory.len(),
            self.covers.len(),
            self.covers.bytes() as f64 / 1024.0 / 1024.0,
            player.queue.len(),
            player.underruns(),
            self.connection,
        );
    }
}
