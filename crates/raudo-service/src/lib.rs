//! The raudo download service: queue, engine, yt-dlp, persistence, and the IPC
//! endpoint for the CLI and the browser host. Embedded by the desktop app;
//! `raudo-daemon` runs it without a window.

mod cover;
mod db;
mod manager;
mod resolver;
mod video;

use std::path::PathBuf;
use std::sync::Arc;

use raudo_ipc::{BoxFuture, Call, Event, Handler};
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tracing::info;

pub use manager::Manager;
pub use raudo_ipc::ServeError;
pub use video::find_tool;

fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("raudo")
}

fn download_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// A started service: the manager (for an embedding app) and the IPC endpoint.
pub struct Service {
    pub manager: Arc<Manager>,
}

#[derive(Debug)]
pub enum StartError {
    AlreadyRunning,
    Other(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::AlreadyRunning => f.write_str("raudo is already running"),
            StartError::Other(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for StartError {}

impl Service {
    /// Opens the database, resumes interrupted downloads and starts answering clients.
    /// Fails with `AlreadyRunning` when another instance owns the endpoint.
    pub async fn start() -> Result<Self, StartError> {
        let (listener, token) = raudo_ipc::listen().await.map_err(|e| match e {
            ServeError::AlreadyRunning => StartError::AlreadyRunning,
            other => StartError::Other(other.to_string()),
        })?;

        let data = data_dir();
        std::fs::create_dir_all(&data).map_err(|e| StartError::Other(format!("{}: {e}", data.display())))?;
        let db = db::Db::open(&data.join("raudo.db")).map_err(|e| StartError::Other(format!("database: {e}")))?;
        let requeued = db.requeue_interrupted().map_err(|e| StartError::Other(e.to_string()))?;
        if requeued > 0 {
            info!(requeued, "resuming downloads interrupted by the last shutdown");
        }
        let temp_root = dirs::cache_dir().unwrap_or_else(|| data.clone()).join("raudo").join("parts");
        let manager = Manager::new(db, download_dir(), temp_root);
        manager.load_settings();

        let handler: Arc<dyn Handler> = Arc::new(IpcHandler(manager.clone()));
        tokio::spawn(async move {
            if let Err(e) = raudo_ipc::serve(listener, token, handler).await {
                tracing::error!("IPC endpoint stopped: {e}");
            }
        });
        info!(endpoint = %raudo_ipc::paths::runtime_dir().display(), "raudo service ready");
        manager.schedule();
        Ok(Service { manager })
    }

    /// Pauses running downloads so they resume on the next start.
    pub async fn shutdown(&self) {
        self.manager.shutdown().await;
    }
}

struct IpcHandler(Arc<Manager>);

fn to_value<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).expect("serializable")
}

impl Handler for IpcHandler {
    fn call(&self, call: Call) -> BoxFuture<Result<Value, String>> {
        let m = self.0.clone();
        Box::pin(async move {
            match call {
                Call::Add(add) => m.add(add).map(to_value),
                Call::List => m.list().map(to_value),
                Call::Pause { id } => m.pause(id).map(|_| Value::Null),
                Call::Resume { id } => m.resume(id).map(|_| Value::Null),
                Call::Restart { id } => m.restart(id).map(|_| Value::Null),
                Call::Remove { id, delete_file } => m.remove(id, delete_file).map(|_| Value::Null),
                Call::SetSpeedLimit { kib } => {
                    m.set_speed_limit(kib);
                    Ok(Value::Null)
                }
                Call::SetMaxConcurrent { n } => m.set_max_concurrent(n).map(|_| Value::Null),
                Call::Formats(request) => m.formats(&request).await.map(to_value),
                Call::Inspect { target, request } => m.inspect(&target, request).await,
                Call::Details { id } => m.details(id),
                Call::Settings => m.settings(),
                Call::Ping => Ok(json!("pong")),
                Call::Subscribe => Err("subscribe is handled by the connection".into()),
            }
        })
    }

    fn events(&self) -> broadcast::Receiver<Event> {
        self.0.subscribe()
    }
}
