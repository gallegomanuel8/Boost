//! Segmented HTTP download engine.
//!
//! Strategy (taken from the analysis of IDM, see analisis/HALLAZGOS_IDMan.md §2):
//! * the first connection asks for `bytes=0-` and keeps streaming the whole file;
//! * whenever a connection slot is free, the active segment with the largest
//!   *estimated* remaining work (`remaining - speed * horizon`) is split in two
//!   and a new connection takes the tail;
//! * segments below `min_split` are never created;
//! * every extra connection must answer `206` with the exact `Content-Range`,
//!   otherwise the server is treated as not supporting ranges.
//!
//! Unlike IDM, all connections write into one preallocated file with positional
//! writes, so there is no "rebuild" step at the end.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, ETAG, IF_RANGE, LAST_MODIFIED,
};
use reqwest::{Client, Response, StatusCode};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tracing::{debug, warn};

use crate::http;
use crate::limiter::Limiter;
use crate::model::{Event, Options, Request, Segment, State};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("server answered {0}")]
    Status(StatusCode),
    #[error("the remote file changed since the download started")]
    RemoteChanged,
    #[error("connection failed after {0} retries: {1}")]
    RetriesExhausted(u32, String),
}

/// Where to save. With `Dir`, the name comes from the server or the URL.
#[derive(Debug, Clone)]
pub enum Dest {
    Dir(PathBuf),
    File(PathBuf),
}

pub struct Job {
    pub request: Request,
    pub dest: Dest,
    /// Previously saved state to continue from; its partial file must still exist.
    pub resume: Option<State>,
}

#[derive(Debug)]
pub enum Outcome {
    Completed { path: PathBuf, state: State },
    Paused(State),
}

pub const PART_SUFFIX: &str = ".raudo";

pub fn part_path(final_path: &Path) -> PathBuf {
    let mut s = final_path.as_os_str().to_owned();
    s.push(PART_SUFFIX);
    PathBuf::from(s)
}

struct Slot {
    seg: Segment,
    active: bool,
    /// Bytes per second of the connection working on it.
    speed: f64,
}

struct Ctx {
    client: Client,
    headers: HeaderMap,
    url: String,
    file: Arc<File>,
    limiter: Arc<Limiter>,
    slots: Mutex<Vec<Slot>>,
    /// Continuing a previous session: a full answer means the file changed.
    resuming: bool,
    /// `If-Range` validator, only sent when resuming.
    if_range: Option<HeaderValue>,
    max_retries: u32,
    cancel: watch::Receiver<bool>,
}

enum Exit {
    Done,
    Paused,
    /// Extra connection got a full (200) answer or a different range.
    RangeUnsupported,
    RemoteChanged,
    Failed(String),
}

