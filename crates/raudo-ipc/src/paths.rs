//! Where the service listens and keeps its access token.
//!
//! Linux: `$XDG_RUNTIME_DIR/raudo/` (user-only, tmpfs) holds the socket and the
//! token. Sandboxed browsers (Flatpak) get this one folder mounted with
//! `--filesystem=xdg-run/raudo`. Windows: a per-user named pipe, token in
//! `%LOCALAPPDATA%\raudo\run\`.

use std::path::PathBuf;

pub fn runtime_dir() -> PathBuf {
    #[cfg(unix)]
    {
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
            return PathBuf::from(dir).join("raudo");
        }
    }
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("raudo").join("run")
}

pub fn token_path() -> PathBuf {
    runtime_dir().join("ipc.token")
}

#[cfg(unix)]
pub fn socket_path() -> PathBuf {
    runtime_dir().join("raudo.sock")
}

/// One pipe per user: names are machine-wide, so the user name keeps two
/// sessions apart (the token is what keeps other users out).
#[cfg(windows)]
pub fn pipe_name() -> String {
    let user: String = std::env::var("USERNAME")
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    format!(r"\\.\pipe\raudo-{user}")
}

/// Creates the runtime dir with user-only permissions.
pub fn create_runtime_dir() -> std::io::Result<PathBuf> {
    let dir = runtime_dir();
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

/// Writes a fresh random token readable only by this user and returns it.
pub fn write_new_token() -> std::io::Result<String> {
    create_runtime_dir()?;
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let path = token_path();
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path)?;
        f.write_all(token.as_bytes())?;
    }
    #[cfg(not(unix))]
    std::fs::write(&path, &token)?;
    Ok(token)
}

pub fn read_token() -> std::io::Result<String> {
    Ok(std::fs::read_to_string(token_path())?.trim().to_owned())
}
