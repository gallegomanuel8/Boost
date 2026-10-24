use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use raudo_core::{hls, Dest, Error as EngineError, Event, Job, Limiter, Options, Outcome};
use raudo_proto::{AddDownload, DownloadInfo, MediaInfo, Request, Status, VideoOptions};
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};

use crate::db::{Db, Stored};
use crate::resolver;
use crate::video::{self, VideoJob, VideoOutcome};

/// How a download task ended, whatever did the work (engine or yt-dlp).
enum Finished {
    Completed(PathBuf),
    Paused,
    /// Start again from zero (remote file changed).
    Restart,
    Failed(String),
}

/// HLS/DASH manifests need yt-dlp even when added as plain links.
fn is_manifest(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
    path.ends_with(".m3u8") || path.ends_with(".mpd")
}

/// Notifications for D-Bus clients.
pub enum Notify {
    Changed(DownloadInfo),
    Removed(i64),
}

struct Running {
    cancel: watch::Sender<bool>,
    /// Set by `remove`: delete the partial file once the engine has stopped.
    discard: Arc<AtomicBool>,
    speed: f64,
    connections: usize,
}

pub struct Manager {
    db: Mutex<Db>,
    client: reqwest::Client,
    limiter: Arc<Limiter>,
    opts: Options,
    default_dir: PathBuf,
    /// Partial files live in `<temp_root>/<id>/`, never in the user's download folder.
    temp_root: PathBuf,
    running: Mutex<HashMap<i64, Running>>,
    notify: mpsc::UnboundedSender<Notify>,
    /// Pauses caused by shutdown keep the download queued for the next start.
    shutting_down: AtomicBool,
    /// `inspect` answers by target and URL.
    inspected: Mutex<HashMap<String, (Instant, serde_json::Value)>>,
}

pub type Result<T> = std::result::Result<T, String>;

fn db_err(e: rusqlite::Error) -> String {
    format!("database error: {e}")
}

impl Manager {
    pub fn new(db: Db, default_dir: PathBuf, temp_root: PathBuf, notify: mpsc::UnboundedSender<Notify>) -> Arc<Self> {
        Arc::new(Self {
            db: Mutex::new(db),
            client: raudo_core::client().expect("TLS backend available"),
            limiter: Arc::new(Limiter::new(None)),
            opts: Options::default(),
            default_dir,
            temp_root,
            running: Mutex::new(HashMap::new()),
            notify,
            shutting_down: AtomicBool::new(false),
            inspected: Mutex::new(HashMap::new()),
        })
    }

    pub fn add(self: &Arc<Self>, mut add: AddDownload) -> Result<i64> {
        if !(add.request.url.starts_with("http://") || add.request.url.starts_with("https://")) {
            return Err("only http and https URLs are supported".into());
        }
        if add.video.is_none() && is_manifest(&add.request.url) {
            add.video = Some(VideoOptions::default());
        }
        // Names come from web pages: never let them carry a path.
        add.filename = add.filename.as_deref().and_then(raudo_core::http::sanitize);
        let dir = match &add.dir {
            Some(d) => PathBuf::from(d),
            None => self.default_dir.clone(),
        };
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let id = self.db.lock().unwrap().insert(&add, &dir.to_string_lossy()).map_err(db_err)?;
        info!(id, url = %add.request.url, "added");
        self.changed(id);
        self.schedule();
        Ok(id)
    }

    pub fn list(&self) -> Result<Vec<DownloadInfo>> {
        let mut list = self.db.lock().unwrap().list().map_err(db_err)?;
        let running = self.running.lock().unwrap();
        for d in &mut list {
            if let Some(r) = running.get(&d.id) {
                d.speed = r.speed;
                d.connections = r.connections;
            }
        }
        Ok(list)
    }

    pub fn pause(self: &Arc<Self>, id: i64) -> Result<()> {
        if let Some(r) = self.running.lock().unwrap().get(&id) {
            // The task records `paused` once the engine has saved its state.
            let _ = r.cancel.send(true);
            return Ok(());
        }
        let db = self.db.lock().unwrap();
        match db.status(id).map_err(db_err)? {
            None => return Err(format!("no download {id}")),
            Some(Status::Queued) => db.set_status(id, Status::Paused, None).map_err(db_err)?,
            Some(_) => return Ok(()),
        }
        drop(db);
        self.changed(id);
        Ok(())
    }

