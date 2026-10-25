//! Browsers raudo has an extension for: whether each one has it installed,
//! and how to install it.
//!
//! Detection reads the browsers' profile folders, so it works while the
//! browser is closed: Firefox keeps installed add-ons as `extensions/<id>.xpi`,
//! Chromium-family browsers list them (unpacked ones too) under
//! `extensions.settings` in each profile's preferences.

use std::path::{Path, PathBuf};

use serde::Serialize;

const FIREFOX_ID: &str = "boost-downloader@boost.app";
/// The add-on's ID while the app was called raudo.
const OLD_FIREFOX_ID: &str = "raudo@raudo.org";
const CHROME_ID: &str = "boloeecmdjhgdkcahinhhpakjhiljbhi";

#[derive(Debug, Serialize)]
pub struct WebExtension {
    pub browser: &'static str,
    pub name: &'static str,
    /// The browser's profile folder exists.
    pub present: bool,
    /// Some profile has the extension.
    pub installed: bool,
    /// Only the pre-Boost extension is installed (Firefox).
    pub outdated: bool,
    /// Boost can help install it: the signed .xpi for Firefox, the unpacked
    /// extension folder for Chromium browsers.
    pub available: bool,
    /// Chromium: the folder to choose in "Load unpacked" (the extension's own
    /// copy for Flatpak browsers, which cannot read the rest of the home folder).
    pub folder: Option<PathBuf>,
}

struct Chromium {
    browser: &'static str,
    name: &'static str,
    /// Config dir under ~/.config (and inside the Flatpak's own config).
    linux: &'static str,
    flatpak: &'static str,
    /// User data dir under %LOCALAPPDATA%.
    windows: &'static str,
    /// User data dir under ~/Library/Application Support.
    macos: &'static str,
}

const CHROMIUM: &[Chromium] = &[
    Chromium { browser: "brave", name: "Brave", linux: "BraveSoftware/Brave-Browser", flatpak: "com.brave.Browser", windows: r"BraveSoftware\Brave-Browser\User Data", macos: "BraveSoftware/Brave-Browser" },
    Chromium { browser: "chrome", name: "Google Chrome", linux: "google-chrome", flatpak: "com.google.Chrome", windows: r"Google\Chrome\User Data", macos: "Google/Chrome" },
    Chromium { browser: "edge", name: "Microsoft Edge", linux: "microsoft-edge", flatpak: "com.microsoft.Edge", windows: r"Microsoft\Edge\User Data", macos: "Microsoft Edge" },
    Chromium { browser: "chromium", name: "Chromium", linux: "chromium", flatpak: "org.chromium.Chromium", windows: r"Chromium\User Data", macos: "Chromium" },
];

pub fn list(xpi: Option<&Path>) -> Vec<WebExtension> {
    let firefox = firefox_roots();
    let has = |id: &str| {
        firefox.iter().flat_map(|r| subdirs(r)).any(|p| p.join("extensions").join(format!("{id}.xpi")).is_file())
    };
    let installed = has(FIREFOX_ID);
    let mut out = vec![WebExtension {
        browser: "firefox",
        name: "Firefox",
        present: firefox.iter().any(|r| r.is_dir()),
        installed,
        outdated: !installed && has(OLD_FIREFOX_ID),
        available: xpi.is_some(),
        folder: None,
    }];
    for c in CHROMIUM {
        let roots = chromium_roots(c);
        let folder = chromium_extension_folder(c);
        out.push(WebExtension {
            browser: c.browser,
            name: c.name,
            present: roots.iter().any(|r| r.is_dir()),
            installed: roots.iter().flat_map(|r| subdirs(r)).any(|p| chromium_profile_has_raudo(&p)),
            outdated: false,
            available: folder.is_some(),
            folder,
        });
    }
    out
}

/// Where the installer left the unpacked extension for this browser.
fn chromium_extension_folder(c: &Chromium) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if cfg!(windows) {
        // %LOCALAPPDATA%\Programs\Boost\extension, next to boost.exe.
        candidates.extend(std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("extension"))));
    } else if cfg!(target_os = "macos") {
        // Copied out of the app bundle at startup: "Load unpacked" cannot look inside an .app.
        candidates.extend(dirs::data_dir().map(|d| d.join("boost/extension")));
    } else {
        let native = dirs::config_dir().is_some_and(|d| d.join(c.linux).is_dir());
        let flatpak = dirs::home_dir().map(|h| h.join(".var/app").join(c.flatpak));
        if !native {
            candidates.extend(flatpak.map(|f| f.join("data/raudo/extension")));
        }
        candidates.extend(dirs::data_dir().map(|d| d.join("boost/extension")));
    }
    candidates.into_iter().find(|p| p.join("manifest.json").is_file())
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default()
}