pub async fn run(
    client: &Client,
    job: Job,
    opts: &Options,
    limiter: Arc<Limiter>,
    events: mpsc::UnboundedSender<Event>,
    cancel: watch::Receiver<bool>,
) -> Result<Outcome, Error> {
    let headers = http::build_headers(&job.request);
    let emit = |e: Event| {
        let _ = events.send(e);
    };

    // Name chosen by an earlier session of this job: its partial file is ours to overwrite.
    let prior_name = job.resume.as_ref().and_then(|s| s.filename.clone());
    let (mut state, final_path, first) = match job.resume {
        Some(st)
            if st.resumable
                && !st.segments.is_empty()
                && part_path(&resolve_final(&job.dest, st.filename.as_deref())).exists() =>
        {
            let path = resolve_final(&job.dest, st.filename.as_deref());
            (st, path, None)
        }
        _ => {
            let (st, resp) = probe(client, &job.request.url, &headers).await?;
            let path = match prior_name.map(|n| resolve_final(&job.dest, Some(&n))) {
                Some(p) if !p.exists() => p,
                _ => unique_path(resolve_final(&job.dest, st.filename.as_deref())),
            };
            let mut st = st;
            st.filename = path.file_name().map(|n| n.to_string_lossy().into_owned());
            (st, path, Some(resp))
        }
    };
    let part = part_path(&final_path);
    let file = open_part(&part, state.size, first.is_some())?;

    let resuming = first.is_none();
    let if_range = if resuming { validator(&state) } else { None };
    if state.segments.is_empty() {
        state.segments.push(Segment { start: 0, end: state.size.unwrap_or(u64::MAX), done: 0 });
    }
    emit(Event::Started(state.clone()));

    let ctx = Arc::new(Ctx {
        client: client.clone(),
        headers,
        url: job.request.url.clone(),
        file: Arc::new(file),
        limiter,
        slots: Mutex::new(
            state.segments.iter().map(|&seg| Slot { seg, active: false, speed: 0.0 }).collect(),
        ),
        resuming,
        if_range,
        max_retries: opts.max_retries,
        cancel: cancel.clone(),
    });

    let mut workers: JoinSet<(usize, Exit)> = JoinSet::new();
    let mut first = first;
    let mut resumable = state.resumable;
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut last_snapshot = Instant::now();
    let mut last_bytes = state.downloaded();
    let mut last_progress = Instant::now();
    let mut paused = false;

    loop {
        if !paused {
            spawn_workers(&ctx, &mut workers, &mut first, resumable, opts);
        }

        tokio::select! {
            Some(joined) = workers.join_next(), if !workers.is_empty() => {
                let (idx, exit) = joined.expect("worker panicked");
                ctx.slots.lock().unwrap()[idx].active = false;
                match exit {
                    Exit::Done => {}
                    Exit::Paused => paused = true,
                    Exit::RangeUnsupported => {
                        resumable = false;
                        give_back(&ctx, idx);
                        emit(Event::Notice("server ignores Range, using a single connection".into()));
                    }
                    Exit::RemoteChanged => {
                        stop_all(&mut workers).await;
                        return Err(Error::RemoteChanged);
                    }
                    Exit::Failed(msg) => {
                        stop_all(&mut workers).await;
                        state.segments = snapshot_segments(&ctx);
                        emit(Event::Snapshot(state.clone()));
                        return Err(Error::RetriesExhausted(opts.max_retries, msg));
                    }
                }
            }
            _ = tick.tick() => {}
        }

        if *cancel.borrow() {
            paused = true;
        }

        state.segments = snapshot_segments(&ctx);
        state.resumable = resumable;
        let downloaded = state.downloaded();
        let elapsed = last_progress.elapsed().as_secs_f64();
        if elapsed >= 0.5 {
            emit(Event::Progress {
                downloaded,
                total: state.size,
                bytes_per_sec: (downloaded.saturating_sub(last_bytes)) as f64 / elapsed,
                connections: workers.len(),
            });
            last_bytes = downloaded;
            last_progress = Instant::now();
        }
        if last_snapshot.elapsed() >= Duration::from_secs(1) {
            emit(Event::Snapshot(state.clone()));
            last_snapshot = Instant::now();
        }

        if workers.is_empty() {
            if paused {
                ctx.file.sync_data()?;
                emit(Event::Snapshot(state.clone()));
                return Ok(Outcome::Paused(state));
            }
            if all_complete(&ctx) {
                break;
            }
        }
    }

    // Unknown-size downloads end at EOF: the single segment tells the real size.
    if state.size.is_none() {
        let seg = state.segments[0];
        state.size = Some(seg.pos());
        state.segments[0].end = seg.pos();
        ctx.file.set_len(seg.pos())?;
    }
    ctx.file.sync_all()?;
    std::fs::rename(&part, &final_path)?;
    emit(Event::Snapshot(state.clone()));
    Ok(Outcome::Completed { path: final_path, state })
}

