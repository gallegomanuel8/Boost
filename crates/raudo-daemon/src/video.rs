//! Video downloads through yt-dlp (sites, HLS/DASH manifests) and ffmpeg (merging).
//!
//! Pausing sends SIGINT; yt-dlp keeps its `.part` files and continues them on the next run.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use raudo_proto::{MediaFormat, MediaInfo, Request};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;

const PROGRESS: &str = "RAUDO-P ";
const FILE: &str = "RAUDO-F ";

pub struct VideoJob<'a> {
    pub request: &'a Request,
    /// Where yt-dlp leaves the finished file (the caller moves it to its destination).
    pub dir: &'a Path,
    /// Fragments and part files.
    pub temp_dir: &'a Path,
    /// File name without extension; `None` = yt-dlp's `title [id]`.
    pub title: Option<&'a str>,
    pub format: Option<&'a str>,
    /// Extract the audio as MP3 at this bitrate (kbit/s, 0 = best VBR).
    pub audio_kbps: Option<u32>,
    /// Bytes per second.
    pub rate_limit: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
    pub speed: f64,
    /// File yt-dlp is writing (a format-specific name before merging).
    pub file: Option<String>,
}

pub enum VideoOutcome {
    Completed(PathBuf),
    Paused,
}

/// yt-dlp from PATH, falling back to `~/.local/bin` (not on PATH under D-Bus activation).
pub fn ytdlp_bin() -> PathBuf {
    if let Some(p) = std::env::var_os("RAUDO_YTDLP") {
        return p.into();
    }
    find_in_path("yt-dlp")
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/bin/yt-dlp")).filter(|p| p.is_file()))
        .unwrap_or_else(|| "yt-dlp".into())
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// YouTube needs a JavaScript runtime to unlock most formats (without one only
/// 144p survives). yt-dlp enables only deno by default; fall back to node.
fn js_runtime_args() -> Vec<String> {
    if find_in_path("deno").is_some() {
        return Vec::new();
    }
    let node = dirs::home_dir()
        .map(|h| h.join(".local/opt/node/bin/node"))
        .filter(|p| p.is_file())
        .or_else(|| find_in_path("node"));
    match node {
        Some(p) => vec!["--js-runtimes".into(), format!("node:{}", p.display())],
        None => Vec::new(),
    }
}

/// `title` usable as a file name stem. Uniqueness is settled when the finished
/// file is moved out of the temp dir, once its real extension is known.
pub fn clean_stem(title: &str) -> String {
    let clean: String = title
        .chars()
        .map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c })
        .collect();
    // Bytes, not characters: emoji-laden titles would otherwise exceed the 255-byte limit.
    let stem = raudo_core::http::truncate_bytes(clean.trim().trim_start_matches('.'), 180).trim_end();
    if stem.is_empty() { "video".into() } else { stem.to_owned() }
}

fn context_args(req: &Request) -> Vec<String> {
    let mut a = Vec::new();
    if let Some(r) = &req.referer {
        a.extend(["--referer".into(), r.clone()]);
    }
    if let Some(ua) = &req.user_agent {
        a.extend(["--user-agent".into(), ua.clone()]);
    }
    // yt-dlp scopes a Cookie header to the URL's domain before using it.
    if let Some(c) = &req.cookies {
        a.extend(["--add-header".into(), format!("Cookie:{c}")]);
    }
    for (k, v) in &req.headers {
        a.extend(["--add-header".into(), format!("{k}:{v}")]);
    }
    a
}

pub fn download_args(job: &VideoJob) -> Vec<String> {
    let mut a: Vec<String> = [
        "--newline",
        "--progress",
        "--no-playlist",
        "--no-simulate",
        "--continue",
        "--concurrent-fragments",
        "4",
        "--progress-template",
        "download:RAUDO-P %(progress)j",
        "--print",
        "after_move:RAUDO-F %(filepath)s",
    ]
    .map(String::from)
    .into();
    let template = match job.title {
        // `%` would be read as a template field.
        Some(t) => format!("{}.%(ext)s", t.replace('%', "%%")),
        None => "%(title).180B [%(id)s].%(ext)s".into(),
    };
    a.extend(["-o".into(), template]);
    a.extend(["-P".into(), format!("home:{}", job.dir.display())]);
    a.extend(["-P".into(), format!("temp:{}", job.temp_dir.display())]);
    a.extend(js_runtime_args());
    match job.audio_kbps {
        Some(kbps) => {
            // Audio-only stream when the site has one (YouTube, DASH). Muxed-only sites (most
            // HLS) carry the same audio in every variant, so a ≤480p one avoids fetching
            // gigabytes of video just to throw it away.
            a.extend(["-f".into(), job.format.unwrap_or("ba/b[height<=480]/w").to_owned()]);
            a.extend(["-x".into(), "--audio-format".into(), "mp3".into()]);
            let quality = if kbps == 0 { "0".to_owned() } else { format!("{kbps}K") };
            a.extend(["--audio-quality".into(), quality]);
        }
        None => a.extend(["-f".into(), job.format.unwrap_or("bv*+ba/b").to_owned()]),
    }
    if let Some(r) = job.rate_limit {
        a.extend(["--limit-rate".into(), r.to_string()]);
    }
    a.extend(context_args(job.request));
    a.push("--".into());
    a.push(job.request.url.clone());
    a
}

