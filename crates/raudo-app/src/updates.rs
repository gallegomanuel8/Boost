//! Update check: asks GitHub for the latest Boost release (at start-up and every
//! few hours) and tells the window when there is a newer one.
//!
//! macOS installs it by itself with `cargo-packager-updater`: the release's
//! `latest.json` names a `Boost.app.tar.gz` and its signature, checked against
//! the project's public key before the bundle is replaced. Linux and Windows
//! only get the release page (their packages are installed by their own scripts).
//!
//! The network calls of the updater are blocking: they run off the async workers.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use cargo_packager_updater::semver::Version;
use cargo_packager_updater::{Config, Update, UpdaterBuilder};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

const REPO: &str = "gallegomanuel8/Boost";
/// Manifest of the latest release for the self-installing platforms (macOS).
const MANIFEST: &str = "https://github.com/gallegomanuel8/Boost/releases/latest/download/latest.json";
/// Key the updates are signed with (the private one is kept out of the repository).
const PUBLIC_KEY: &str = include_str!("../updater/public-key.txt");
/// After start-up, so the check does not compete with resuming downloads.
const FIRST_CHECK: Duration = Duration::from_secs(20);
const EVERY: Duration = Duration::from_secs(6 * 3600);

/// A newer release, as the window shows it.
#[derive(Debug, Clone, Serialize)]
pub struct Available {
    pub version: String,
    /// Release page: notes and downloads.
    pub page: String,
    pub notes: String,
    /// Boost can download and install it by itself (macOS app bundle).
    pub installable: bool,
}

/// Progress of an installation, sent to the window as `update-progress`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "stage", rename_all = "lowercase")]
pub enum Progress {
    /// Fraction downloaded, when the size is known.
    Downloading { fraction: Option<f32> },
    Installing,
    Failed { error: String },
}

/// The installable update found by the last check (macOS).
#[derive(Default)]
pub struct Pending(Mutex<Option<Update>>);

/// Turning the automatic check off leaves this file behind.
fn opt_out_marker() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("boost").join("no-update-check"))
}

pub fn automatic() -> bool {
    opt_out_marker().is_some_and(|m| !m.exists())
}

pub fn set_automatic(enabled: bool) -> std::io::Result<()> {
    let Some(marker) = opt_out_marker() else { return Ok(()) };
    if enabled {
        match std::fs::remove_file(marker) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    } else {
        std::fs::create_dir_all(marker.parent().expect("has a parent"))?;
        std::fs::write(marker, b"")
    }
}

fn current() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the package version is semver")
}

/// `v0.2.0` → 0.2.0; `None` for tags that are not versions.
fn tag_version(tag: &str) -> Option<Version> {
    Version::parse(tag.trim().trim_start_matches('v')).ok()
}

/// The latest published release (drafts and pre-releases are not "latest").
async fn latest_release() -> Result<(Version, String, String), String> {
    let url = std::env::var("BOOST_UPDATE_API").unwrap_or_else(|_| format!("https://api.github.com/repos/{REPO}/releases/latest"));
    let body = reqwest::Client::new()
        .get(url)
        .header("User-Agent", concat!("Boost/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
    let version = tag_version(&s("tag_name")).ok_or("the latest release has no version tag")?;
    Ok((version, s("html_url"), s("body")))
}

/// The app bundle can replace itself only when run as an .app (not from a cargo build).
fn self_installing() -> bool {
    cfg!(target_os = "macos")
        && std::env::current_exe().is_ok_and(|e| e.ancestors().nth(3).is_some_and(|b| b.extension().is_some_and(|x| x == "app")))
}

/// The signed package of `version` for this platform, from the release's `latest.json`.
fn installable_update(version: &Version) -> Result<Option<Update>, String> {
    let endpoint = std::env::var("BOOST_UPDATE_MANIFEST").unwrap_or_else(|_| MANIFEST.to_owned());
    let config = Config {
        endpoints: vec![endpoint.parse().map_err(|e: cargo_packager_updater::url::ParseError| e.to_string())?],
        pubkey: PUBLIC_KEY.trim().to_owned(),
        windows: None,
    };
    let updater = UpdaterBuilder::new(current(), config)
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    // A manifest left behind by an older release must not install that one.
    Ok(updater.check().map_err(|e| e.to_string())?.filter(|u| tag_version(&u.version).as_ref() == Some(version)))
}

/// Checks for a newer release. `Ok(None)`: Boost is up to date.
pub async fn check(app: &AppHandle) -> Result<Option<Available>, String> {
    let (version, page, notes) = latest_release().await?;
    if version <= current() {
        return Ok(None);
    }
    let mut installable = false;
    if self_installing() {
        let v = version.clone();
        let found = tauri::async_runtime::spawn_blocking(move || installable_update(&v))
            .await
            .map_err(|e| e.to_string())?;
        match found {
            Ok(update) => {
                installable = update.is_some();
                *app.state::<Pending>().0.lock().unwrap() = update;
            }
            // Released without a macOS package (yet): still worth announcing.
            Err(e) => tracing::info!("no installable update for {version}: {e}"),
        }
    }
    Ok(Some(Available { version: version.to_string(), page, notes, installable }))
}

/// Checks now and then while Boost runs; the window hears of a new release as
/// `update-available`.
pub async fn watch(app: AppHandle) {
    tokio::time::sleep(FIRST_CHECK).await;
    loop {
        if automatic() {
            match check(&app).await {
                Ok(Some(found)) => {
                    tracing::info!(version = %found.version, installable = found.installable, "update available");
                    let _ = app.emit("update-available", &found);
                }
                Ok(None) => {}
                Err(e) => tracing::info!("update check failed: {e}"),
            }
        }
        tokio::time::sleep(EVERY).await;
    }
}

/// Downloads, verifies and installs the update found by the last check, telling
/// the window how it goes. On success the caller restarts Boost.
pub async fn install(app: &AppHandle) -> Result<(), String> {
    let update = app.state::<Pending>().0.lock().unwrap().take().ok_or("no hay ninguna actualización que instalar")?;
    let progress = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let done = std::sync::atomic::AtomicUsize::new(0);
        let bytes = update.download_extended(
            |chunk, total| {
                let done = done.fetch_add(chunk, std::sync::atomic::Ordering::Relaxed) + chunk;
                let fraction = total.filter(|t| *t > 0).map(|t| (done as f32 / t as f32).min(1.0));
                let _ = progress.emit("update-progress", Progress::Downloading { fraction });
            },
            || {},
        )?;
        let _ = progress.emit("update-progress", Progress::Installing);
        // Asks for an administrator password when the app's folder needs it.
        update.install(bytes)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string());
    if let Err(error) = &result {
        let _ = app.emit("update-progress", Progress::Failed { error: error.clone() });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_is_valid() {
        assert!(PUBLIC_KEY.trim().len() > 40);
        assert!(MANIFEST.parse::<cargo_packager_updater::url::Url>().is_ok());
        current();
    }

    #[test]
    fn tags_are_versions() {
        assert_eq!(tag_version("v0.1.4"), Some(Version::new(0, 1, 4)));
        assert_eq!(tag_version("0.2.0"), Some(Version::new(0, 2, 0)));
        assert_eq!(tag_version("latest"), None);
        assert!(tag_version("v0.1.10").unwrap() > tag_version("v0.1.9").unwrap());
    }
}
