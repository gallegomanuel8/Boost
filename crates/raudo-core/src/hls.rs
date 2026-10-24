//! Native HLS downloader, the way IDM does it (CM3UParser/CTSReader): read the
//! playlist, fetch every segment as a player would, then join them.
//!
//! Requests are plain HTTP with the browser's context (user agent, referer,
//! cookies), so streams that refuse other tools still download. Output is one
//! file per track (TS, or fragmented MP4 when the playlist has EXT-X-MAP);
//! remuxing into MP4/MP3 is the caller's job (ffmpeg).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use aes::Aes128;
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use futures_util::{stream, StreamExt};
use reqwest::header::HeaderMap;
use reqwest::{Client, StatusCode};
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;
use url::Url;

use crate::http;
use crate::model::Request;

// ------------------------------------------------------------------ playlist

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub uri: String,
    pub bandwidth: u64,
    pub height: Option<u32>,
    /// `AUDIO` group id when audio comes as a separate rendition.
    pub audio: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioRendition {
    pub group: String,
    pub uri: String,
    pub default: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    None,
    Aes128 { uri: String, iv: Option<[u8; 16]> },
    /// SAMPLE-AES, DRM...: not supported.
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SegmentInfo {
    pub uri: String,
    pub duration: f64,
    pub seq: u64,
    pub key: Key,
    /// (length, offset)
    pub range: Option<(u64, u64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaPlaylist {
    pub segments: Vec<SegmentInfo>,
    /// EXT-X-MAP: initialization section of fragmented MP4 streams.
    pub init: Option<(String, Option<(u64, u64)>)>,
    pub endlist: bool,
}

impl MediaPlaylist {
    pub fn duration(&self) -> f64 {
        self.segments.iter().map(|s| s.duration).sum()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Playlist {
    Master { variants: Vec<Variant>, audio: Vec<AudioRendition> },
    Media(MediaPlaylist),
}

/// `KEY=value,KEY="quoted, value"` → map.
fn attributes(s: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        let Some(eq) = rest.find('=') else { break };
        let key = rest[..eq].trim().to_ascii_uppercase();
        rest = &rest[eq + 1..];
        let value;
        if let Some(stripped) = rest.strip_prefix('"') {
            let end = stripped.find('"').unwrap_or(stripped.len());
            value = stripped[..end].to_owned();
            rest = stripped.get(end + 1..).unwrap_or("");
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            value = rest[..end].trim().to_owned();
            rest = &rest[end..];
        }
        rest = rest.trim_start().strip_prefix(',').unwrap_or(rest).trim_start();
        out.insert(key, value);
    }
    out
}

/// `len[@offset]`; without offset the range follows the previous one.
fn byterange(s: &str, next_offset: u64) -> Option<(u64, u64)> {
    let (len, off) = match s.split_once('@') {
        Some((l, o)) => (l.trim().parse().ok()?, o.trim().parse().ok()?),
        None => (s.trim().parse().ok()?, next_offset),
    };
    Some((len, off))
}

fn parse_iv(s: &str) -> Option<[u8; 16]> {
    let hex = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    let hex = format!("{hex:0>32}");
    let mut iv = [0u8; 16];
    for (i, b) in iv.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(iv)
}

pub fn parse(text: &str) -> Result<Playlist, String> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    if lines.next().map(|l| l.trim_start_matches('\u{feff}')) != Some("#EXTM3U") {
        return Err("not an HLS playlist".into());
    }
    let mut variants = Vec::new();
    let mut audio = Vec::new();
    let mut pending_variant: Option<Variant> = None;

    let mut segments = Vec::new();
    let mut init = None;
    let mut endlist = false;
    let mut seq: u64 = 0;
    let mut key = Key::None;
    let mut duration = None;
    let mut range = None;
    let mut next_offset = 0u64;

    for line in lines {
        if let Some(a) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            let a = attributes(a);
            pending_variant = Some(Variant {
                uri: String::new(),
                bandwidth: a.get("BANDWIDTH").and_then(|b| b.parse().ok()).unwrap_or(0),
                height: a.get("RESOLUTION").and_then(|r| r.split_once('x')).and_then(|(_, h)| h.parse().ok()),
                audio: a.get("AUDIO").cloned(),
            });
        } else if let Some(a) = line.strip_prefix("#EXT-X-MEDIA:") {
            let a = attributes(a);
            if a.get("TYPE").map(String::as_str) == Some("AUDIO") {
                if let (Some(group), Some(uri)) = (a.get("GROUP-ID"), a.get("URI")) {
                    audio.push(AudioRendition {
                        group: group.clone(),
                        uri: uri.clone(),
                        default: a.get("DEFAULT").map(String::as_str) == Some("YES"),
                    });
                }
            }
        } else if let Some(v) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            seq = v.trim().parse().unwrap_or(0);
        } else if let Some(a) = line.strip_prefix("#EXT-X-KEY:") {
            let a = attributes(a);
            key = match a.get("METHOD").map(String::as_str) {
                Some("NONE") | None => Key::None,
                Some("AES-128") => Key::Aes128 {
                    uri: a.get("URI").cloned().unwrap_or_default(),
                    iv: a.get("IV").and_then(|iv| parse_iv(iv)),
                },
                Some(other) => Key::Unsupported(other.to_owned()),
            };
        } else if let Some(a) = line.strip_prefix("#EXT-X-MAP:") {
            let a = attributes(a);
            if let Some(uri) = a.get("URI") {
                init = Some((uri.clone(), a.get("BYTERANGE").and_then(|r| byterange(r, 0))));
            }
        } else if let Some(v) = line.strip_prefix("#EXTINF:") {
            duration = Some(v.split(',').next().unwrap_or("0").trim().parse().unwrap_or(0.0));
        } else if let Some(v) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            range = byterange(v, next_offset);
        } else if line == "#EXT-X-ENDLIST" {
            endlist = true;
        } else if line.starts_with('#') {
            // Other tags do not change what we download.
        } else if let Some(mut v) = pending_variant.take() {
            v.uri = line.to_owned();
            variants.push(v);
        } else if let Some(d) = duration.take() {
            if let Some((len, off)) = range {
                next_offset = off + len;
            }
            segments.push(SegmentInfo { uri: line.to_owned(), duration: d, seq, key: key.clone(), range: range.take() });
            seq += 1;
        }
    }

    if !variants.is_empty() {
        Ok(Playlist::Master { variants, audio })
    } else {
        Ok(Playlist::Media(MediaPlaylist { segments, init, endlist }))
    }
}