/// Copies the extension shipped in the app bundle to where Chromium browsers
/// can load it unpacked, when that copy is missing or from another version.
pub fn sync_extension_folder(resource_dir: Option<PathBuf>) -> std::io::Result<()> {
    let (Some(src), Some(dst)) = (resource_dir.map(|d| d.join("extension")), dirs::data_dir().map(|d| d.join("boost/extension")))
    else {
        return Ok(());
    };
    let manifest = |dir: &Path| std::fs::read(dir.join("manifest.json")).ok();
    if manifest(&src).is_none() || manifest(&src) == manifest(&dst) {
        return Ok(());
    }
    let _ = std::fs::remove_dir_all(&dst);
    copy_dir(&src, &dst)
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}

/// Folders that hold Firefox profiles (each subfolder is one).
fn firefox_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if cfg!(windows) {
        if let Some(app_data) = dirs::config_dir() {
            roots.push(app_data.join(r"Mozilla\Firefox\Profiles"));
        }
    } else if cfg!(target_os = "macos") {
        roots.extend(dirs::data_dir().map(|d| d.join("Firefox/Profiles")));
    } else if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".mozilla/firefox"));
        roots.push(dirs::config_dir().unwrap_or_else(|| home.join(".config")).join("mozilla/firefox"));
        roots.push(home.join(".var/app/org.mozilla.firefox/.mozilla/firefox"));
        roots.push(home.join(".var/app/org.mozilla.firefox/config/mozilla/firefox"));
        roots.push(home.join("snap/firefox/common/.mozilla/firefox"));
    }
    roots
}

fn chromium_roots(c: &Chromium) -> Vec<PathBuf> {
    if cfg!(windows) {
        return dirs::data_local_dir().map(|d| vec![d.join(c.windows)]).unwrap_or_default();
    }
    if cfg!(target_os = "macos") {
        return dirs::data_dir().map(|d| vec![d.join(c.macos)]).unwrap_or_default();
    }
    let mut roots = Vec::new();
    if let Some(config) = dirs::config_dir() {
        roots.push(config.join(c.linux));
    }
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".var/app").join(c.flatpak).join("config").join(c.linux));
    }
    roots
}

fn chromium_profile_has_raudo(profile: &Path) -> bool {
    ["Secure Preferences", "Preferences"].iter().any(|f| {
        std::fs::read(profile.join(f))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .is_some_and(|v| v.pointer(&format!("/extensions/settings/{CHROME_ID}")).is_some())
    })
}

/// The signed Firefox extension: where the installer puts it (Linux:
/// ~/.local/share/boost, Windows: next to boost.exe, macOS: the bundle's
/// Resources), or the newest one
/// `scripts/sign-extension.sh` produced when running from the source tree.
pub fn find_xpi(resource_dir: Option<PathBuf>) -> Option<PathBuf> {
    let installed = [
        dirs::data_dir().map(|d| d.join("boost").join("boost.xpi")),
        std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("boost.xpi"))),
        resource_dir.map(|d| d.join("boost.xpi")),
    ];
    if let Some(p) = installed.into_iter().flatten().find(|p| p.is_file()) {
        return Some(p);
    }
    let artifacts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web-ext-artifacts");
    std::fs::read_dir(artifacts)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".xpi"))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

/// Opens the .xpi with Firefox, which then asks the user to add the extension.
pub fn install_firefox(xpi: &Path) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        // A second firefox process refuses to start while one runs: let
        // LaunchServices hand the file to the running one (or start it).
        let status = std::process::Command::new("open")
            .args(["-a", "Firefox"])
            .arg(xpi)
            .status()
            .map_err(|e| format!("No se pudo abrir Firefox: {e}"))?;
        return if status.success() { Ok(()) } else { Err("No se ha encontrado Firefox en este equipo".into()) };
    }
    let exe = firefox_exe().ok_or("No se ha encontrado Firefox en este equipo")?;
    let mut child = std::process::Command::new(exe)
        .arg(xpi)
        .spawn()
        .map_err(|e| format!("No se pudo abrir Firefox: {e}"))?;
    // A running Firefox hands the file to its window and this process exits at once.
    std::thread::spawn(move || child.wait());
    Ok(())
}

fn firefox_exe() -> Option<PathBuf> {
    if cfg!(windows) {
        ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
            .iter()
            .filter_map(std::env::var_os)
            .map(|d| PathBuf::from(d).join(r"Mozilla Firefox\firefox.exe"))
            .find(|p| p.is_file())
    } else {
        std::env::var_os("PATH")
            .into_iter()
            .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
            .map(|d| d.join("firefox"))
            .find(|p| p.is_file())
    }
}
