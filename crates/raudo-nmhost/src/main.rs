//! Native Messaging host: bridges the browser extension to the raudo service over
//! its local IPC (Unix socket / Windows named pipe), starting raudo if needed.
//!
//! Browser side framing (Chrome and Firefox): `u32` little-endian length + UTF-8 JSON.
//! Every request carries an `id` that is echoed in its response. stdout carries
//! protocol frames only; diagnostics go to stderr (shown in the browser console).

use std::time::Duration;

use raudo_ipc::{Client, ClientError};
use raudo_proto::{AddDownload, Request, VideoOptions};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const PROTOCOL: u32 = 1;
/// Browsers refuse host messages above 1 MiB.
const MAX_OUT: usize = 1024 * 1024;
/// Our own sanity cap for incoming messages.
const MAX_IN: usize = 8 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Msg {
    Hello {
        #[serde(default)]
        browser: Option<String>,
    },
    Download(DownloadMsg),
    Formats(PageMsg),
    /// Downloads to show in the popup.
    List,
    /// Page support or HLS stream details, for the popup.
    Inspect(InspectMsg),
    /// Stop a download and discard its partial files.
    Cancel {
        #[serde(rename = "downloadId")]
        download_id: i64,
    },
    /// Start Boost in the background (the popup's "launch" button).
    Launch,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageMsg {
    url: String,
    #[serde(default)]
    referer: Option<String>,
    #[serde(default)]
    cookies: Option<String>,
    #[serde(default)]
    user_agent: Option<String>,
    /// Tab the media was found on, to re-resolve refused stream links.
    #[serde(default)]
    page: Option<String>,
}

#[derive(Deserialize)]
struct InspectMsg {
    /// "page" or "hls"
    target: String,
    #[serde(flatten)]
    page: PageMsg,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadMsg {
    #[serde(flatten)]
    page: PageMsg,
    #[serde(default)]
    filename: Option<String>,
    /// Page title, a name hint for random server names and stream playlists.
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    video: bool,
    #[serde(default)]
    format: Option<String>,
    /// MP3 bitrate in kbit/s: download the audio track only.
    #[serde(default)]
    audio_kbps: Option<u32>,
}

impl PageMsg {
    fn request(self) -> Request {
        Request {
            url: self.url,
            referer: self.referer,
            cookies: self.cookies,
            user_agent: self.user_agent,
            page: self.page,
            ..Default::default()
        }
    }
}

/// Capture rules sent to the extension. They live here, not in the extension,
/// so every browser gets the same behaviour.
fn config() -> Value {
    json!({
        "capture": {
            "enabled": true,
            // Browser downloads smaller than this stay in the browser.
            "minSize": 1024 * 1024,
            "exts": ["zip","rar","7z","tar","gz","xz","bz2","zst","iso","img","dmg","exe","msi","deb","rpm",
                     "appimage","apk","mp4","mkv","webm","avi","mov","flv","m4v","mp3","m4a","flac","ogg",
                     "opus","wav","pdf","epub"],
        },
        // Pages yt-dlp handles better than raw stream capture.
        "videoSites": ["youtube.com","youtu.be","vimeo.com","dailymotion.com","twitch.tv","x.com",
                       "twitter.com","instagram.com","facebook.com","tiktok.com","reddit.com"],
        // Media responses below this size are ignored (thumbnails, ads, segments).
        "mediaMinSize": 512 * 1024,
    })
}

struct Host {
    client: Option<Client>,
}

/// The app (or the headless service) installed next to this host.
fn service_executable() -> Option<std::path::PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let exe = |name: &str| dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    // `raudo` is the app's name before it became Boost.
    [exe("boost"), exe("raudo"), exe("raudo-daemon")].into_iter().find(|p| p.is_file())
}

/// Starts Boost in the background, detached from the browser's process group.
fn launch_service() -> bool {
    let Some(exe) = service_executable() else { return false };
    let mut cmd = std::process::Command::new(&exe);
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    if exe.file_stem().is_some_and(|s| s == "boost" || s == "raudo") {
        cmd.arg("--background");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Own session: closing the browser must not stop the downloads.
        cmd.process_group(0);
        cmd.spawn().is_ok()
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        // Browsers run this host in a job object and kill the whole job, Boost
        // included, when the port closes; Boost has to leave it.
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        keep_std_handles_private();
        let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        if cmd.creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB).spawn().is_ok() {
            return true;
        }
        // A job that forbids leaving it: better started inside than not at all.
        cmd.creation_flags(flags).spawn().is_ok()
    }
}

/// Boost must not inherit the browser's pipes (this host's stdio): it would hold
/// them open after the host exits, and the browser would never see it go.
#[cfg(windows)]
fn keep_std_handles_private() {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    extern "system" {
        fn SetHandleInformation(handle: *mut std::ffi::c_void, mask: u32, flags: u32) -> i32;
    }
    const HANDLE_FLAG_INHERIT: u32 = 1;
    for h in [std::io::stdin().as_raw_handle(), std::io::stdout().as_raw_handle(), std::io::stderr().as_raw_handle()] {
        if !h.is_null() {
            // SAFETY: a std handle of this process; only its inherit flag changes.
            unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0) };
        }
    }
}

fn remote(e: ClientError) -> String {
    match e {
        ClientError::NotRunning => "Boost is not running".into(),
        other => other.to_string(),
    }
}

impl Host {
    /// Connected client. Boost is never started from here unasked: closing the
    /// app stops the downloads, and the popup offers to launch it.
    async fn client(&mut self) -> Result<&mut Client, String> {
        if self.client.is_none() {
            self.client = Some(Client::connect("nmhost").await.map_err(remote)?);
        }
        Ok(self.client.as_mut().expect("just set"))
    }

