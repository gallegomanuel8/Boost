use std::path::Path;

use raudo_core::State;
use raudo_proto::{AddDownload, DownloadInfo, Request, Status, VideoOptions};
use rusqlite::{params, Connection, OptionalExtension, Row};

pub struct Db {
    conn: Connection,
}

/// Everything needed to (re)start one download.
pub struct Stored {
    pub request: Request,
    pub dir: String,
    pub filename: Option<String>,
    pub state: Option<State>,
    pub video: Option<VideoOptions>,
    /// Name requested by the client (page title for videos).
    pub title: Option<String>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS queues (
    id             INTEGER PRIMARY KEY,
    name           TEXT NOT NULL,
    max_concurrent INTEGER NOT NULL DEFAULT 3
);
INSERT OR IGNORE INTO queues (id, name, max_concurrent) VALUES (1, 'main', 3);

CREATE TABLE IF NOT EXISTS downloads (
    id          INTEGER PRIMARY KEY,
    url         TEXT NOT NULL,
    request     TEXT NOT NULL,            -- JSON Request
    dir         TEXT NOT NULL,
    filename    TEXT,
    fixed_name  INTEGER NOT NULL DEFAULT 0,
    status      TEXT NOT NULL,
    size        INTEGER,
    downloaded  INTEGER NOT NULL DEFAULT 0,
    state       TEXT,                     -- JSON engine State, for resume
    error       TEXT,
    queue_id    INTEGER NOT NULL DEFAULT 1 REFERENCES queues(id),
    created_at  INTEGER NOT NULL DEFAULT (unixepoch()),
    finished_at INTEGER
);
CREATE INDEX IF NOT EXISTS downloads_status ON downloads(status, id);
";

/// Applied in order; `PRAGMA user_version` records how many have run. Append only.
const MIGRATIONS: &[&str] = &[
    // 1: video downloads through yt-dlp
    "ALTER TABLE downloads ADD COLUMN video INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE downloads ADD COLUMN format TEXT;",
    // 2: requested name, kept apart from `filename`, which tracks what is on disk
    "ALTER TABLE downloads ADD COLUMN title TEXT;",
    // 3: audio-only (MP3) downloads
    "ALTER TABLE downloads ADD COLUMN audio_kbps INTEGER;",
];

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        // WAL + NORMAL: durable across application crashes, cheap per-second updates.
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(SCHEMA)?;
        let version: usize = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, m) in MIGRATIONS.iter().enumerate().skip(version) {
            conn.execute_batch(&format!("BEGIN; {m} PRAGMA user_version = {}; COMMIT;", i + 1))?;
        }
        Ok(Self { conn })
    }

    /// Downloads interrupted by a daemon crash or shutdown go back to the queue.
    pub fn requeue_interrupted(&self) -> rusqlite::Result<usize> {
        self.conn.execute("UPDATE downloads SET status = 'queued' WHERE status = 'downloading'", [])
    }

    pub fn insert(&self, add: &AddDownload, dir: &str) -> rusqlite::Result<i64> {
        let request = serde_json::to_string(&add.request).expect("serializable");
        let status = if add.paused { Status::Paused } else { Status::Queued };
        self.conn.execute(
            "INSERT INTO downloads (url, request, dir, filename, fixed_name, status, video, format, title, audio_kbps)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                add.request.url,
                request,
                dir,
                add.filename,
                add.filename.is_some(),
                status.as_str(),
                add.video.is_some(),
                add.video.as_ref().and_then(|v| v.format.clone()),
                add.title.as_ref().or(add.filename.as_ref()),
                add.video.as_ref().and_then(|v| v.audio_kbps),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn stored(&self, id: i64) -> rusqlite::Result<Option<Stored>> {
        self.conn
            .query_row(
                "SELECT request, dir, filename, fixed_name, state, video, format, title, audio_kbps
                 FROM downloads WHERE id = ?1",
                [id],
                |r| {
                    let request: String = r.get(0)?;
                    let fixed: bool = r.get(3)?;
                    let state: Option<String> = r.get(4)?;
                    let video: bool = r.get(5)?;
                    Ok(Stored {
                        request: serde_json::from_str(&request).unwrap_or_default(),
                        dir: r.get(1)?,
                        filename: if fixed { r.get(2)? } else { None },
                        state: state.and_then(|s| serde_json::from_str(&s).ok()),
                        video: video.then(|| VideoOptions {
                            format: r.get(6).ok().flatten(),
                            audio_kbps: r.get(8).ok().flatten(),
                        }),
                        title: r.get(7)?,
                    })
                },
            )
            .optional()
    }

    pub fn info(&self, id: i64) -> rusqlite::Result<Option<DownloadInfo>> {
        self.conn
            .query_row(&format!("{SELECT_INFO} WHERE id = ?1"), [id], info_from_row)
            .optional()
    }

    pub fn list(&self) -> rusqlite::Result<Vec<DownloadInfo>> {
        let mut stmt = self.conn.prepare(&format!("{SELECT_INFO} ORDER BY id"))?;
        let rows = stmt.query_map([], info_from_row)?;
        rows.collect()
    }

    pub fn status(&self, id: i64) -> rusqlite::Result<Option<Status>> {
        self.conn
            .query_row("SELECT status FROM downloads WHERE id = ?1", [id], |r| r.get::<_, String>(0))
            .optional()
            .map(|s| s.and_then(|s| Status::parse(&s)))
    }

    pub fn set_status(&self, id: i64, status: Status, error: Option<&str>) -> rusqlite::Result<()> {
        let finished = matches!(status, Status::Completed);
        self.conn.execute(
            "UPDATE downloads SET status = ?2, error = ?3,
                 finished_at = CASE WHEN ?4 THEN unixepoch() ELSE finished_at END
             WHERE id = ?1",
            params![id, status.as_str(), error, finished],
        )?;
        Ok(())
    }

    /// Engine progress: what the next run resumes from.
    pub fn save_state(&self, id: i64, state: &State) -> rusqlite::Result<()> {
        let json = serde_json::to_string(state).expect("serializable");
        self.conn.execute(
            "UPDATE downloads SET state = ?2, size = ?3, downloaded = ?4, filename = COALESCE(?5, filename)
             WHERE id = ?1",
            params![id, json, state.size.map(|s| s as i64), state.downloaded() as i64, state.filename],
        )?;
        Ok(())
    }

    /// Progress of a yt-dlp download (no engine state to keep).
    pub fn save_progress(&self, id: i64, downloaded: u64, size: Option<u64>) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE downloads SET downloaded = ?2, size = COALESCE(?3, size) WHERE id = ?1",
            params![id, downloaded as i64, size.map(|s| s as i64)],
        )?;
        Ok(())
    }

    pub fn set_filename(&self, id: i64, name: &str) -> rusqlite::Result<()> {
        self.conn.execute("UPDATE downloads SET filename = ?2 WHERE id = ?1", params![id, name])?;
        Ok(())
    }

    pub fn clear_state(&self, id: i64) -> rusqlite::Result<()> {
        self.conn.execute("UPDATE downloads SET state = NULL, downloaded = 0 WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM downloads WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Queued downloads in order, per queue, with each queue's concurrency limit.
    pub fn queued(&self) -> rusqlite::Result<Vec<(i64, i64, u32)>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.id, d.queue_id, q.max_concurrent FROM downloads d JOIN queues q ON q.id = d.queue_id
             WHERE d.status = 'queued' ORDER BY d.id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }

    pub fn queue_of(&self, id: i64) -> rusqlite::Result<i64> {
        self.conn.query_row("SELECT queue_id FROM downloads WHERE id = ?1", [id], |r| r.get(0))
    }

    pub fn set_max_concurrent(&self, queue: i64, n: u32) -> rusqlite::Result<()> {
        self.conn.execute("UPDATE queues SET max_concurrent = ?2 WHERE id = ?1", params![queue, n])?;
        Ok(())
    }
}

const SELECT_INFO: &str =
    "SELECT id, url, dir, filename, status, size, downloaded, error, queue_id, video FROM downloads";

fn info_from_row(r: &Row) -> rusqlite::Result<DownloadInfo> {
    let status: String = r.get(4)?;
    Ok(DownloadInfo {
        id: r.get(0)?,
        url: r.get(1)?,
        dir: r.get(2)?,
        filename: r.get(3)?,
        status: Status::parse(&status).unwrap_or(Status::Error),
        size: r.get::<_, Option<i64>>(5)?.map(|s| s as u64),
        downloaded: r.get::<_, i64>(6)? as u64,
        speed: 0.0,
        connections: 0,
        error: r.get(7)?,
        queue: r.get(8)?,
        video: r.get(9)?,
    })
}
