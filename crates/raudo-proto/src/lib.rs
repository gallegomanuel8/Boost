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
    /// Page the media was found on. If the stream link fails (tokens bound to the
    /// browser's IP, e.g. behind a VPN), the daemon resolves a fresh one from it.
    #[serde(default)]
    pub page: Option<String>,
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
    /// Name hint (page title): used for videos, and for files whose server name looks random.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub paused: bool,
    /// Download through yt-dlp (video pages, HLS/DASH manifests) instead of plain HTTP.
    #[serde(default)]
    pub video: Option<VideoOptions>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VideoOptions {
    /// yt-dlp format selector (`-f`), e.g. a format id from `Formats` or `bv*+ba/b`.
    #[serde(default)]
    pub format: Option<String>,
    /// Keep only the audio track, converted to MP3 at this bitrate in kbit/s (0 = best VBR).
    #[serde(default)]
    pub audio_kbps: Option<u32>,
}

/// Result of `Formats`: what yt-dlp can offer for a page.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaInfo {
    pub title: String,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
    pub formats: Vec<MediaFormat>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaFormat {
    pub id: String,
    pub ext: String,
    pub resolution: Option<String>,
    pub fps: Option<f64>,
    /// `None` when the format carries no video (audio only).
    pub vcodec: Option<String>,
    /// `None` when the format carries no audio (video only).
    pub acodec: Option<String>,
    /// Exact or estimated size in bytes.
    pub filesize: Option<u64>,
    /// Total bitrate in kbit/s.
    pub tbr: Option<f64>,
    pub note: Option<String>,
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
    pub video: bool,
    /// Unix seconds.
    #[serde(default)]
    pub created_at: i64,
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
    /// `request`: JSON `Request` of a video page. Returns JSON `MediaInfo`.
    fn formats(&self, request: &str) -> zbus::Result<String>;
    /// `target`: "page" → `{supported, via}`; "hls" → stream info (quality,
    /// duration, container, variant URLs). Returns JSON.
    fn inspect(&self, target: &str, request: &str) -> zbus::Result<String>;

    /// JSON `DownloadInfo`, emitted on every state change and periodically while downloading.
    #[zbus(signal)]
    fn changed(&self, info: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn removed(&self, id: i64) -> zbus::Result<()>;
}