/// Highest bandwidth not above `max_height`; the lowest one if none fits.
pub fn pick_variant(variants: &[Variant], max_height: Option<u32>) -> Option<&Variant> {
    let fits = |v: &&Variant| match (max_height, v.height) {
        (Some(max), Some(h)) => h <= max,
        _ => true,
    };
    variants
        .iter()
        .filter(fits)
        .max_by_key(|v| v.bandwidth)
        .or_else(|| variants.iter().min_by_key(|v| v.bandwidth))
}

// ----------------------------------------------------------------- inspect

/// What a playlist offers, for listing it the way IDM does ("TS, 1 h, 720p").
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct StreamInfo {
    pub master: bool,
    /// Absolute URLs of the variant playlists (empty for a media playlist).
    pub variants: Vec<String>,
    /// Height of the variant that would be downloaded.
    pub height: Option<u32>,
    pub duration: Option<f64>,
    /// "ts" or "mp4" (fragmented MP4, EXT-X-MAP).
    pub container: Option<&'static str>,
    pub encrypted: bool,
    pub live: bool,
}

pub async fn inspect(client: &Client, request: &Request) -> Result<StreamInfo, String> {
    let fetcher = Fetcher { client, headers: http::build_headers(request) };
    let url = Url::parse(&request.url).map_err(|e| e.to_string())?;
    let mut info = StreamInfo::default();
    let media = match parse(&fetcher.text(url.as_str()).await?)? {
        Playlist::Media(p) => p,
        Playlist::Master { variants, .. } => {
            info.master = true;
            info.variants = variants.iter().filter_map(|v| join(&url, &v.uri).ok()).map(String::from).collect();
            let best = pick_variant(&variants, None).ok_or("empty master playlist")?;
            info.height = best.height;
            match parse(&fetcher.text(join(&url, &best.uri)?.as_str()).await?)? {
                Playlist::Media(p) => p,
                Playlist::Master { .. } => return Err("nested master playlists are not supported".into()),
            }
        }
    };
    info.duration = Some(media.duration());
    info.container = Some(if media.init.is_some() { "mp4" } else { "ts" });
    info.encrypted = media.segments.iter().any(|s| s.key != Key::None);
    info.live = !media.endlist;
    Ok(info)
}

// ---------------------------------------------------------------- download

