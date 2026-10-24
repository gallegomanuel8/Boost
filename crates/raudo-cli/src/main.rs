use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};
use raudo_core::{Dest, Event, Job, Limiter, Options, Outcome, Request, State};
use raudo_ipc::{Client, Event as IpcEvent};
use raudo_proto::{AddDownload, DownloadInfo, Status, VideoOptions};
use tokio::sync::{mpsc, watch};

#[derive(Parser)]
#[command(version, about = "raudo download manager")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Download directly, without the daemon. Ctrl-C pauses; run again to resume.
    Get(GetArgs),
    /// Queue a download in the daemon.
    Add {
        url: String,
        #[arg(short, long)]
        dir: Option<PathBuf>,
        #[arg(short, long)]
        output: Option<String>,
        #[arg(long)]
        paused: bool,
        #[arg(long)]
        referer: Option<String>,
        #[arg(long)]
        cookies: Option<String>,
        /// Download with yt-dlp (video pages, HLS/DASH).
        #[arg(long)]
        video: bool,
        /// yt-dlp format selector (implies --video), see `formats`.
        #[arg(short, long)]
        format: Option<String>,
        /// Keep only the audio, as MP3 at this bitrate in kbit/s (0 = best VBR).
        #[arg(long, value_name = "KBPS")]
        mp3: Option<u32>,
        /// Name to use when the server's file name looks random.
        #[arg(long)]
        title: Option<String>,
    },
    /// Show the formats yt-dlp offers for a video page.
    Formats { url: String },
    /// List downloads.
    List,
    Pause { id: i64 },
    Resume { id: i64 },
    /// Remove from the list (and the file with --delete).
    Rm {
        id: i64,
        #[arg(long)]
        delete: bool,
    },
    /// Global speed limit in KiB/s (0 = unlimited).
    Limit { kib: u64 },
    /// Parallel downloads in the main queue.
    Concurrent { n: u32 },
    /// Follow progress until interrupted.
    Watch,
}

#[derive(clap::Args)]
struct GetArgs {
    url: String,
    /// Output directory (name from the server) or, with --output, exact file.
    #[arg(short, long, default_value = ".")]
    dir: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(short, long, default_value_t = 8)]
    connections: usize,
    /// Speed limit in KiB/s.
    #[arg(short, long)]
    limit: Option<u64>,
    #[arg(long)]
    referer: Option<String>,
    #[arg(long)]
    cookies: Option<String>,
    #[arg(long)]
    user_agent: Option<String>,
}

