//! Types and D-Bus interface shared by the daemon and its clients (CLI, GTK, nmhost).
//!
//! Structured values cross D-Bus as JSON strings: D-Bus has no optional fields,
//! and every client already speaks serde.

use serde::{Deserialize, Serialize};

pub const BUS_NAME: &str = "org.raudo.Manager";
pub const OBJECT_PATH: &str = "/org/raudo/Manager";
pub const INTERFACE: &str = "org.raudo.Manager1";

/// What to fetch and with which browser context (cookies, referer, UA).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Request {
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub cookies: Option<String>,
    #[serde(default)]
    pub referer: Option<String>,
    #[serde(default)]
    pub user_agent: Option<String>,
}

/// Argument of `Add`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AddDownload {
    pub request: Request,
    /// Target directory; `None` = user's download folder.
    #[serde(default)]
    pub dir: Option<String>,
    /// Exact file name instead of the one announced by the server.
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub paused: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Downloading,
    Paused,
    Completed,
    Error,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Queued => "queued",
            Status::Downloading => "downloading",
            Status::Paused => "paused",
            Status::Completed => "completed",
            Status::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => Status::Queued,
            "downloading" => Status::Downloading,
            "paused" => Status::Paused,
            "completed" => Status::Completed,
            "error" => Status::Error,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadInfo {
    pub id: i64,
    pub url: String,
    pub dir: String,
    pub filename: Option<String>,
    pub status: Status,
    pub size: Option<u64>,
    pub downloaded: u64,
    /// Bytes per second, only meaningful while downloading.
    pub speed: f64,
    pub connections: usize,
    pub error: Option<String>,
    pub queue: i64,
}

#[zbus::proxy(interface = "org.raudo.Manager1", default_service = "org.raudo.Manager", default_path = "/org/raudo/Manager")]
pub trait Manager {
    /// `download`: JSON `AddDownload`. Returns the new id.
    fn add(&self, download: &str) -> zbus::Result<i64>;
    /// JSON array of `DownloadInfo`.
    fn list(&self) -> zbus::Result<String>;
    fn pause(&self, id: i64) -> zbus::Result<()>;
    fn resume(&self, id: i64) -> zbus::Result<()>;
    /// Removes the entry; `delete_file` also deletes a completed file.
    fn remove(&self, id: i64, delete_file: bool) -> zbus::Result<()>;
    /// KiB/s, 0 = unlimited.
    fn set_speed_limit(&self, kib_per_sec: u64) -> zbus::Result<()>;
    fn set_max_concurrent(&self, n: u32) -> zbus::Result<()>;

    /// JSON `DownloadInfo`, emitted on every state change and periodically while downloading.
    #[zbus(signal)]
    fn changed(&self, info: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn removed(&self, id: i64) -> zbus::Result<()>;
}
