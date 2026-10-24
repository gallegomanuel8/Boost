mod db;
mod manager;
mod resolver;
mod service;
mod video;

use std::path::PathBuf;

use raudo_proto::{BUS_NAME, OBJECT_PATH};
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;
use tracing::{info, warn};
use zbus::object_server::SignalEmitter;

use manager::{Manager, Notify};
use service::Service;

fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("raudo")
}

fn download_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let data = data_dir();
    std::fs::create_dir_all(&data)?;
    let db = db::Db::open(&data.join("raudo.db"))?;
    let requeued = db.requeue_interrupted()?;
    if requeued > 0 {
        info!(requeued, "resuming downloads interrupted by the last shutdown");
    }

    let (notify_tx, mut notify_rx) = mpsc::unbounded_channel();
    let temp_root = dirs::cache_dir().unwrap_or_else(|| data.clone()).join("raudo").join("parts");
    let manager = Manager::new(db, download_dir(), temp_root, notify_tx);

    let conn = zbus::connection::Builder::session()?
        .name(BUS_NAME)?
        .serve_at(OBJECT_PATH, Service { manager: manager.clone() })?
        .build()
        .await?;
    info!("listening on D-Bus as {BUS_NAME}");

    let emitter_conn = conn.clone();
    tokio::spawn(async move {
        let emitter = SignalEmitter::new(&emitter_conn, OBJECT_PATH).expect("valid path");
        while let Some(n) = notify_rx.recv().await {
            let r = match n {
                Notify::Changed(info) => {
                    Service::changed(&emitter, &serde_json::to_string(&info).expect("serializable")).await
                }
                Notify::Removed(id) => Service::removed(&emitter, id).await,
            };
            if let Err(e) = r {
                warn!("signal: {e}");
            }
        }
    });

    manager.schedule();

    let mut term = signal(SignalKind::terminate())?;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
    info!("shutting down, saving download states");
    manager.shutdown().await;
    Ok(())
}
