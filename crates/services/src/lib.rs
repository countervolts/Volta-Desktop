//! I/O services for the Volta desktop app.

pub mod local;
pub mod lrclib;
pub mod metrics;
pub mod musicbrainz;
pub mod navidrome;
pub mod share;
pub mod storage;
pub mod update;

pub use navidrome::{Credentials, Navidrome, NavidromeError, SearchResults, PlaylistChanges};
pub use storage::Storage;
