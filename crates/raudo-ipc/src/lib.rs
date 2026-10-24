//! Local IPC between the raudo service (inside the app, or `raudo-daemon`) and
//! its clients (CLI, browser native host). Replaces D-Bus so it works on Windows
//! too: a Unix socket on Linux, a named pipe on Windows, both through tokio.
//!
//! Wire format: one JSON value per line. The client first sends
//! `{"token":"…","client":"…"}` (the token file is readable by this user only),
//! then requests `{"id":1,"method":"add","params":{…}}`; replies are
//! `{"id":1,"ok":true,"result":…}` or `{"id":1,"ok":false,"error":"…"}`.
//! After `subscribe`, events `{"event":"changed","info":{…}}` arrive on the
//! same connection.

pub mod paths;
mod transport;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use raudo_proto::{AddDownload, DownloadInfo, MediaInfo, Request};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc};

pub use transport::{Conn, Listener};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Call {
    Add(AddDownload),
    List,
    Pause { id: i64 },
    Resume { id: i64 },
    Remove { id: i64, delete_file: bool },
    SetSpeedLimit { kib: u64 },
    SetMaxConcurrent { n: u32 },
    Formats(Request),
    Inspect { target: String, request: Request },
    /// Where one download stands: its file, folder and segment map.
    Details { id: i64 },
    /// Speed limit, parallel downloads, download folder.
    Settings,
    Subscribe,
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RequestMsg {
    id: u64,
    #[serde(flatten)]
    call: Call,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum Event {
    Changed { info: DownloadInfo },
    Removed { id: i64 },
}

#[derive(Debug)]
pub enum ClientError {
    /// No service is listening: the app is not running.
    NotRunning,
    /// The token was refused (stale token file, another user's service).
    Denied,
    Io(String),
    /// The service answered with an error.
    Remote(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::NotRunning => f.write_str("raudo is not running"),
            ClientError::Denied => f.write_str("raudo refused the connection (access token mismatch)"),
            ClientError::Io(e) => write!(f, "connection to raudo failed: {e}"),
            ClientError::Remote(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for ClientError {}

type Result<T> = std::result::Result<T, ClientError>;

fn io_err(e: impl std::fmt::Display) -> ClientError {
    ClientError::Io(e.to_string())
}

// ------------------------------------------------------------------ client

pub struct Client {
    reader: BufReader<tokio::io::ReadHalf<Conn>>,
    writer: tokio::io::WriteHalf<Conn>,
    next_id: u64,
}

impl Client {
    pub async fn connect(client_name: &str) -> Result<Self> {
        let conn = transport::connect().await.map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => ClientError::NotRunning,
            _ => io_err(e),
        })?;
        let token = paths::read_token().map_err(|_| ClientError::NotRunning)?;
        let (r, w) = tokio::io::split(conn);
        let mut client = Client { reader: BufReader::new(r), writer: w, next_id: 1 };
        client.send(&json!({ "token": token, "client": client_name })).await?;
        let reply = client.read_line().await?.ok_or(ClientError::Denied)?;
        if reply.get("ok") != Some(&Value::Bool(true)) {
            return Err(ClientError::Denied);
        }
        Ok(client)
    }

    async fn send(&mut self, v: &Value) -> Result<()> {
        let mut line = serde_json::to_vec(v).expect("serializable");
        line.push(b'\n');
        self.writer.write_all(&line).await.map_err(io_err)?;
        self.writer.flush().await.map_err(io_err)
    }

    async fn read_line(&mut self) -> Result<Option<Value>> {
        let mut line = String::new();
        if self.reader.read_line(&mut line).await.map_err(io_err)? == 0 {
            return Ok(None);
        }
        serde_json::from_str(&line).map(Some).map_err(io_err)
    }

    pub async fn call(&mut self, call: Call) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&serde_json::to_value(RequestMsg { id, call }).expect("serializable")).await?;
        loop {
            let msg = self.read_line().await?.ok_or_else(|| io_err("connection closed"))?;
            if msg.get("id").and_then(Value::as_u64) != Some(id) {
                continue; // an event or another reply
            }
            return if msg.get("ok") == Some(&Value::Bool(true)) {
                Ok(msg.get("result").cloned().unwrap_or(Value::Null))
            } else {
                Err(ClientError::Remote(msg.get("error").and_then(Value::as_str).unwrap_or("error").to_owned()))
            };
        }
    }

    fn parse<T: serde::de::DeserializeOwned>(v: Value) -> Result<T> {
        serde_json::from_value(v).map_err(io_err)
    }

    pub async fn add(&mut self, add: AddDownload) -> Result<i64> {
        Self::parse(self.call(Call::Add(add)).await?)
    }
    pub async fn list(&mut self) -> Result<Vec<DownloadInfo>> {
        Self::parse(self.call(Call::List).await?)
    }
    pub async fn pause(&mut self, id: i64) -> Result<()> {
        self.call(Call::Pause { id }).await.map(drop)
    }
    pub async fn resume(&mut self, id: i64) -> Result<()> {
        self.call(Call::Resume { id }).await.map(drop)
    }
    pub async fn remove(&mut self, id: i64, delete_file: bool) -> Result<()> {
        self.call(Call::Remove { id, delete_file }).await.map(drop)
    }
    pub async fn set_speed_limit(&mut self, kib: u64) -> Result<()> {
        self.call(Call::SetSpeedLimit { kib }).await.map(drop)
    }
    pub async fn set_max_concurrent(&mut self, n: u32) -> Result<()> {
        self.call(Call::SetMaxConcurrent { n }).await.map(drop)
    }
    pub async fn formats(&mut self, request: Request) -> Result<MediaInfo> {
        Self::parse(self.call(Call::Formats(request)).await?)
    }
    pub async fn settings(&mut self) -> Result<Value> {
        self.call(Call::Settings).await
    }

    pub async fn details(&mut self, id: i64) -> Result<Value> {
        self.call(Call::Details { id }).await
    }

    pub async fn inspect(&mut self, target: &str, request: Request) -> Result<Value> {
        self.call(Call::Inspect { target: target.to_owned(), request }).await
    }

    /// Switches the connection to event delivery; read them with [`Client::next_event`].
    pub async fn subscribe(&mut self) -> Result<()> {
        self.call(Call::Subscribe).await.map(drop)
    }

    pub async fn next_event(&mut self) -> Result<Option<Event>> {
        loop {
            let Some(msg) = self.read_line().await? else { return Ok(None) };
            if msg.get("event").is_some() {
                return serde_json::from_value(msg).map(Some).map_err(io_err);
            }
        }
    }
}

// ------------------------------------------------------------------ server

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// What the service exposes; implemented by the download manager.
pub trait Handler: Send + Sync + 'static {
    fn call(&self, call: Call) -> BoxFuture<std::result::Result<Value, String>>;
    fn events(&self) -> broadcast::Receiver<Event>;
}

