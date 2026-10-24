//! raudo without a window: the download service for servers, scripts and
//! development. The desktop app embeds the same service.

use raudo_service::{Service, StartError};
use tracing::info;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let service = match Service::start().await {
        Ok(s) => s,
        Err(StartError::AlreadyRunning) => {
            eprintln!("raudo is already running");
            return std::process::ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("raudo-daemon: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    wait_for_stop().await;
    info!("shutting down, saving download states");
    service.shutdown().await;
    std::process::ExitCode::SUCCESS
}

#[cfg(unix)]
async fn wait_for_stop() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("signal handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

#[cfg(windows)]
async fn wait_for_stop() {
    let _ = tokio::signal::ctrl_c().await;
}