    pub fn resume(self: &Arc<Self>, id: i64) -> Result<()> {
        let db = self.db.lock().unwrap();
        match db.status(id).map_err(db_err)? {
            None => return Err(format!("no download {id}")),
            Some(Status::Paused | Status::Error) => db.set_status(id, Status::Queued, None).map_err(db_err)?,
            Some(_) => return Ok(()),
        }
        drop(db);
        self.changed(id);
        self.schedule();
        Ok(())
    }

    pub fn remove(self: &Arc<Self>, id: i64, delete_file: bool) -> Result<()> {
        let info = self.db.lock().unwrap().info(id).map_err(db_err)?.ok_or(format!("no download {id}"))?;
        let path = info.filename.as_ref().map(|f| PathBuf::from(&info.dir).join(f));
        if let Some(r) = self.running.lock().unwrap().get(&id) {
            // The task deletes the temp dir once the engine or yt-dlp has stopped writing.
            r.discard.store(true, Ordering::SeqCst);
            let _ = r.cancel.send(true);
        } else {
            self.clean_temp(id);
            // Partial files of downloads made before the temp dir existed.
            if let Some(p) = &path {
                let _ = std::fs::remove_file(raudo_core::part_path(p));
            }
        }
        if delete_file && info.status == Status::Completed {
            if let Some(p) = &path {
                let _ = std::fs::remove_file(p);
            }
        }
        self.db.lock().unwrap().delete(id).map_err(db_err)?;
        let _ = self.notify.send(Notify::Removed(id));
        Ok(())
    }

    fn temp_dir(&self, id: i64) -> PathBuf {
        self.temp_root.join(id.to_string())
    }