#[derive(Debug, Clone, Default)]
pub struct HlsProgress {
    pub downloaded: u64,
    /// Estimate from bandwidth × duration until the last segment arrives.
    pub total: Option<u64>,
    pub speed: f64,
    pub segments_done: usize,
    pub segments_total: usize,
}

pub struct HlsJob<'a> {
    pub request: &'a Request,
    /// Segment files and joined tracks go here; reruns skip finished segments.
    pub work_dir: &'a Path,
    /// Pick the best variant up to this height (e.g. 480 when only audio is wanted).
    pub max_height: Option<u32>,
    pub concurrency: usize,
}

pub enum HlsOutcome {
    /// Joined tracks: video (or muxed) first, then a separate audio track if any.
    Completed(Vec<PathBuf>),
    Paused,
}

struct Fetcher<'a> {
    client: &'a Client,
    headers: HeaderMap,
}

impl Fetcher<'_> {
    async fn get(&self, url: &str, range: Option<(u64, u64)>) -> Result<bytes::Bytes, (Option<StatusCode>, String)> {
        let mut h = self.headers.clone();
        if let Some((len, off)) = range {
            http::insert_range(&mut h, off, Some(off + len));
        }
        let resp = self.client.get(url).headers(h).send().await.map_err(|e| (None, e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err((Some(status), format!("{url}: server answered {status}")));
        }
        resp.bytes().await.map_err(|e| (None, e.to_string()))
    }

    /// Retries transient failures; 4xx (expired or forbidden link) fails at once.
    async fn get_retry(&self, url: &str, range: Option<(u64, u64)>) -> Result<bytes::Bytes, String> {
        let mut attempt = 0;
        loop {
            match self.get(url, range).await {
                Ok(b) => return Ok(b),
                Err((Some(s), msg)) if s.is_client_error() && s != StatusCode::TOO_MANY_REQUESTS => return Err(msg),
                Err((_, msg)) => {
                    attempt += 1;
                    if attempt > 6 {
                        return Err(msg);
                    }
                    tokio::time::sleep(Duration::from_secs(1 << attempt.min(4))).await;
                }
            }
        }
    }

    async fn text(&self, url: &str) -> Result<String, String> {
        let b = self.get_retry(url, None).await?;
        Ok(String::from_utf8_lossy(&b).into_owned())
    }
}

fn join(base: &Url, uri: &str) -> Result<Url, String> {
    base.join(uri).map_err(|e| format!("bad URL {uri}: {e}"))
}

fn decrypt(data: &[u8], key: &[u8; 16], iv: &[u8; 16]) -> Result<Vec<u8>, String> {
    cbc::Decryptor::<Aes128>::new(key.into(), iv.into())
        .decrypt_padded_vec_mut::<Pkcs7>(data)
        .map_err(|_| "AES-128 decryption failed (wrong key?)".to_owned())
}

/// Default IV is the media sequence number, big-endian.
fn sequence_iv(seq: u64) -> [u8; 16] {
    let mut iv = [0u8; 16];
    iv[8..].copy_from_slice(&seq.to_be_bytes());
    iv
}

