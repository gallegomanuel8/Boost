//! `raudo-nmhost register | unregister`: tells the browsers where this host is.
//!
//! Windows: manifests next to this executable, referenced from
//! `HKCU\Software\<browser>\NativeMessagingHosts\org.raudo.nmhost`.
//! Linux: manifests in each installed browser's per-user folder (sandboxed
//! Flatpak browsers are set up by scripts/install-dev.sh instead).

use std::path::{Path, PathBuf};

use serde_json::json;

pub const HOST_NAME: &str = "org.raudo.nmhost";
/// Pinned by the public key in the extension's manifest.
pub const CHROME_ID: &str = "boloeecmdjhgdkcahinhhpakjhiljbhi";
pub const FIREFOX_ID: &str = "raudo@raudo.org";

fn chromium_manifest(host: &Path) -> serde_json::Value {
    json!({
        "name": HOST_NAME,
        "description": "raudo download manager",
        "path": host,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{CHROME_ID}/")],
    })
}

fn firefox_manifest(host: &Path) -> serde_json::Value {
    json!({
        "name": HOST_NAME,
        "description": "raudo download manager",
        "path": host,
        "type": "stdio",
        "allowed_extensions": [FIREFOX_ID],
    })
}

fn write_json(path: &Path, v: &serde_json::Value) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(v).expect("serializable"))
}

fn this_exe() -> std::io::Result<PathBuf> {
    std::env::current_exe()?.canonicalize()
}

#[cfg(windows)]
mod imp {
    use super::*;
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    /// Registry roots per browser; each reads its own key (Brave and Vivaldi also
    /// read Chrome's, which is harmless).
    const CHROMIUM_KEYS: &[&str] = &[
        r"Software\Google\Chrome\NativeMessagingHosts",
        r"Software\Chromium\NativeMessagingHosts",
        r"Software\BraveSoftware\Brave-Browser\NativeMessagingHosts",
        r"Software\Microsoft\Edge\NativeMessagingHosts",
        r"Software\Vivaldi\NativeMessagingHosts",
    ];
    const FIREFOX_KEY: &str = r"Software\Mozilla\NativeMessagingHosts";

    pub fn register() -> std::io::Result<Vec<String>> {
        let host = this_exe()?;
        let dir = host.parent().expect("executable has a folder").to_path_buf();
        let chromium = dir.join(format!("{HOST_NAME}.chromium.json"));
        let firefox = dir.join(format!("{HOST_NAME}.firefox.json"));
        write_json(&chromium, &chromium_manifest(&host))?;
        write_json(&firefox, &firefox_manifest(&host))?;

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let mut done = Vec::new();
        for root in CHROMIUM_KEYS {
            let (key, _) = hkcu.create_subkey(format!(r"{root}\{HOST_NAME}"))?;
            key.set_value("", &chromium.display().to_string())?;
            done.push(root.to_string());
        }
        let (key, _) = hkcu.create_subkey(format!(r"{FIREFOX_KEY}\{HOST_NAME}"))?;
        key.set_value("", &firefox.display().to_string())?;
        done.push(FIREFOX_KEY.to_string());
        Ok(done)
    }

    pub fn unregister() -> std::io::Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        for root in CHROMIUM_KEYS.iter().chain([&FIREFOX_KEY]) {
            let _ = hkcu.delete_subkey_all(format!(r"{root}\{HOST_NAME}"));
        }
        if let Some(dir) = this_exe()?.parent() {
            let _ = std::fs::remove_file(dir.join(format!("{HOST_NAME}.chromium.json")));
            let _ = std::fs::remove_file(dir.join(format!("{HOST_NAME}.firefox.json")));
        }
        Ok(())
    }
}

#[cfg(unix)]
mod imp {
    use super::*;

    /// Per-user manifest folders; only browsers whose config folder exists get one.
    fn chromium_dirs(home: &Path) -> Vec<PathBuf> {
        ["google-chrome", "google-chrome-beta", "chromium", "BraveSoftware/Brave-Browser", "microsoft-edge", "vivaldi"]
            .iter()
            .map(|d| home.join(".config").join(d))
            .filter(|d| d.is_dir())
            .map(|d| d.join("NativeMessagingHosts"))
            .collect()
    }

    /// Firefox reads ~/.mozilla even when its profile uses ~/.config/mozilla.
    fn firefox_dir(home: &Path) -> Option<PathBuf> {
        let has_firefox = home.join(".mozilla").is_dir() || home.join(".config/mozilla").is_dir();
        has_firefox.then(|| home.join(".mozilla/native-messaging-hosts"))
    }

    pub fn register() -> std::io::Result<Vec<String>> {
        let host = this_exe()?;
        let home = dirs_home()?;
        let mut done = Vec::new();
        for dir in chromium_dirs(&home) {
            write_json(&dir.join(format!("{HOST_NAME}.json")), &chromium_manifest(&host))?;
            done.push(dir.display().to_string());
        }
        if let Some(dir) = firefox_dir(&home) {
            write_json(&dir.join(format!("{HOST_NAME}.json")), &firefox_manifest(&host))?;
            done.push(dir.display().to_string());
        }
        Ok(done)
    }

    pub fn unregister() -> std::io::Result<()> {
        let home = dirs_home()?;
        for dir in chromium_dirs(&home).into_iter().chain(firefox_dir(&home)) {
            let _ = std::fs::remove_file(dir.join(format!("{HOST_NAME}.json")));
        }
        Ok(())
    }

    fn dirs_home() -> std::io::Result<PathBuf> {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME is not set"))
    }
}

pub use imp::{register, unregister};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifests_name_the_right_extensions() {
        let host = Path::new("/opt/raudo/raudo-nmhost");
        let c = chromium_manifest(host);
        assert_eq!(c["allowed_origins"][0], "chrome-extension://boloeecmdjhgdkcahinhhpakjhiljbhi/");
        assert_eq!(c["path"], "/opt/raudo/raudo-nmhost");
        let f = firefox_manifest(host);
        assert_eq!(f["allowed_extensions"][0], "raudo@raudo.org");
        assert_eq!(f["type"], "stdio");
    }
}