/// First request: `Range: bytes=0-` tells size and range support in one round trip.
async fn probe(client: &Client, url: &str, headers: &HeaderMap) -> Result<(State, Response), Error> {
    let mut h = headers.clone();
    http::insert_range(&mut h, 0, None);
    let resp = client.get(url).headers(h).send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Error::Status(status));
    }
    let rh = resp.headers();
    let text = |name: HeaderName| rh.get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);

    let (size, resumable) = if status == StatusCode::PARTIAL_CONTENT {
        let total = text(CONTENT_RANGE).and_then(|v| http::parse_content_range(&v)).and_then(|r| r.2);
        (total, total.is_some())
    } else {
        // 200: range ignored for this request; extra connections will re-check.
        let len = text(CONTENT_LENGTH).and_then(|v| v.parse().ok());
        let accepts = text(ACCEPT_RANGES).is_some_and(|v| v.eq_ignore_ascii_case("bytes"));
        (len, len.is_some() && accepts)
    };

    let final_url = resp.url().to_string();
    let filename = http::filename_from_disposition(rh)
        .or_else(|| http::filename_from_url(&final_url))
        .unwrap_or_else(|| "download".into());

    let state = State {
        url: url.to_owned(),
        size,
        etag: text(ETAG),
        last_modified: text(LAST_MODIFIED),
        resumable,
        filename: Some(filename),
        segments: Vec::new(),
    };
    debug!(?state, "probe");
    Ok((state, resp))
}

fn spawn_workers(
    ctx: &Arc<Ctx>,
    workers: &mut JoinSet<(usize, Exit)>,
    first: &mut Option<Response>,
    resumable: bool,
    opts: &Options,
) {
    if let Some(resp) = first.take() {
        ctx.slots.lock().unwrap()[0].active = true;
        let c = ctx.clone();
        workers.spawn(async move { (0, worker(c, 0, Some(resp)).await) });
    }
    while workers.len() < opts.max_connections.max(1) {
        let idx = {
            let mut slots = ctx.slots.lock().unwrap();
            // Continue segments left idle (resume, failed or reverted connection) first.
            if let Some(i) = slots.iter().position(|s| !s.active && !s.seg.is_complete()) {
                if !resumable && slots[i].seg.done > 0 {
                    // Without Range support only a restart from 0 is possible.
                    slots[i].seg.done = 0;
                }
                Some(i)
            } else if resumable {
                split_best(&mut slots, opts)
            } else {
                None
            }
        };
        let Some(idx) = idx else { break };
        ctx.slots.lock().unwrap()[idx].active = true;
        let c = ctx.clone();
        workers.spawn(async move { (idx, worker(c, idx, None).await) });
    }
}

/// Splits the active segment with the largest estimated remaining work.
/// Returns the index of the new tail segment.
fn split_best(slots: &mut Vec<Slot>, opts: &Options) -> Option<usize> {
    let (best, est) = slots
        .iter()
        .enumerate()
        .filter(|(_, s)| s.active && s.seg.end != u64::MAX)
        .map(|(i, s)| (i, s.seg.remaining().saturating_sub((s.speed * opts.split_horizon_secs) as u64)))
        .max_by_key(|&(_, est)| est)?;
    if est < 2 * opts.min_split {
        return None;
    }
    let old = slots[best].seg;
    let new_start = old.end - est / 2;
    slots[best].seg.end = new_start;
    slots.push(Slot { seg: Segment { start: new_start, end: old.end, done: 0 }, active: false, speed: 0.0 });
    Some(slots.len() - 1)
}

/// A tail connection could not get its range: hand the bytes back to the
/// segment that precedes it so the first connection keeps going.
fn give_back(ctx: &Ctx, idx: usize) {
    let mut slots = ctx.slots.lock().unwrap();
    let tail = slots[idx].seg;
    if tail.done > 0 {
        return;
    }
    if let Some(prev) = slots.iter_mut().find(|s| s.seg.end == tail.start && s.active) {
        prev.seg.end = tail.end;
        slots[idx].seg.end = slots[idx].seg.start;
    }
}

