use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use raudo_core::{Dest, Error as EngineError, Event, Job, Limiter, Options, Outcome};
use raudo_proto::{AddDownload, DownloadInfo, Status};
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};

use crate::db::Db;

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
    running: Mutex<HashMap<i64, Running>>,
    notify: mpsc::UnboundedSender<Notify>,
    /// Pauses caused by shutdown keep the download queued for the next start.
    shutting_down: AtomicBool,
}

pub type Result<T> = std::result::Result<T, String>;

fn db_err(e: rusqlite::Error) -> String {
    format!("database error: {e}")
}

impl Manager {
    pub fn new(db: Db, default_dir: PathBuf, notify: mpsc::UnboundedSender<Notify>) -> Arc<Self> {
        Arc::new(Self {
            db: Mutex::new(db),
            client: raudo_core::client().expect("TLS backend available"),
            limiter: Arc::new(Limiter::new(None)),
            opts: Options::default(),
            default_dir,
            running: Mutex::new(HashMap::new()),
            notify,
            shutting_down: AtomicBool::new(false),
        })
    }

    pub fn add(self: &Arc<Self>, add: AddDownload) -> Result<i64> {
        if !(add.request.url.starts_with("http://") || add.request.url.starts_with("https://")) {
            return Err("only http and https URLs are supported".into());
        }
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
            r.discard.store(true, Ordering::SeqCst);
            let _ = r.cancel.send(true);
        } else if let Some(p) = &path {
            let _ = std::fs::remove_file(raudo_core::part_path(p));
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

        let dir = PathBuf::from(&stored.dir);
        let job = Job {
            request: stored.request,
            dest: match &stored.filename {
                Some(name) => Dest::File(dir.join(name)),
                None => Dest::Dir(dir.clone()),
            },
            resume: stored.state,
        };
        let this = self.clone();
        tokio::spawn(async move {
            let (tx, rx) = mpsc::unbounded_channel();
            let events = tokio::spawn(this.clone().consume_events(id, rx));
            let outcome = raudo_core::run(&this.client, job, &this.opts, this.limiter.clone(), tx, cancel_rx).await;
            let last_state = events.await.ok().flatten();
            this.finish(id, outcome, discard.load(Ordering::SeqCst), &dir, last_state);
        });
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

    fn finish(
        self: &Arc<Self>,
        id: i64,
        outcome: std::result::Result<Outcome, EngineError>,
        discard: bool,
        dir: &std::path::Path,
        last_state: Option<raudo_core::State>,
    ) {
        self.running.lock().unwrap().remove(&id);
        if discard {
            if let Some(name) = last_state.and_then(|s| s.filename) {
                let _ = std::fs::remove_file(raudo_core::part_path(&dir.join(name)));
            }
            self.schedule();
            return;
        }
        let db = self.db.lock().unwrap();
        let result = match outcome {
            Ok(Outcome::Completed { path, .. }) => {
                info!(id, path = %path.display(), "completed");
                db.set_status(id, Status::Completed, None)
            }
            Ok(Outcome::Paused(_)) if self.shutting_down.load(Ordering::SeqCst) => {
                db.set_status(id, Status::Queued, None)
            }
            Ok(Outcome::Paused(_)) => db.set_status(id, Status::Paused, None),
            Err(EngineError::RemoteChanged) => {
                warn!(id, "remote file changed, restarting from scratch");
                db.clear_state(id).and_then(|_| db.set_status(id, Status::Queued, None))
            }
            Err(e) => {
                warn!(id, "failed: {e}");
                db.set_status(id, Status::Error, Some(&e.to_string()))
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
