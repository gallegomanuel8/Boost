use serde::{Deserialize, Serialize};

pub use raudo_proto::Request;

/// Byte range `[start, end)` owned by at most one connection.
/// `done` bytes from `start` are already on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub start: u64,
    pub end: u64,
    pub done: u64,
}

impl Segment {
    pub fn pos(&self) -> u64 {
        self.start + self.done
    }

    pub fn remaining(&self) -> u64 {
        self.end.saturating_sub(self.pos())
    }

    pub fn is_complete(&self) -> bool {
        self.pos() >= self.end
    }
}

/// Persistable progress of one download. Storage is the caller's job
/// (JSON sidecar in the CLI, SQLite in the daemon).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    pub url: String,
    /// Final size; `None` when the server did not announce it.
    pub size: Option<u64>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// Server honours `Range`, so several connections and resume are possible.
    pub resumable: bool,
    pub filename: Option<String>,
    pub segments: Vec<Segment>,
}

impl State {
    pub fn downloaded(&self) -> u64 {
        self.segments.iter().map(|s| s.done.min(s.end - s.start)).sum()
    }

    pub fn is_complete(&self) -> bool {
        self.size.is_some() && !self.segments.is_empty() && self.segments.iter().all(Segment::is_complete)
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub max_connections: usize,
    /// Never create a segment smaller than this (IDM uses 23 363 bytes).
    pub min_split: u64,
    /// Seconds of expected progress subtracted when ranking segments to split.
    pub split_horizon_secs: f64,
    /// Global cap in bytes/s, `None` = unlimited.
    pub speed_limit: Option<u64>,
    pub max_retries: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_connections: 8,
            min_split: 24 * 1024,
            split_horizon_secs: 1.0,
            speed_limit: None,
            max_retries: 8,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    /// First response parsed: size, name, resumability are known.
    Started(State),
    Progress {
        downloaded: u64,
        total: Option<u64>,
        bytes_per_sec: f64,
        connections: usize,
    },
    /// Periodic copy of the state for persistence.
    Snapshot(State),
    /// Connection-level notice (retry, fallback to single connection, ...).
    Notice(String),
}
