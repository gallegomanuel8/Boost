use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Parser;
use raudo_core::{Dest, Event, Job, Limiter, Options, Outcome, Request, State};
use tokio::sync::{mpsc, watch};

/// Test front end for the raudo engine. Ctrl-C pauses; run again to resume.
#[derive(Parser)]
#[command(version)]
struct Args {
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
    let args = Args::parse();
    match run(args).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("\nerror: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
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