    fn clean_temp(&self, id: i64) {
        let dir = self.temp_dir(id);
        if dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                warn!(id, "cannot remove {}: {e}", dir.display());
            }
        }
    }

    pub async fn formats(&self, request: &Request) -> Result<MediaInfo> {
        video::formats(request).await
    }

    /// `page`: can the page itself be downloaded (yt-dlp extractor or a player we
    /// can resolve)? `hls`: what a playlist contains (quality, duration, container).
    /// Answers are cached for a while: the popup asks every time it opens.
    pub async fn inspect(self: &Arc<Self>, target: &str, request: Request) -> Result<serde_json::Value> {
        const TTL: Duration = Duration::from_secs(600);
        let key = format!("{target}|{}", request.url);
        if let Some((at, v)) = self.inspected.lock().unwrap().get(&key) {
            if at.elapsed() < TTL {
                return Ok(v.clone());
            }
        }
        let value = match target {
            "page" => {
                let via = match video::ytdlp_supports(&request.url).await {
                    Some(true) => Some("yt-dlp"),
                    known => {
                        let mut r = request.clone();
                        r.user_agent.get_or_insert_with(|| resolver::BROWSER_UA.to_owned());
                        if resolver::resolve(&self.client, &r).await.is_some() {
                            Some("player")
                        } else if known.is_none() {
                            // Could not ask yt-dlp: do not block the buttons on a guess.
                            Some("yt-dlp")
                        } else {
                            None
                        }
                    }
                };
                serde_json::json!({ "supported": via.is_some(), "via": via })
            }
            "hls" => match hls::inspect(&self.client, &request).await {
                Ok(info) => serde_json::to_value(info).expect("serializable"),
                Err(e) => {
                    warn!(url = %request.url, referer = ?request.referer, "inspect hls: {e}");
                    return Err(e);
                }
            },
            other => return Err(format!("unknown inspect target {other}")),
        };
        let mut cache = self.inspected.lock().unwrap();
        cache.retain(|_, (at, _)| at.elapsed() < TTL);
        cache.insert(key, (Instant::now(), value.clone()));
        Ok(value)
    }

    pub fn set_speed_limit(&self, kib: u64) {
        self.limiter.set_rate((kib > 0).then_some(kib * 1024));
    }

    pub fn set_max_concurrent(self: &Arc<Self>, n: u32) -> Result<()> {
        self.db.lock().unwrap().set_max_concurrent(1, n.max(1)).map_err(db_err)?;
        self.schedule();
        Ok(())
    }

    /// Starts queued downloads while their queue has free slots.
    pub fn schedule(self: &Arc<Self>) {
        let queued = match self.db.lock().unwrap().queued() {
            Ok(q) => q,
            Err(e) => return warn!("schedule: {e}"),
        };
        let mut running_per_queue: HashMap<i64, u32> = HashMap::new();
        {
            let running = self.running.lock().unwrap();
            let db = self.db.lock().unwrap();
            for id in running.keys() {
                if let Ok(q) = db.queue_of(*id) {
                    *running_per_queue.entry(q).or_default() += 1;
                }
            }
        }
        for (id, queue, max) in queued {
            let n = running_per_queue.entry(queue).or_default();
            if *n >= max {
                continue;
            }
            *n += 1;
            self.start(id);
        }
    }

    fn start(self: &Arc<Self>, id: i64) {
        let stored = {
            let db = self.db.lock().unwrap();
            let stored = match db.stored(id) {
                Ok(Some(s)) => s,
                _ => return,
            };
            if let Err(e) = db.set_status(id, Status::Downloading, None) {
                return warn!(id, "start: {e}");
            }
            stored
        };
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let discard = Arc::new(AtomicBool::new(false));
        self.running.lock().unwrap().insert(
            id,
            Running { cancel: cancel_tx, discard: discard.clone(), speed: 0.0, connections: 0 },
        );
        self.changed(id);

        if stored.video.is_some() {
            return self.start_video(id, stored, cancel_rx, discard);
        }
        let dir = PathBuf::from(&stored.dir);
        // An explicit file name wins; otherwise the title only replaces random server names.
        let title_hint = if stored.filename.is_none() { stored.title.clone() } else { None };
        let job = Job {
            title_hint,
            request: stored.request,
            dest: match &stored.filename {
                Some(name) => Dest::File(dir.join(name)),
                None => Dest::Dir(dir.clone()),
            },
            resume: stored.state,
            temp_dir: Some(self.temp_dir(id)),
        };
        let this = self.clone();
        tokio::spawn(async move {
            let (tx, rx) = mpsc::unbounded_channel();
            let events = tokio::spawn(this.clone().consume_events(id, rx));
            let outcome = raudo_core::run(&this.client, job, &this.opts, this.limiter.clone(), tx, cancel_rx).await;
            let _ = events.await;
            let finished = match outcome {
                Ok(Outcome::Completed { path, .. }) => Finished::Completed(path),
                Ok(Outcome::Paused(_)) => Finished::Paused,
                Err(EngineError::RemoteChanged) => Finished::Restart,
                Err(e) => Finished::Failed(e.to_string()),
            };
            this.finish(id, finished, discard.load(Ordering::SeqCst));
        });
    }

    fn start_video(self: &Arc<Self>, id: i64, stored: Stored, cancel: watch::Receiver<bool>, discard: Arc<AtomicBool>) {
        let this = self.clone();
        tokio::spawn(async move {
            let dir = PathBuf::from(&stored.dir);
            let format = stored.video.as_ref().and_then(|v| v.format.clone());
            let audio_kbps = stored.video.as_ref().and_then(|v| v.audio_kbps);
            let temp = this.temp_dir(id);
            // yt-dlp overwrites an existing output (MP3 conversion always does), so it
            // finishes inside the temp dir and we pick a free name in `dir` ourselves.
            let out = temp.join("out");
            let mut title = stored.title.as_deref().map(video::clean_stem);
            let mut request = stored.request.clone();

            let mut outcome = this
                .stream_or_ytdlp(id, &request, &out, &temp, title.as_deref(), format.as_deref(), audio_kbps, cancel.clone())
                .await;
            // Look for the stream behind the page ourselves when yt-dlp cannot parse
            // the page, or when the link the browser saw is refused here (tokens bound
            // to the browser's IP: Firefox's built-in VPN, proxies).
            let unsupported = matches!(&outcome, Err(e) if e.contains("Unsupported URL"));
            let refused = matches!(&outcome, Err(e) if e.contains("403")) && request.page.is_some();
            if unsupported || refused {
                let mut page = if unsupported {
                    request.clone()
                } else {
                    info!(id, "stream link refused, resolving a fresh one from the page");
                    Request {
                        url: request.page.clone().unwrap_or_default(),
                        referer: None,
                        headers: Vec::new(),
                        ..request.clone()
                    }
                };
                // Same user agent for the page fetch and the download: tokens may be bound to it.
                page.user_agent.get_or_insert_with(|| resolver::BROWSER_UA.to_owned());
                request.user_agent.clone_from(&page.user_agent);
                if let Some(found) = resolver::resolve(&this.client, &page).await {
                    info!(id, url = %found.url, "resolved embedded player stream");
                    title = title.or(found.title.as_deref().map(video::clean_stem));
                    request = Request {
                        url: found.url,
                        referer: Some(found.referer),
                        cookies: None,
                        headers: Vec::new(),
                        page: None,
                        ..page
                    };
                    outcome = this
                        .stream_or_ytdlp(id, &request, &out, &temp, title.as_deref(), format.as_deref(), audio_kbps, cancel)
                        .await;
                }
            }
            let finished = match outcome {
                Ok(VideoOutcome::Completed(done)) => {
                    let name = done.file_name().map(PathBuf::from).unwrap_or_else(|| "video".into());
                    let target = raudo_core::unique_path(dir.join(name), None);
                    match std::fs::create_dir_all(&dir).and_then(|_| raudo_core::move_file(&done, &target)) {
                        Ok(()) => Finished::Completed(target),
                        Err(e) => Finished::Failed(format!("cannot move to {}: {e}", target.display())),
                    }
                }
                Ok(VideoOutcome::Paused) => Finished::Paused,
                Err(e) => Finished::Failed(e),
            };
            this.finish(id, finished, discard.load(Ordering::SeqCst));
        });
    }

    /// HLS playlists are fetched natively, segment by segment (as IDM does);
    /// everything else goes through yt-dlp.
    #[allow(clippy::too_many_arguments)]
    async fn stream_or_ytdlp(
        self: &Arc<Self>,
        id: i64,
        request: &Request,
        out: &std::path::Path,
        temp: &std::path::Path,
        title: Option<&str>,
        format: Option<&str>,
        audio_kbps: Option<u32>,
        cancel: watch::Receiver<bool>,
    ) -> std::result::Result<VideoOutcome, String> {
        let mut on_progress = self.progress_sink(id);
        if !video::is_hls(&request.url) {
            let job = VideoJob {
                request,
                dir: out,
                temp_dir: temp,
                title,
                format,
                audio_kbps,
                rate_limit: self.limiter.rate(),
            };
            return video::run(job, on_progress, cancel).await;
        }

        let job = hls::HlsJob {
            request,
            work_dir: &temp.join("hls"),
            // The audio is the same in every variant: a small one is enough for MP3.
            max_height: audio_kbps.map(|_| 480),
            concurrency: 6,
        };
        let from_url = raudo_core::http::filename_from_url(&request.url)
            .filter(|n| !raudo_core::http::looks_random(n))
            .map(|n| n.trim_end_matches(".m3u8").to_owned());
        let stem = video::clean_stem(&title.map(str::to_owned).or(from_url).unwrap_or_else(|| "video".into()));
        // Shown while downloading; the final name may get " (1)" if taken.
        let shown = format!("{stem}.{}", if audio_kbps.is_some() { "mp3" } else { "mp4" });
        let progress = |p: hls::HlsProgress| {
            on_progress(video::Progress {
                downloaded: p.downloaded,
                total: p.total,
                speed: p.speed,
                file: Some(shown.clone()),
            })
        };
        match hls::run(&self.client, job, progress, cancel).await? {
            hls::HlsOutcome::Paused => Ok(VideoOutcome::Paused),
            hls::HlsOutcome::Completed(tracks) => {
                video::mux(&tracks, out, &stem, audio_kbps).await.map(VideoOutcome::Completed)
            }
        }
    }

    /// Records download progress: speed for `list`, throttled database writes and signals.
    fn progress_sink(self: &Arc<Self>, id: i64) -> impl FnMut(video::Progress) + '_ {
        let mut last_save = Instant::now() - Duration::from_secs(1);
        move |p: video::Progress| {
            if let Some(r) = self.running.lock().unwrap().get_mut(&id) {
                r.speed = p.speed;
                r.connections = 1;
            }
            if last_save.elapsed() >= Duration::from_millis(500) {
                let db = self.db.lock().unwrap();
                let saved = db
                    .save_progress(id, p.downloaded, p.total)
                    .and_then(|_| p.file.as_deref().map_or(Ok(()), |f| db.set_filename(id, f)));
                drop(db);
                if let Err(e) = saved {
                    warn!(id, "save progress: {e}");
                }
                self.changed(id);
                last_save = Instant::now();
            }
        }
    }

    /// Persists engine snapshots and forwards throttled progress.
    /// Returns the last state seen, used to find the partial file on removal.
    async fn consume_events(self: Arc<Self>, id: i64, mut rx: mpsc::UnboundedReceiver<Event>) -> Option<raudo_core::State> {
        let mut last_notify = Instant::now() - Duration::from_secs(1);
        let mut last_state = None;
        while let Some(ev) = rx.recv().await {
            match ev {
                Event::Started(s) | Event::Snapshot(s) => {
                    if let Err(e) = self.db.lock().unwrap().save_state(id, &s) {
                        warn!(id, "save state: {e}");
                    }
                    last_state = Some(s);
                }
                Event::Progress { bytes_per_sec, connections, .. } => {
                    if let Some(r) = self.running.lock().unwrap().get_mut(&id) {
                        r.speed = bytes_per_sec;
                        r.connections = connections;
                    }
                }
                Event::Notice(n) => info!(id, "{n}"),
            }
            if last_notify.elapsed() >= Duration::from_millis(500) {
                self.changed(id);
                last_notify = Instant::now();
            }
        }
        last_state
    }

    fn finish(self: &Arc<Self>, id: i64, finished: Finished, discard: bool) {
        self.running.lock().unwrap().remove(&id);
        if discard {
            // Row already deleted by `remove`.
            self.clean_temp(id);
            self.schedule();
            return;
        }
        if matches!(finished, Finished::Completed(_) | Finished::Restart) {
            self.clean_temp(id);
        }
        let db = self.db.lock().unwrap();
        let result = match finished {
            Finished::Completed(path) => {
                info!(id, path = %path.display(), "completed");
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                // Real size on disk: yt-dlp totals are often estimates.
                let len = std::fs::metadata(&path).map(|m| m.len()).ok();
                name.map_or(Ok(()), |n| db.set_filename(id, &n))
                    .and_then(|_| len.map_or(Ok(()), |l| db.save_progress(id, l, Some(l))))
                    .and_then(|_| db.set_status(id, Status::Completed, None))
            }
            Finished::Paused if self.shutting_down.load(Ordering::SeqCst) => db.set_status(id, Status::Queued, None),
            Finished::Paused => db.set_status(id, Status::Paused, None),
            Finished::Restart => {
                warn!(id, "remote file changed, restarting from scratch");
                db.clear_state(id).and_then(|_| db.set_status(id, Status::Queued, None))
            }
            Finished::Failed(e) => {
                warn!(id, "failed: {e}");
                db.set_status(id, Status::Error, Some(&e))
            }
        };
        drop(db);
        if let Err(e) = result {
            warn!(id, "finish: {e}");
        }
        self.changed(id);
        if !self.shutting_down.load(Ordering::SeqCst) {
            self.schedule();
        }
    }

    /// Pauses everything for a clean shutdown; states are saved by the engine.
    pub async fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        for r in self.running.lock().unwrap().values() {
            let _ = r.cancel.send(true);
        }
        for _ in 0..100 {
            if self.running.lock().unwrap().is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        warn!("some downloads did not stop in time; they will resume on next start");
    }

    fn changed(&self, id: i64) {
        let info = self.db.lock().unwrap().info(id).ok().flatten();
        if let Some(mut info) = info {
            if let Some(r) = self.running.lock().unwrap().get(&id) {
                info.speed = r.speed;
                info.connections = r.connections;
            }
            let _ = self.notify.send(Notify::Changed(info));
        }
    }
}