/// Aggregates per-file progress: merged formats download video then audio,
/// each restarting from zero.
#[derive(Default)]
pub struct ProgressParser {
    current_file: Option<String>,
    /// Bytes of the files already finished in this run.
    done_before: u64,
    cur_downloaded: u64,
    cur_total: Option<u64>,
}

impl ProgressParser {
    pub fn feed(&mut self, line: &str) -> Option<Progress> {
        let v: Value = serde_json::from_str(line.strip_prefix(PROGRESS)?).ok()?;
        let file = v.get("filename").and_then(Value::as_str).map(str::to_owned);
        if file != self.current_file {
            if self.current_file.is_some() {
                self.done_before += self.cur_total.unwrap_or(0).max(self.cur_downloaded);
            }
            self.current_file = file;
        }
        let num = |k: &str| v.get(k).and_then(Value::as_f64);
        self.cur_downloaded = num("downloaded_bytes").unwrap_or(0.0) as u64;
        self.cur_total = num("total_bytes").or_else(|| num("total_bytes_estimate")).map(|t| t as u64);
        Some(Progress {
            downloaded: self.done_before + self.cur_downloaded,
            total: self.cur_total.map(|t| self.done_before + t),
            speed: num("speed").unwrap_or(0.0),
            file: self.current_file.as_deref().and_then(|f| Path::new(f).file_name()).map(|n| n.to_string_lossy().into_owned()),
        })
    }
}

pub async fn run(
    job: VideoJob<'_>,
    mut on_progress: impl FnMut(Progress),
    mut cancel: watch::Receiver<bool>,
) -> Result<VideoOutcome, String> {
    let mut child = Command::new(ytdlp_bin())
        .args(download_args(&job))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("cannot run yt-dlp: {e}"))?;

    let mut stdout = BufReader::new(child.stdout.take().expect("piped")).lines();
    let mut stderr = BufReader::new(child.stderr.take().expect("piped")).lines();
    let mut parser = ProgressParser::default();
    let mut final_path = None;
    let mut last_error = None;
    let mut paused = false;
    let (mut out_open, mut err_open) = (true, true);

    while out_open || err_open {
        tokio::select! {
            line = stdout.next_line(), if out_open => match line {
                Ok(Some(l)) => {
                    if let Some(p) = l.strip_prefix(FILE) {
                        final_path = Some(PathBuf::from(p));
                    } else if let Some(p) = parser.feed(&l) {
                        on_progress(p);
                    }
                }
                _ => out_open = false,
            },
            line = stderr.next_line(), if err_open => match line {
                Ok(Some(l)) => {
                    if let Some(e) = l.strip_prefix("ERROR: ") {
                        last_error = Some(e.to_owned());
                    }
                }
                _ => err_open = false,
            },
            _ = cancel.wait_for(|c| *c), if !paused => {
                paused = true;
                interrupt(&child);
            }
        }
    }
    let status = child.wait().await.map_err(|e| e.to_string())?;
    if paused {
        return Ok(VideoOutcome::Paused);
    }
    match (status.success(), final_path) {
        (true, Some(p)) => Ok(VideoOutcome::Completed(p)),
        (true, None) => Err("yt-dlp finished without reporting a file".into()),
        (false, _) => Err(last_error.unwrap_or_else(|| format!("yt-dlp exited with {status}"))),
    }
}