async fn worker(ctx: Arc<Ctx>, idx: usize, mut resp: Option<Response>) -> Exit {
    let mut attempt = 0u32;
    loop {
        let (pos, end) = {
            let s = ctx.slots.lock().unwrap()[idx].seg;
            (s.pos(), s.end)
        };
        if pos >= end {
            return Exit::Done;
        }
        let r = match resp.take() {
            Some(r) => Ok(r),
            None => request_range(&ctx, pos, end).await,
        };
        let result = match r {
            Ok(r) => match check_response(&ctx, &r, pos, end) {
                Ok(()) => stream_body(&ctx, idx, r).await,
                Err(exit) => return exit,
            },
            Err(e) => Err(e.to_string()),
        };
        match result {
            Ok(exit) => return exit,
            Err(msg) => {
                let progressed = ctx.slots.lock().unwrap()[idx].seg.pos() > pos;
                attempt = if progressed { 1 } else { attempt + 1 };
                if attempt > ctx.max_retries {
                    return Exit::Failed(msg);
                }
                let delay = Duration::from_secs((1u64 << attempt.min(5)).min(30));
                warn!(idx, attempt, %msg, ?delay, "connection error, retrying");
                let mut cancel = ctx.cancel.clone();
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = cancel.wait_for(|c| *c) => return Exit::Paused,
                }
            }
        }
    }
}

async fn request_range(ctx: &Ctx, pos: u64, end: u64) -> Result<Response, reqwest::Error> {
    let mut h = ctx.headers.clone();
    http::insert_range(&mut h, pos, (end != u64::MAX).then_some(end));
    if let Some(v) = &ctx.if_range {
        h.insert(IF_RANGE, v.clone());
    }
    ctx.client.get(&ctx.url).headers(h).send().await
}

fn check_response(ctx: &Ctx, r: &Response, pos: u64, end: u64) -> Result<(), Exit> {
    match r.status() {
        StatusCode::PARTIAL_CONTENT => {
            let range = r
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(http::parse_content_range);
            match range {
                Some((first, _, _)) if first == pos => Ok(()),
                _ => Err(Exit::RangeUnsupported),
            }
        }
        // Full body for a ranged request: entity changed (If-Range) or no Range support.
        StatusCode::OK if ctx.resuming && pos > 0 => Err(Exit::RemoteChanged),
        StatusCode::OK if pos == 0 => Ok(()),
        StatusCode::OK => Err(Exit::RangeUnsupported),
        s => {
            debug!(pos, end, %s, "unexpected status");
            Err(Exit::Failed(format!("server answered {s}")))
        }
    }
}

/// Copies the body into the file until the (possibly shrinking) segment end.
async fn stream_body(ctx: &Ctx, idx: usize, resp: Response) -> Result<Exit, String> {
    let mut body = resp.bytes_stream();
    let mut cancel = ctx.cancel.clone();
    let mut window_start = Instant::now();
    let mut window_bytes = 0u64;

    loop {
        let chunk = tokio::select! {
            c = body.next() => c,
            _ = cancel.wait_for(|c| *c) => return Ok(Exit::Paused),
        };
        let chunk = match chunk {
            Some(Ok(c)) => c,
            Some(Err(e)) => return Err(e.to_string()),
            None => {
                let mut slots = ctx.slots.lock().unwrap();
                let seg = &mut slots[idx].seg;
                if seg.end == u64::MAX {
                    // Unknown size: EOF is the end.
                    seg.end = seg.pos();
                    return Ok(Exit::Done);
                }
                return if seg.is_complete() { Ok(Exit::Done) } else { Err("connection closed early".into()) };
            }
        };
        ctx.limiter.acquire(chunk.len()).await;

        let (pos, end) = {
            let s = ctx.slots.lock().unwrap()[idx].seg;
            (s.pos(), s.end)
        };
        let take = (chunk.len() as u64).min(end.saturating_sub(pos)) as usize;
        if take > 0 {
            let file = ctx.file.clone();
            let data = chunk.slice(..take);
            tokio::task::spawn_blocking(move || file.write_all_at(&data, pos))
                .await
                .expect("write task panicked")
                .map_err(|e| e.to_string())?;
        }

        let mut slots = ctx.slots.lock().unwrap();
        let slot = &mut slots[idx];
        slot.seg.done += take as u64;
        window_bytes += take as u64;
        let dt = window_start.elapsed().as_secs_f64();
        if dt >= 0.5 {
            let inst = window_bytes as f64 / dt;
            slot.speed = if slot.speed == 0.0 { inst } else { 0.7 * slot.speed + 0.3 * inst };
            window_start = Instant::now();
            window_bytes = 0;
        }
        if slot.seg.is_complete() {
            return Ok(Exit::Done);
        }
    }
}