#[derive(Debug)]
pub enum ServeError {
    /// Another instance answers on the endpoint.
    AlreadyRunning,
    Io(std::io::Error),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServeError::AlreadyRunning => f.write_str("raudo is already running"),
            ServeError::Io(e) => write!(f, "cannot listen for clients: {e}"),
        }
    }
}

impl std::error::Error for ServeError {}

/// Claims the endpoint and writes a fresh token. Fails if another instance runs,
/// leaving that instance's token untouched.
pub async fn listen() -> std::result::Result<(Listener, String), ServeError> {
    if transport::connect().await.is_ok() {
        return Err(ServeError::AlreadyRunning);
    }
    let token = paths::write_new_token().map_err(ServeError::Io)?;
    let listener = transport::bind().map_err(ServeError::Io)?;
    Ok((listener, token))
}

pub async fn serve(mut listener: Listener, token: String, handler: Arc<dyn Handler>) -> std::io::Result<()> {
    let token = Arc::new(token);
    loop {
        let conn = listener.accept().await?;
        tokio::spawn(handle_conn(conn, token.clone(), handler.clone()));
    }
}

/// Equal-time comparison, so the token cannot be guessed byte by byte.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn handle_conn(conn: Conn, token: Arc<String>, handler: Arc<dyn Handler>) {
    let (r, mut w) = tokio::io::split(conn);
    let mut reader = BufReader::new(r);
    let mut line = String::new();
    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
        return;
    }
    let presented = serde_json::from_str::<Value>(&line)
        .ok()
        .and_then(|v| v.get("token").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default();
    if !same(&presented, &token) {
        let _ = w.write_all(b"{\"ok\":false,\"error\":\"access denied\"}\n").await;
        return;
    }
    if w.write_all(b"{\"ok\":true}\n").await.is_err() {
        return;
    }

    // One writer task: replies of concurrent requests and events share the pipe.
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        while let Some(v) = out_rx.recv().await {
            let mut bytes = serde_json::to_vec(&v).expect("serializable");
            bytes.push(b'\n');
            if w.write_all(&bytes).await.is_err() || w.flush().await.is_err() {
                break;
            }
        }
    });

    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let msg: RequestMsg = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                let _ = out_tx.send(json!({ "id": null, "ok": false, "error": format!("bad request: {e}") }));
                continue;
            }
        };
        if let Call::Subscribe = msg.call {
            let mut events = handler.events();
            let tx = out_tx.clone();
            let _ = tx.send(json!({ "id": msg.id, "ok": true }));
            tokio::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(ev) => {
                            if tx.send(serde_json::to_value(ev).expect("serializable")).is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            continue;
        }
        // Requests run concurrently: a slow `formats` must not hold up `list`.
        let tx = out_tx.clone();
        let fut = handler.call(msg.call);
        tokio::spawn(async move {
            let reply = match fut.await {
                Ok(result) => json!({ "id": msg.id, "ok": true, "result": result }),
                Err(error) => json!({ "id": msg.id, "ok": false, "error": error }),
            };
            let _ = tx.send(reply);
        });
    }
    drop(out_tx);
    let _ = writer.await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_shapes() {
        let m = RequestMsg { id: 3, call: Call::Pause { id: 7 } };
        assert_eq!(serde_json::to_value(&m).unwrap(), json!({"id":3,"method":"pause","params":{"id":7}}));
        let l: RequestMsg = serde_json::from_value(json!({"id":1,"method":"list"})).unwrap();
        assert!(matches!(l.call, Call::List));
        let ev = Event::Removed { id: 5 };
        assert_eq!(serde_json::to_value(ev).unwrap(), json!({"event":"removed","id":5}));
        assert!(same("abc", "abc") && !same("abc", "abd") && !same("abc", "ab"));
    }
}