    /// Starts Boost and waits until it answers. `false` when this host cannot
    /// start it (no Boost next to it, as in a Flatpak sandbox): the extension
    /// then opens the boost:// link, which the desktop hands to Boost.
    async fn launch(&mut self) -> bool {
        if self.call(raudo_ipc::Call::Ping).await.is_ok() {
            return true;
        }
        if !launch_service() {
            return false;
        }
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if let Ok(c) = Client::connect("nmhost").await {
                self.client = Some(c);
                return true;
            }
        }
        false
    }

    /// Runs one request; on a broken connection (raudo restarted) reconnects once.
    async fn call(&mut self, call: raudo_ipc::Call) -> Result<Value, String> {
        match self.client().await?.call(call.clone()).await {
            Ok(v) => Ok(v),
            Err(ClientError::Remote(e)) => Err(e),
            Err(_) => {
                self.client = None;
                self.client().await?.call(call).await.map_err(remote)
            }
        }
    }

    async fn handle(&mut self, msg: Msg) -> Result<Value, String> {
        use raudo_ipc::Call;
        match msg {
            Msg::Hello { browser } => {
                eprintln!("raudo-nmhost: hello from {}", browser.as_deref().unwrap_or("?"));
                // Reachability check, so the extension can show "raudo is not running".
                let daemon = self.call(Call::Ping).await.is_ok();
                Ok(json!({ "protocol": PROTOCOL, "daemon": daemon, "config": config() }))
            }
            Msg::Download(d) => {
                let video = d.video || d.format.is_some() || d.audio_kbps.is_some();
                let add = AddDownload {
                    request: d.page.request(),
                    dir: None,
                    filename: d.filename,
                    title: d.title,
                    paused: false,
                    video: video.then_some(VideoOptions { format: d.format, audio_kbps: d.audio_kbps }),
                };
                let id = self.call(Call::Add(add)).await?;
                Ok(json!({ "downloadId": id }))
            }
            Msg::Formats(p) => {
                let info = self.call(Call::Formats(p.request())).await?;
                Ok(json!({ "media": info }))
            }
            Msg::List => {
                let list = self.call(Call::List).await?;
                let mut list: Vec<Value> = serde_json::from_value(list).map_err(|e| e.to_string())?;
                // Newest first, unfinished on top; cap so the reply stays far below 1 MiB.
                list.reverse();
                list.sort_by_key(|d| d["status"] == "completed");
                list.truncate(30);
                Ok(json!({ "downloads": list }))
            }
            Msg::Inspect(i) => {
                let info = self.call(Call::Inspect { target: i.target, request: i.page.request() }).await?;
                Ok(json!({ "info": info }))
            }
            Msg::Cancel { download_id } => {
                self.call(Call::Remove { id: download_id, delete_file: false }).await?;
                Ok(json!({ "cancelled": download_id }))
            }
            Msg::Launch => Ok(json!({ "daemon": self.launch().await })),
        }
    }
}

async fn read_frame(stdin: &mut tokio::io::Stdin) -> std::io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match stdin.read_exact(&mut len).await {
        Ok(_) => {}
        // Browser closed the port.
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_IN {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "message too large"));
    }
    let mut buf = vec![0u8; len];
    stdin.read_exact(&mut buf).await?;
    Ok(Some(buf))
}

async fn write_frame(stdout: &mut tokio::io::Stdout, v: &Value) -> std::io::Result<()> {
    let mut body = serde_json::to_vec(v).expect("serializable");
    if body.len() > MAX_OUT {
        body = serde_json::to_vec(&json!({ "id": v.get("id"), "ok": false, "error": "response too large" }))
            .expect("serializable");
    }
    stdout.write_all(&(body.len() as u32).to_le_bytes()).await?;
    stdout.write_all(&body).await?;
    stdout.flush().await
}

mod register;

/// Browsers start the host with their own arguments (an origin, a manifest
/// path); only these two words are commands for us.
fn admin_command() -> Option<std::process::ExitCode> {
    let arg = std::env::args().nth(1)?;
    let result = match arg.as_str() {
        "register" => register::register().map(|places| {
            for p in places {
                println!("registered: {p}");
            }
        }),
        "unregister" => register::unregister().map(|_| println!("unregistered")),
        _ => return None,
    };
    Some(match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("raudo-nmhost {arg}: {e}");
            std::process::ExitCode::FAILURE
        }
    })
}

fn main() -> std::process::ExitCode {
    if let Some(code) = admin_command() {
        return code;
    }
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
    match rt.block_on(run_host()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("raudo-nmhost: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run_host() -> std::io::Result<()> {
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut host = Host { client: None };

    while let Some(frame) = read_frame(&mut stdin).await? {
        let raw: Value = match serde_json::from_slice(&frame) {
            Ok(v) => v,
            Err(e) => {
                write_frame(&mut stdout, &json!({ "ok": false, "error": format!("bad JSON: {e}") })).await?;
                continue;
            }
        };
        let id = raw.get("id").cloned().unwrap_or(Value::Null);
        let reply = match serde_json::from_value::<Msg>(raw) {
            Ok(msg) => match host.handle(msg).await {
                Ok(Value::Object(mut body)) => {
                    body.insert("id".into(), id);
                    body.insert("ok".into(), true.into());
                    Value::Object(body)
                }
                Ok(other) => json!({ "id": id, "ok": true, "result": other }),
                Err(e) => json!({ "id": id, "ok": false, "error": e }),
            },
            Err(e) => json!({ "id": id, "ok": false, "error": format!("unknown message: {e}") }),
        };
        write_frame(&mut stdout, &reply).await?;
    }
    Ok(())
}