/// Builds the final file from natively downloaded HLS tracks (video or muxed
/// stream first, optional separate audio second), without re-encoding video.
/// With `audio_kbps` only the audio is kept, as MP3.
pub async fn mux(tracks: &[PathBuf], out_dir: &Path, stem: &str, audio_kbps: Option<u32>) -> Result<PathBuf, String> {
    let first = tracks.first().ok_or("nothing was downloaded")?;
    tokio::fs::create_dir_all(out_dir).await.map_err(|e| e.to_string())?;
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-loglevel".into(), "error".into(), "-y".into()];
    for t in tracks {
        // TS streams may only state the picture size deep into the file ("dimensions not set").
        args.extend(["-probesize".into(), "100M".into(), "-analyzeduration".into(), "100M".into()]);
        args.extend(["-i".into(), t.to_string_lossy().into_owned()]);
    }
    let audio_map = if tracks.len() > 1 { "1:a" } else { "0:a" };
    let out = match audio_kbps {
        Some(kbps) => {
            let out = out_dir.join(format!("{stem}.mp3"));
            args.extend(["-map".into(), audio_map.into(), "-vn".into(), "-c:a".into(), "libmp3lame".into()]);
            if kbps == 0 {
                args.extend(["-q:a".into(), "0".into()]);
            } else {
                args.extend(["-b:a".into(), format!("{kbps}k")]);
            }
            out
        }
        None => {
            let out = out_dir.join(format!("{stem}.mp4"));
            args.extend(["-map".into(), "0:v?".into(), "-map".into(), format!("{audio_map}?")]);
            args.extend(["-c".into(), "copy".into(), "-movflags".into(), "+faststart".into()]);
            out
        }
    };
    args.push(out.to_string_lossy().into_owned());

    let result = Command::new("ffmpeg").args(&args).stdin(Stdio::null()).kill_on_drop(true).output().await;
    match result {
        Ok(o) if o.status.success() => Ok(out),
        // Codecs MP4 cannot hold: keep the joined stream as it came.
        _ if audio_kbps.is_none() && tracks.len() == 1 => {
            let ext = first.extension().and_then(|e| e.to_str()).unwrap_or("ts");
            let keep = out_dir.join(format!("{stem}.{ext}"));
            tokio::fs::rename(first, &keep).await.map_err(|e| e.to_string())?;
            Ok(keep)
        }
        Ok(o) => Err(format!("ffmpeg failed: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => Err(format!("cannot run ffmpeg: {e}")),
    }
}

/// Whether yt-dlp has a site-specific extractor for `url` (its generic fallback
/// does not count). Uses the zipapp's own matcher, so no network request is made.
/// `None` when that cannot be determined (non-zipapp build, no python).
pub async fn ytdlp_supports(url: &str) -> Option<bool> {
    const CHECK: &str = "import sys\n\
        sys.path.insert(0, sys.argv[1])\n\
        from yt_dlp.extractor import gen_extractor_classes\n\
        print(any(ie.suitable(sys.argv[2]) for ie in gen_extractor_classes() if ie.IE_NAME != 'generic'))";
    let bin = ytdlp_bin();
    // Only the zipapp build (starts with a #! line) can be imported as a module.
    let head = tokio::fs::read(&bin).await.ok()?;
    if !head.starts_with(b"#!") {
        return None;
    }
    let out = Command::new("python3")
        .args(["-I", "-c", CHECK])
        .arg(&bin)
        .arg(url)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    match String::from_utf8_lossy(&out.stdout).trim() {
        "True" => Some(true),
        "False" => Some(false),
        _ => None,
    }
}

/// HLS playlists are downloaded natively instead of through yt-dlp.
pub fn is_hls(url: &str) -> bool {
    url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase().ends_with(".m3u8")
}

/// SIGINT lets yt-dlp close its part files cleanly (SIGKILL could leave ffmpeg output half-written).
fn interrupt(child: &tokio::process::Child) {
    if let Some(pid) = child.id() {
        // SAFETY: plain syscall on a pid we own; worst case it targets an already-exited child.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGINT);
        }
    }
}

pub async fn formats(req: &Request) -> Result<MediaInfo, String> {
    let out = Command::new(ytdlp_bin())
        .args(["-J", "--no-playlist", "--no-warnings"])
        .args(js_runtime_args())
        .args(context_args(req))
        .arg("--")
        .arg(&req.url)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("cannot run yt-dlp: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err.lines().rev().find(|l| l.starts_with("ERROR: ")).unwrap_or(err.trim());
        return Err(line.trim_start_matches("ERROR: ").to_owned());
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("bad yt-dlp JSON: {e}"))?;
    Ok(media_info(&v))
}

pub fn media_info(v: &Value) -> MediaInfo {
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let codec = |v: &Value, k: &str| s(v, k).filter(|c| c != "none");
    let formats = v
        .get("formats")
        .and_then(Value::as_array)
        .map(|fs| {
            fs.iter()
                .filter(|f| s(f, "format_id").is_some())
                // Storyboards and other image formats are not downloadable media.
                .filter(|f| codec(f, "vcodec").is_some() || codec(f, "acodec").is_some() || s(f, "protocol").is_some_and(|p| p.starts_with("m3u8")))
                .map(|f| MediaFormat {
                    id: s(f, "format_id").unwrap_or_default(),
                    ext: s(f, "ext").unwrap_or_default(),
                    resolution: s(f, "resolution"),
                    fps: f.get("fps").and_then(Value::as_f64),
                    vcodec: codec(f, "vcodec"),
                    acodec: codec(f, "acodec"),
                    filesize: f
                        .get("filesize")
                        .and_then(Value::as_f64)
                        .or_else(|| f.get("filesize_approx").and_then(Value::as_f64))
                        .map(|n| n as u64),
                    tbr: f.get("tbr").and_then(Value::as_f64),
                    note: s(f, "format_note"),
                })
                .collect()
        })
        .unwrap_or_default();
    MediaInfo {
        title: s(v, "title").unwrap_or_default(),
        duration: v.get("duration").and_then(Value::as_f64),
        thumbnail: s(v, "thumbnail"),
        formats,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_carry_browser_context() {
        let req = Request {
            url: "https://example.org/v".into(),
            cookies: Some("a=1; b=2".into()),
            referer: Some("https://example.org/".into()),
            ..Default::default()
        };
        let job = VideoJob {
            request: &req,
            dir: Path::new("/tmp/x"),
            temp_dir: Path::new("/tmp/t"),
            title: Some("100% real"),
            format: Some("22"),
            audio_kbps: None,
            rate_limit: Some(1000),
        };
        let a = download_args(&job);
        let has = |k: &str, v: &str| a.windows(2).any(|w| w[0] == k && w[1] == v);
        assert!(has("-f", "22"));
        assert!(has("-P", "home:/tmp/x"));
        assert!(has("-P", "temp:/tmp/t"));
        assert!(has("-o", "100%% real.%(ext)s"));
        assert!(has("--limit-rate", "1000"));
        assert!(has("--add-header", "Cookie:a=1; b=2"));
        assert!(has("--referer", "https://example.org/"));
        // URL after `--`, so a URL starting with '-' can never become an option.
        assert_eq!(&a[a.len() - 2..], ["--", "https://example.org/v"]);

        let mp3 = VideoJob { format: None, audio_kbps: Some(192), ..job };
        let a = download_args(&mp3);
        let has = |k: &str, v: &str| a.windows(2).any(|w| w[0] == k && w[1] == v);
        assert!(has("-f", "ba/b[height<=480]/w") && has("--audio-format", "mp3") && has("--audio-quality", "192K"));
    }

    #[test]
    fn stems_are_clean() {
        assert_eq!(clean_stem(" a/b "), "a_b");
        assert_eq!(clean_stem("..."), "video");
        assert_eq!(clean_stem(&"x".repeat(400)).len(), 180);
        assert!(clean_stem(&"🎬".repeat(100)).len() <= 180);
    }

    #[test]
    fn progress_accumulates_across_merged_files() {
        let mut p = ProgressParser::default();
        let line = |f: &str, d: u64, t: u64| {
            format!("{PROGRESS}{{\"filename\":\"{f}\",\"downloaded_bytes\":{d},\"total_bytes\":{t},\"speed\":5.0}}")
        };
        assert_eq!(p.feed(&line("v.mp4", 50, 100)).unwrap().downloaded, 50);
        assert_eq!(p.feed(&line("v.mp4", 100, 100)).unwrap().downloaded, 100);
        let a = p.feed(&line("a.m4a", 10, 40)).unwrap();
        assert_eq!((a.downloaded, a.total), (110, Some(140)));
        assert!(p.feed("[download] something else").is_none());
    }

    #[test]
    fn media_info_skips_storyboards() {
        let v: Value = serde_json::json!({
            "title": "T", "duration": 10.0,
            "formats": [
                {"format_id": "sb0", "ext": "mhtml", "vcodec": "none", "acodec": "none", "protocol": "mhtml"},
                {"format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a.40.2", "filesize": 1000},
                {"format_id": "137", "ext": "mp4", "vcodec": "avc1", "acodec": "none", "resolution": "1920x1080"}
            ]
        });
        let m = media_info(&v);
        assert_eq!(m.formats.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["140", "137"]);
        assert_eq!(m.formats[0].vcodec, None);
        assert_eq!(m.formats[0].filesize, Some(1000));
    }
}