pub async fn run(
    client: &Client,
    job: HlsJob<'_>,
    mut on_progress: impl FnMut(HlsProgress),
    cancel: watch::Receiver<bool>,
) -> Result<HlsOutcome, String> {
    let fetcher = Fetcher { client, headers: http::build_headers(job.request) };
    let master_url = Url::parse(&job.request.url).map_err(|e| e.to_string())?;
    tokio::fs::create_dir_all(job.work_dir).await.map_err(|e| e.to_string())?;

    // Master playlist → chosen variant (+ separate audio); media playlist → itself.
    let mut tracks: Vec<(Url, MediaPlaylist)> = Vec::new();
    let mut bandwidth = 0u64;
    match parse(&fetcher.text(master_url.as_str()).await?)? {
        Playlist::Media(p) => tracks.push((master_url.clone(), p)),
        Playlist::Master { variants, audio } => {
            let v = pick_variant(&variants, job.max_height).ok_or("empty master playlist")?;
            bandwidth = v.bandwidth;
            let vurl = join(&master_url, &v.uri)?;
            let Playlist::Media(vp) = parse(&fetcher.text(vurl.as_str()).await?)? else {
                return Err("nested master playlists are not supported".into());
            };
            tracks.push((vurl, vp));
            let rendition = v.audio.as_ref().and_then(|g| {
                let in_group: Vec<_> = audio.iter().filter(|a| &a.group == g).collect();
                in_group.iter().find(|a| a.default).or(in_group.first()).copied()
            });
            if let Some(a) = rendition {
                let aurl = join(&master_url, &a.uri)?;
                if let Playlist::Media(ap) = parse(&fetcher.text(aurl.as_str()).await?)? {
                    tracks.push((aurl, ap));
                }
            }
        }
    }
    for (_, p) in &tracks {
        if !p.endlist {
            return Err("live streams are not supported (no EXT-X-ENDLIST)".into());
        }
        if let Some(Key::Unsupported(m)) = p.segments.iter().map(|s| &s.key).find(|k| matches!(k, Key::Unsupported(_))) {
            return Err(format!("encrypted stream ({m}) is not supported"));
        }
    }

    let segments_total: usize = tracks.iter().map(|(_, p)| p.segments.len()).sum();
    let duration = tracks.first().map(|(_, p)| p.duration()).unwrap_or(0.0);
    let estimate = (bandwidth > 0).then(|| (bandwidth as f64 / 8.0 * duration) as u64);
    let mut progress = HlsProgress { total: estimate, segments_total, ..Default::default() };
    let mut keys: HashMap<String, [u8; 16]> = HashMap::new();
    let mut outputs = Vec::new();
    let started = Instant::now();
    let mut window = (Instant::now(), 0u64);

    for (t, (base, playlist)) in tracks.iter().enumerate() {
        let seg_dir = job.work_dir.join(format!("track{t}"));
        tokio::fs::create_dir_all(&seg_dir).await.map_err(|e| e.to_string())?;

        // Keys are tiny and shared by many segments: fetch each once, up front.
        for s in &playlist.segments {
            if let Key::Aes128 { uri, .. } = &s.key {
                let kurl = join(base, uri)?.to_string();
                if !keys.contains_key(&kurl) {
                    let k = fetcher.get_retry(&kurl, None).await?;
                    let k: [u8; 16] = k.as_ref().try_into().map_err(|_| format!("bad AES key size ({} bytes)", k.len()))?;
                    keys.insert(kurl, k);
                }
            }
        }

        // Collected first: a lazy `map` closure in the stream type trips Send checks under tokio::spawn.
        let jobs: Vec<_> = playlist.segments.iter().enumerate().map(|(i, s)| {
            let path = seg_dir.join(format!("{i:06}.seg"));
            let url = join(base, &s.uri);
            let keys = &keys;
            let fetcher = &fetcher;
            let mut cancel = cancel.clone();
            async move {
                if let Ok(meta) = tokio::fs::metadata(&path).await {
                    return Ok((meta.len(), true)); // finished in an earlier run
                }
                let url = url?;
                let data = tokio::select! {
                    d = fetcher.get_retry(url.as_str(), s.range) => d?,
                    _ = cancel.wait_for(|c| *c) => return Err("paused".to_owned()),
                };
                let data = match &s.key {
                    Key::Aes128 { uri, iv } => {
                        let kurl = join(base, uri)?.to_string();
                        let key = keys.get(&kurl).ok_or("missing key")?;
                        decrypt(&data, key, &iv.unwrap_or_else(|| sequence_iv(s.seq)))?
                    }
                    _ => data.to_vec(),
                };
                // Write under a temp name: a segment file exists only when complete.
                let tmp = path.with_extension("part");
                tokio::fs::write(&tmp, &data).await.map_err(|e| e.to_string())?;
                tokio::fs::rename(&tmp, &path).await.map_err(|e| e.to_string())?;
                Ok::<_, String>((data.len() as u64, false))
            }
        }).collect();
        let mut results = stream::iter(jobs).buffer_unordered(job.concurrency.max(1));
        while let Some(r) = results.next().await {
            match r {
                Ok((len, _cached)) => {
                    progress.downloaded += len;
                    progress.segments_done += 1;
                    window.1 += len;
                    let dt = window.0.elapsed().as_secs_f64();
                    if dt >= 0.5 {
                        progress.speed = window.1 as f64 / dt;
                        window = (Instant::now(), 0);
                    }
                    // Refine the size estimate from the average segment so far.
                    if progress.segments_done > 0 && progress.segments_done < segments_total {
                        let avg = progress.downloaded as f64 / progress.segments_done as f64;
                        progress.total = Some((avg * segments_total as f64) as u64);
                    }
                    on_progress(progress.clone());
                }
                Err(e) if e == "paused" => return Ok(HlsOutcome::Paused),
                Err(e) => return Err(e),
            }
            if *cancel.borrow() {
                return Ok(HlsOutcome::Paused);
            }
        }

        // Join in playlist order, init section first for fragmented MP4.
        let ext = if playlist.init.is_some() { "mp4" } else { "ts" };
        let out_path = job.work_dir.join(format!("track{t}.{ext}"));
        let mut out = tokio::fs::File::create(&out_path).await.map_err(|e| e.to_string())?;
        if let Some((uri, range)) = &playlist.init {
            let init = fetcher.get_retry(join(base, uri)?.as_str(), *range).await?;
            out.write_all(&init).await.map_err(|e| e.to_string())?;
        }
        for i in 0..playlist.segments.len() {
            let data = tokio::fs::read(seg_dir.join(format!("{i:06}.seg"))).await.map_err(|e| e.to_string())?;
            out.write_all(&data).await.map_err(|e| e.to_string())?;
        }
        out.flush().await.map_err(|e| e.to_string())?;
        drop(out);
        tokio::fs::remove_dir_all(&seg_dir).await.ok();
        outputs.push(out_path);
    }

    progress.total = Some(progress.downloaded);
    progress.speed = progress.downloaded as f64 / started.elapsed().as_secs_f64().max(0.001);
    on_progress(progress);
    Ok(HlsOutcome::Completed(outputs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_playlist() {
        let m = "#EXTM3U\n\
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"English, main\",DEFAULT=YES,URI=\"audio/en.m3u8\"\n\
            #EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360,CODECS=\"avc1.4d401e,mp4a.40.2\",AUDIO=\"aud\"\n\
            360/index.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=2993000,RESOLUTION=1280x720,AUDIO=\"aud\"\n\
            720/index.m3u8\n";
        let Playlist::Master { variants, audio } = parse(m).unwrap() else { panic!("not master") };
        assert_eq!(variants.len(), 2);
        assert_eq!(audio, [AudioRendition { group: "aud".into(), uri: "audio/en.m3u8".into(), default: true }]);
        assert_eq!(pick_variant(&variants, None).unwrap().height, Some(720));
        assert_eq!(pick_variant(&variants, Some(480)).unwrap().height, Some(360));
        assert_eq!(pick_variant(&variants, Some(100)).unwrap().height, Some(360));
    }

    #[test]
    fn media_playlist_with_key_map_and_ranges() {
        let m = "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:7\n\
            #EXT-X-MAP:URI=\"init.mp4\",BYTERANGE=\"720@0\"\n\
            #EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\",IV=0x000102030405060708090a0b0c0d0e0f\n\
            #EXTINF:6.006,\nseg0.m4s\n\
            #EXT-X-KEY:METHOD=NONE\n\
            #EXT-X-BYTERANGE:1000@2000\n#EXTINF:4.0,\nall.ts\n\
            #EXT-X-BYTERANGE:500\n#EXTINF:2.5,\nall.ts\n\
            #EXT-X-ENDLIST\n";
        let Playlist::Media(p) = parse(m).unwrap() else { panic!("not media") };
        assert!(p.endlist);
        assert_eq!(p.init, Some(("init.mp4".into(), Some((720, 0)))));
        assert_eq!(p.segments.len(), 3);
        assert_eq!(p.segments[0].seq, 7);
        assert!(matches!(&p.segments[0].key, Key::Aes128 { uri, iv: Some(iv) } if uri == "key.bin" && iv[15] == 0x0f));
        assert_eq!(p.segments[1].key, Key::None);
        assert_eq!(p.segments[1].range, Some((1000, 2000)));
        // No offset: continues after the previous range.
        assert_eq!(p.segments[2].range, Some((500, 3000)));
        assert!((p.duration() - 12.506).abs() < 1e-9);
    }

    #[test]
    fn quoted_attributes_keep_commas() {
        let a = attributes("NAME=\"a, b\",URI=\"x.m3u8\",DEFAULT=YES");
        assert_eq!(a["NAME"], "a, b");
        assert_eq!(a["URI"], "x.m3u8");
        assert_eq!(a["DEFAULT"], "YES");
    }

    #[test]
    fn aes_roundtrip() {
        use cbc::cipher::BlockEncryptMut;
        let key = [7u8; 16];
        let iv = sequence_iv(42);
        let plain = b"segment payload that spans more than one block".to_vec();
        let enc = cbc::Encryptor::<Aes128>::new(&key.into(), &iv.into()).encrypt_padded_vec_mut::<Pkcs7>(&plain);
        assert_eq!(decrypt(&enc, &key, &iv).unwrap(), plain);
        assert_eq!(sequence_iv(1)[15], 1);
    }
}