async fn stop_all(workers: &mut JoinSet<(usize, Exit)>) {
    workers.abort_all();
    while workers.join_next().await.is_some() {}
}

fn snapshot_segments(ctx: &Ctx) -> Vec<Segment> {
    let slots = ctx.slots.lock().unwrap();
    let mut segs: Vec<Segment> = slots.iter().map(|s| s.seg).filter(|s| s.end > s.start).collect();
    segs.sort_by_key(|s| s.start);
    segs
}

fn all_complete(ctx: &Ctx) -> bool {
    ctx.slots.lock().unwrap().iter().all(|s| s.seg.is_complete())
}

fn validator(state: &State) -> Option<HeaderValue> {
    // Weak ETags are not allowed in If-Range.
    state
        .etag
        .as_deref()
        .filter(|e| !e.starts_with("W/"))
        .or(state.last_modified.as_deref())
        .and_then(|v| HeaderValue::from_str(v).ok())
}

fn resolve_final(dest: &Dest, filename: Option<&str>) -> PathBuf {
    match dest {
        Dest::File(p) => p.clone(),
        Dest::Dir(d) => d.join(filename.unwrap_or("download")),
    }
}

/// `name.ext` -> `name (1).ext` while the final or partial file already exists.
/// Compound extensions stay together: `a.tar.xz` -> `a (1).tar.xz`.
fn unique_path(path: PathBuf) -> PathBuf {
    let taken = |p: &Path| p.exists() || part_path(p).exists();
    if !taken(&path) {
        return path;
    }
    let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let (stem, ext) = split_extension(&name);
    (1..)
        .map(|n| path.with_file_name(format!("{stem} ({n}){ext}")))
        .find(|p| !taken(p))
        .expect("unbounded range")
}

fn split_extension(name: &str) -> (&str, &str) {
    let lower = name.to_ascii_lowercase();
    for compound in [".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst", ".tar.lz", ".tar.z"] {
        if lower.len() > compound.len() && lower.ends_with(compound) {
            return name.split_at(name.len() - compound.len());
        }
    }
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

fn open_part(path: &Path, size: Option<u64>, fresh: bool) -> Result<File, Error> {
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(fresh).open(path)?;
    if let (Some(size), true) = (size, fresh) {
        // Sparse preallocation; positional writes fill it in any order.
        file.set_len(size)?;
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(start: u64, end: u64, done: u64, speed: f64) -> Slot {
        Slot { seg: Segment { start, end, done }, active: true, speed }
    }

    #[test]
    fn splits_segment_with_most_estimated_work() {
        let opts = Options::default();
        // Same remaining bytes, but slot 1 is slower: it finishes later, so it gets split.
        let mut slots = vec![slot(0, 1_000_000, 0, 500_000.0), slot(1_000_000, 2_000_000, 0, 10_000.0)];
        let new = split_best(&mut slots, &opts).unwrap();
        assert_eq!(new, 2);
        let est = 1_000_000 - 10_000;
        assert_eq!(slots[1].seg.end, 2_000_000 - est / 2);
        assert_eq!(slots[2].seg, Segment { start: 2_000_000 - est / 2, end: 2_000_000, done: 0 });
    }

    #[test]
    fn never_creates_tiny_segments() {
        let opts = Options::default();
        let mut slots = vec![slot(0, 40_000, 0, 0.0)];
        assert!(split_best(&mut slots, &opts).is_none());
    }

    #[test]
    fn unique_names() {
        let dir = std::env::temp_dir().join(format!("raudo-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.iso");
        std::fs::write(&p, b"x").unwrap();
        assert_eq!(unique_path(p.clone()), dir.join("a (1).iso"));
        let t = dir.join("linux.tar.xz");
        std::fs::write(&t, b"x").unwrap();
        assert_eq!(unique_path(t), dir.join("linux (1).tar.xz"));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(split_extension("README"), ("README", ""));
        assert_eq!(split_extension(".bashrc"), (".bashrc", ""));
    }
}
