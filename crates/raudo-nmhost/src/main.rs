//! Native Messaging host: bridges the browser extension to raudo-daemon over D-Bus.
//!
//! Browser side framing (Chrome and Firefox): `u32` little-endian length + UTF-8 JSON.
//! Every request carries an `id` that is echoed in its response. stdout carries
//! protocol frames only; diagnostics go to stderr (shown in the browser console).

use raudo_proto::{AddDownload, ManagerProxy, Request, VideoOptions};
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
    manager: Option<ManagerProxy<'static>>,
}

impl Host {
    async fn manager(&mut self) -> Result<&ManagerProxy<'static>, String> {
        if self.manager.is_none() {
            let conn = zbus::Connection::session().await.map_err(|e| format!("no D-Bus session: {e}"))?;
            let proxy = ManagerProxy::new(&conn).await.map_err(|e| e.to_string())?;
            self.manager = Some(proxy);
        }
        Ok(self.manager.as_ref().expect("just set"))
    }

    async fn handle(&mut self, msg: Msg) -> Result<Value, String> {
        match msg {
            Msg::Hello { browser } => {
                eprintln!("raudo-nmhost: hello from {}", browser.as_deref().unwrap_or("?"));
                // Reachability check, so the extension can show "daemon not running".
                let daemon = match self.manager().await {
                    Ok(m) => m.list().await.is_ok(),
                    Err(_) => false,
                };
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
                let body = serde_json::to_string(&add).expect("serializable");
                let id = self
                    .call(|m| {
                        let body = body.clone();
                        async move { m.add(&body).await }
                    })
                    .await?;
                Ok(json!({ "downloadId": id }))
            }
            Msg::Formats(p) => {
                let body = serde_json::to_string(&p.request()).expect("serializable");
                let info = self
                    .call(|m| {
                        let body = body.clone();
                        async move { m.formats(&body).await }
                    })
                    .await?;
                let info: Value = serde_json::from_str(&info).map_err(|e| e.to_string())?;
                Ok(json!({ "media": info }))
            }
            Msg::List => {
                let list = self.call(|m| async move { m.list().await }).await?;
                let mut list: Vec<Value> = serde_json::from_str(&list).map_err(|e| e.to_string())?;
                // Newest first, unfinished on top; cap so the reply stays far below 1 MiB.
                list.reverse();
                list.sort_by_key(|d| d["status"] == "completed");
                list.truncate(30);
                Ok(json!({ "downloads": list }))
            }
        }
    }

    /// Runs a D-Bus call; on a stale connection reconnects once.
    async fn call<T, F, Fut>(&mut self, f: F) -> Result<T, String>
    where
        F: Fn(ManagerProxy<'static>) -> Fut,
        Fut: std::future::Future<Output = zbus::Result<T>>,
    {
        let m = self.manager().await?.clone();
        match f(m).await {
            Ok(v) => Ok(v),
            Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" => {
                Err("raudo-daemon is not running".into())
            }
            Err(zbus::Error::MethodError(name, msg, _)) => Err(msg.unwrap_or_else(|| name.to_string())),
            Err(_) => {
                self.manager = None;
                let m = self.manager().await?.clone();
                f(m).await.map_err(|e| match e {
                    zbus::Error::MethodError(name, msg, _) => msg.unwrap_or_else(|| name.to_string()),
                    e => format!("raudo daemon not reachable: {e}"),
                })
            }
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

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut host = Host { manager: None };

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