fn state_path(dir: &Path, url: &str) -> PathBuf {
    // One sidecar per URL in the target dir; the daemon will use SQLite instead.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in url.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    dir.join(format!(".{h:016x}.raudo.json"))
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let result = match Cli::parse().cmd {
        Cmd::Get(args) => get(args).await,
        cmd => daemon(cmd).await,
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("\nerror: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

type AnyResult = Result<(), Box<dyn std::error::Error>>;

async fn daemon(cmd: Cmd) -> AnyResult {
    let mut mgr = Client::connect("cli").await?;
    match cmd {
        Cmd::Get(_) => unreachable!(),
        Cmd::Add { url, dir, output, paused, referer, cookies, video, format, mp3, title } => {
            let dir = dir.map(|d| std::path::absolute(d).map(|p| p.to_string_lossy().into_owned())).transpose()?;
            let add = AddDownload {
                request: Request { url, referer, cookies, ..Default::default() },
                dir,
                filename: output,
                title,
                paused,
                video: (video || format.is_some() || mp3.is_some())
                    .then_some(VideoOptions { format, audio_kbps: mp3 }),
            };
            let id = mgr.add(add).await?;
            println!("{id}");
        }
        Cmd::Formats { url } => {
            let req = Request { url, ..Default::default() };
            let info = mgr.formats(req).await?;
            println!("{}", info.title);
            println!("{:<12} {:<5} {:<11} {:>5} {:<14} {:<12} {:>10}  NOTE", "ID", "EXT", "RESOLUTION", "FPS", "VIDEO", "AUDIO", "SIZE");
            for f in &info.formats {
                println!(
                    "{:<12} {:<5} {:<11} {:>5} {:<14} {:<12} {:>10}  {}",
                    f.id,
                    f.ext,
                    f.resolution.as_deref().unwrap_or("-"),
                    f.fps.map(|v| format!("{v:.0}")).unwrap_or_default(),
                    f.vcodec.as_deref().unwrap_or("-"),
                    f.acodec.as_deref().unwrap_or("-"),
                    f.filesize.map(human).unwrap_or_default(),
                    f.note.as_deref().unwrap_or("")
                );
            }
        }
        Cmd::List => {
            let list = mgr.list().await?;
            println!("{:>4}  {:<11}  {:>6}  {:>10}  {:>11}  NAME", "ID", "STATUS", "%", "SIZE", "SPEED");
            for d in &list {
                print_row(d);
            }
        }
        Cmd::Pause { id } => mgr.pause(id).await?,
        Cmd::Resume { id } => mgr.resume(id).await?,
        Cmd::Rm { id, delete } => mgr.remove(id, delete).await?,
        Cmd::Limit { kib } => mgr.set_speed_limit(kib).await?,
        Cmd::Concurrent { n } => mgr.set_max_concurrent(n).await?,
        Cmd::Watch => {
            mgr.subscribe().await?;
            while let Some(ev) = mgr.next_event().await? {
                if let IpcEvent::Changed { info } = ev {
                    print_row(&info);
                }
            }
        }
    }
    Ok(())
}

fn print_row(d: &DownloadInfo) {
    let pct = d.size.map(|s| format!("{:.1}", d.downloaded as f64 * 100.0 / s.max(1) as f64)).unwrap_or("-".into());
    let speed = if d.status == Status::Downloading { format!("{}/s", human(d.speed as u64)) } else { String::new() };
    let mut name = d.filename.clone().unwrap_or_else(|| d.url.clone());
    if let Some(e) = &d.error {
        name.push_str(&format!("  [{e}]"));
    }
    println!(
        "{:>4}  {:<11}  {:>6}  {:>10}  {:>11}  {}",
        d.id,
        d.status.as_str(),
        pct,
        d.size.map(human).unwrap_or("?".into()),
        speed,
        name
    );
}

async fn get(args: GetArgs) -> AnyResult {
    let dir = match &args.output {
        Some(p) => p.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")).to_path_buf(),
        None => args.dir.clone(),
    };
    let sidecar = state_path(&dir, &args.url);
    let resume: Option<State> = std::fs::read(&sidecar).ok().and_then(|b| serde_json::from_slice(&b).ok());
    if resume.is_some() {
        eprintln!("resuming from {}", sidecar.display());
    }

    let job = Job {
        request: Request {
            url: args.url.clone(),
            referer: args.referer,
            cookies: args.cookies,
            user_agent: args.user_agent,
            ..Default::default()
        },
        dest: match args.output {
            Some(p) => Dest::File(p),
            None => Dest::Dir(args.dir),
        },
        resume,
        temp_dir: None,
        title_hint: None,
    };
    let opts = Options { max_connections: args.connections, ..Default::default() };
    let limiter = Arc::new(Limiter::new(args.limit.map(|k| k * 1024)));
    let client = raudo_core::client()?;

    let (tx, mut rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = watch::channel(false);
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("\npausing...");
            let _ = cancel_tx.send(true);
        }
    });

    let side = sidecar.clone();
    let printer = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                Event::Started(s) => eprintln!(
                    "{} — {} — {}",
                    s.filename.as_deref().unwrap_or("?"),
                    s.size.map(human).unwrap_or_else(|| "unknown size".into()),
                    if s.resumable { "resumable" } else { "single connection" }
                ),
                Event::Progress { downloaded, total, bytes_per_sec, connections } => {
                    let pct = total.map(|t| format!("{:5.1}%", downloaded as f64 * 100.0 / t.max(1) as f64));
                    eprint!(
                        "\r{} {} / {}  {}/s  conns {}   ",
                        pct.unwrap_or_default(),
                        human(downloaded),
                        total.map(human).unwrap_or_else(|| "?".into()),
                        human(bytes_per_sec as u64),
                        connections
                    );
                    let _ = std::io::stderr().flush();
                }
                Event::Snapshot(s) => {
                    if let Ok(json) = serde_json::to_vec_pretty(&s) {
                        let _ = std::fs::write(&side, json);
                    }
                }
                Event::Notice(n) => eprintln!("\n{n}"),
            }
        }
    });

    let outcome = raudo_core::run(&client, job, &opts, limiter, tx, cancel_rx).await;
    printer.await?;
    match outcome? {
        Outcome::Completed { path, .. } => {
            let _ = std::fs::remove_file(&sidecar);
            eprintln!("\nsaved {}", path.display());
        }
        Outcome::Paused(_) => eprintln!("\npaused; run the same command to resume"),
    }
    Ok(())
}

fn human(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{n} B") } else { format!("{v:.1} {}", UNITS[u]) }
}
