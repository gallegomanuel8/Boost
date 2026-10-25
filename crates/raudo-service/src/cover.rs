//! Album art and tags for MP3s: MusicBrainz finds the song's release, Cover Art
//! Archive its front cover. When nothing matches, the file keeps what yt-dlp
//! embedded: the video thumbnail, cropped square, and the page's tags.

use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::video;

const USER_AGENT: &str = concat!("Boost/", env!("CARGO_PKG_VERSION"), " ( https://github.com/gallegomanuel8/Boost )");
/// MusicBrainz allows one request per second from each client.
const MB_SPACING: Duration = Duration::from_millis(1100);
const TIMEOUT: Duration = Duration::from_secs(15);

/// What the download tells about the song (yt-dlp's fields, or just a title).
#[derive(Debug, Default, Clone)]
pub struct Hint {
    pub title: Option<String>,
    pub track: Option<String>,
    pub artist: Option<String>,
    pub channel: Option<String>,
}

impl Hint {
    /// From yt-dlp's `RAUDO-M` line; `title` stands in when there is none (HLS).
    pub fn new(meta: Option<&Value>, title: Option<&str>) -> Hint {
        let s = |k: &str| {
            meta.and_then(|m| m.get(k)).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned)
        };
        let first_artist = meta
            .and_then(|m| m.get("artists"))
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .map(str::to_owned);
        Hint {
            title: s("title").or_else(|| title.map(str::to_owned)),
            track: s("track"),
            artist: first_artist.or_else(|| s("artist")).or_else(|| s("creator")),
            channel: s("channel").or_else(|| s("uploader")),
        }
    }

    /// (artist, song) to look up: the site's music fields when it has them, else
    /// read from a title like "Song (Official Video) – Artist" and the channel.
    pub fn song(&self) -> Option<(String, String)> {
        if let (Some(a), Some(t)) = (&self.artist, &self.track) {
            return Some((a.clone(), t.clone()));
        }
        let title = clean_title(self.title.as_deref()?);
        let channel = self.channel.as_deref().map(clean_channel).unwrap_or_default();
        let parts = split_title(&title);
        match parts {
            Some((a, b)) => {
                let (ch, na, nb) = (norm(&channel), norm(&a), norm(&b));
                let is_channel = |n: &str| !ch.is_empty() && !n.is_empty() && (n == ch || ch.contains(n) || n.contains(&ch));
                // "Artist - Song" is the usual order; the channel tells when it is the other way.
                if is_channel(&nb) && !is_channel(&na) {
                    Some((b, a))
                } else {
                    Some((a, b))
                }
            }
            None if !channel.is_empty() && !title.is_empty() => Some((channel, title)),
            None => None,
        }
    }
}

/// Lowercase letters and digits only, for comparing names.
fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Drops what video titles add around the song: bracketed notes ("(Official
/// Video)", "[4K UPGRADE]", "【MV】") and featured artists.
fn clean_title(title: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for c in title.chars() {
        match c {
            '(' | '[' | '{' | '【' | '「' => depth += 1,
            ')' | ']' | '}' | '】' | '」' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let lower = out.to_lowercase();
    // " ft. X" up to the next separator: the artist credit carries it, not the song.
    for feat in [" feat. ", " feat ", " ft. ", " ft ", " featuring "] {
        // Lowercasing can change byte lengths: the index must still fit `out`.
        if let Some(i) = lower.find(feat).filter(|&i| lower.len() == out.len() && out.is_char_boundary(i)) {
            let rest = &out[i..];
            let end = SEPARATORS.iter().filter_map(|s| rest.find(s)).min().unwrap_or(rest.len());
            out = format!("{}{}", &out[..i], &rest[end..]);
            break;
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

const SEPARATORS: [&str; 5] = [" - ", " – ", " — ", " | ", " ~ "];

fn split_title(title: &str) -> Option<(String, String)> {
    let (i, sep) = SEPARATORS.iter().filter_map(|s| title.find(s).map(|i| (i, *s))).min_by_key(|(i, _)| *i)?;
    let (a, b) = (title[..i].trim(), title[i + sep.len()..].trim());
    (!a.is_empty() && !b.is_empty()).then(|| (a.to_owned(), b.to_owned()))
}

/// "LinkinParkVEVO", "Linkin Park - Topic", "Linkin Park Official" -> the artist.
fn clean_channel(channel: &str) -> String {
    let mut c = channel.trim().to_owned();
    for suffix in [" - Topic", "VEVO", " Official", " Music"] {
        let cut = c.len().saturating_sub(suffix.len());
        if c.is_char_boundary(cut) && c[cut..].eq_ignore_ascii_case(suffix) {
            c.truncate(cut);
        }
    }
    c.trim().to_owned()
}

/// A release that has the song, as MusicBrainz lists it.
#[derive(Debug, Clone, PartialEq)]
struct Candidate {
    /// 0 studio album, 1 single, 2 EP, 3 other, 4 live/compilation/soundtrack...
    rank: u8,
    date: String,
    title: String,
    artist: String,
    album: String,
    release: String,
    group: String,
}

/// Recordings of other performances or mixes: not the song's own release.
const OTHER_TAKES: [&str; 9] = ["live", "demo", "remix", "instrumental", "acoustic", "karaoke", "rehearsal", "session", "cover"];

/// Releases of `song` by `artist` in a recording search, best first.
fn candidates(result: &Value, artist: &str, song: &str) -> Vec<Candidate> {
    let (want_artist, want_song) = (norm(artist), norm(song));
    let mut found = Vec::new();
    for rec in result.get("recordings").and_then(Value::as_array).into_iter().flatten() {
        let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        if rec.get("score").and_then(Value::as_u64).unwrap_or(0) < 90 || norm(&s(rec, "title")) != want_song {
            continue;
        }
        let take = s(rec, "disambiguation").to_lowercase();
        if OTHER_TAKES.iter().any(|w| take.contains(w)) {
            continue;
        }
        let credit: String = rec
            .get("artist-credit")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|a| format!("{}{}", s(a, "name"), s(a, "joinphrase")))
            .collect();
        let have = norm(&credit);
        if have.is_empty() || !(have == want_artist || have.contains(&want_artist) || want_artist.contains(&have)) {
            continue;
        }
        for rel in rec.get("releases").and_then(Value::as_array).into_iter().flatten() {
            let status = s(rel, "status");
            if !status.is_empty() && status != "Official" {
                continue;
            }
            let group = rel.get("release-group").cloned().unwrap_or(Value::Null);
            let secondary = group.get("secondary-types").and_then(Value::as_array).is_some_and(|a| !a.is_empty());
            let rank = match (secondary, s(&group, "primary-type").as_str()) {
                (true, _) => 4,
                (_, "Album") => 0,
                (_, "Single") => 1,
                (_, "EP") => 2,
                _ => 3,
            };
            found.push(Candidate {
                rank,
                date: Some(s(rel, "date")).filter(|d| !d.is_empty()).unwrap_or_else(|| "9999".into()),
                title: s(rec, "title"),
                artist: credit.clone(),
                album: s(rel, "title"),
                release: s(rel, "id"),
                group: s(&group, "id"),
            });
        }
    }
    found.sort_by(|a, b| a.rank.cmp(&b.rank).then_with(|| a.date.cmp(&b.date)));
    found
}

/// Waits for MusicBrainz's one-request-per-second turn.
async fn mb_turn() {
    static LAST: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    let mut last = LAST.get_or_init(|| Mutex::new(None)).lock().await;
    if let Some(t) = *last {
        let wait = MB_SPACING.saturating_sub(t.elapsed());
        tokio::time::sleep(wait).await;
    }
    *last = Some(Instant::now());
}

/// A recording search; retried when the server is busy (it answers with an error body).
async fn search(client: &reqwest::Client, artist: &str, song: &str) -> Option<Value> {
    let phrase = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let query = format!("recording:{} AND artist:{}", phrase(song), phrase(artist));
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        mb_turn().await;
        let resp = client
            .get("https://musicbrainz.org/ws/2/recording")
            .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "50")])
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .timeout(TIMEOUT)
            .send()
            .await;
        match resp {
            Ok(r) => match r.bytes().await.map(|b| serde_json::from_slice::<Value>(&b)) {
                Ok(Ok(v)) if v.get("recordings").is_some() => return Some(v),
                Ok(Ok(v)) => warn!(error = %v, "MusicBrainz busy"),
                Ok(Err(e)) => warn!("MusicBrainz answer unreadable: {e}"),
                Err(e) => warn!("MusicBrainz answer cut: {e}"),
            },
            Err(e) => warn!("MusicBrainz unreachable: {e}"),
        }
    }
    None
}

/// Front cover, 500 px: the release group's chosen one, else the release's own.
async fn front_cover(client: &reqwest::Client, c: &Candidate) -> Option<Vec<u8>> {
    let urls = [
        (!c.group.is_empty()).then(|| format!("https://coverartarchive.org/release-group/{}/front-500", c.group)),
        (!c.release.is_empty()).then(|| format!("https://coverartarchive.org/release/{}/front-500", c.release)),
    ];
    for url in urls.into_iter().flatten() {
        let Ok(r) = client.get(&url).header(reqwest::header::USER_AGENT, USER_AGENT).timeout(TIMEOUT).send().await else {
            continue;
        };
        if !r.status().is_success() {
            continue;
        }
        if let Ok(bytes) = r.bytes().await {
            // JPEG or PNG: what ID3 pictures hold.
            if bytes.starts_with(&[0xff, 0xd8]) || bytes.starts_with(b"\x89PNG") {
                return Some(bytes.to_vec());
            }
        }
    }
    None
}

/// Album art and tags from MusicBrainz for a finished MP3. Best effort: any
/// failure leaves the file as yt-dlp made it.
pub async fn tag_mp3(client: &reqwest::Client, file: &Path, hint: &Hint) {
    let Some((artist, song)) = hint.song() else {
        return;
    };
    let Some(result) = search(client, &artist, &song).await else {
        return;
    };
    let found = candidates(&result, &artist, &song);
    let Some(best) = found.first() else {
        info!(%artist, %song, "no MusicBrainz match: keeping the video thumbnail");
        return;
    };
    // The best release may have no cover in the archive; the next albums may.
    let mut cover = None;
    let mut seen = Vec::new();
    for c in found.iter().filter(|c| c.rank <= 3) {
        if seen.contains(&c.group) || seen.len() >= 3 {
            continue;
        }
        seen.push(c.group.clone());
        if let Some(img) = front_cover(client, c).await {
            cover = Some(img);
            break;
        }
    }
    info!(%artist, %song, album = %best.album, cover = cover.is_some(), "MusicBrainz match");
    if let Err(e) = write_tags(file, best, cover.as_deref()).await {
        warn!("cannot tag {}: {e}", file.display());
    }
}

/// Rewrites the MP3 with the new tags and, when there is one, the album cover in
/// place of the thumbnail. Audio is copied, not re-encoded.
async fn write_tags(file: &Path, found: &Candidate, cover: Option<&[u8]>) -> Result<(), String> {
    let out = file.with_extension("tagged.mp3");
    let art = file.with_extension("cover.img");
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-loglevel".into(), "error".into(), "-y".into()];
    args.extend(["-i".into(), file.to_string_lossy().into_owned()]);
    if let Some(img) = cover {
        tokio::fs::write(&art, img).await.map_err(|e| e.to_string())?;
        args.extend(["-i".into(), art.to_string_lossy().into_owned()]);
        args.extend(["-map".into(), "0:a".into(), "-map".into(), "1:0".into()]);
        args.extend(["-metadata:s:v".into(), "title=Album cover".into(), "-metadata:s:v".into(), "comment=Cover (front)".into()]);
        args.extend(["-disposition:v:0".into(), "attached_pic".into()]);
    } else {
        args.extend(["-map".into(), "0".into()]);
    }
    args.extend(["-c".into(), "copy".into(), "-map_metadata".into(), "0".into(), "-id3v2_version".into(), "3".into()]);
    let year = found.date.get(..4).filter(|y| *y != "9999");
    for (k, v) in [("title", Some(found.title.as_str())), ("artist", Some(found.artist.as_str())), ("album", Some(found.album.as_str())), ("date", year)] {
        if let Some(v) = v.filter(|v| !v.is_empty()) {
            args.extend(["-metadata".into(), format!("{k}={v}")]);
        }
    }
    args.push(out.to_string_lossy().into_owned());

    let result = ffmpeg().args(&args).stdin(std::process::Stdio::null()).kill_on_drop(true).output().await;
    let _ = tokio::fs::remove_file(&art).await;
    match result {
        Ok(o) if o.status.success() => tokio::fs::rename(&out, file).await.map_err(|e| e.to_string()),
        Ok(o) => {
            let _ = tokio::fs::remove_file(&out).await;
            Err(format!("ffmpeg failed: {}", String::from_utf8_lossy(&o.stderr).trim()))
        }
        Err(e) => Err(format!("cannot run ffmpeg: {e}")),
    }
}

fn ffmpeg() -> Command {
    video::command(video::ffmpeg_bin())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hint(title: &str, channel: &str) -> Hint {
        Hint { title: Some(title.into()), channel: Some(channel.into()), ..Default::default() }
    }

    #[test]
    fn songs_from_video_titles() {
        let song = |t, c| hint(t, c).song().map(|(a, s)| format!("{a} / {s}"));
        assert_eq!(song("Numb (Official Music Video) [4K UPGRADE] – Linkin Park", "Linkin Park").as_deref(), Some("Linkin Park / Numb"));
        assert_eq!(song("Rosalía - DESPECHÁ (Official Video)", "RosaliaVEVO").as_deref(), Some("Rosalía / DESPECHÁ"));
        assert_eq!(song("Daft Punk - Get Lucky ft. Pharrell Williams [Official Audio]", "Daft Punk").as_deref(), Some("Daft Punk / Get Lucky"));
        assert_eq!(song("Bohemian Rhapsody (Remastered 2011)", "Queen - Topic").as_deref(), Some("Queen / Bohemian Rhapsody"));
        assert_eq!(song("", "").as_deref(), None);
        // The site's music fields win.
        let h = Hint { track: Some("Numb".into()), artist: Some("Linkin Park".into()), ..hint("whatever", "x") };
        assert_eq!(h.song(), Some(("Linkin Park".into(), "Numb".into())));
    }

    #[test]
    fn studio_album_first() {
        let rel = |status: &str, date: &str, title: &str, primary: &str, secondary: Vec<&str>| {
            json!({"status": status, "date": date, "title": title, "id": title,
                   "release-group": {"id": format!("rg-{title}"), "primary-type": primary, "secondary-types": secondary}})
        };
        let result = json!({"recordings": [
            {"score": 100, "title": "Numb", "artist-credit": [{"name": "Linkin Park"}],
             "releases": [rel("Official", "2003-11-18", "Live in Texas", "Album", vec!["Live"])]},
            {"score": 100, "title": "Numb", "disambiguation": "live, 2017", "artist-credit": [{"name": "Linkin Park"}],
             "releases": [rel("Official", "2001-01-01", "Too early but live", "Album", vec![])]},
            {"score": 100, "title": "Numb", "artist-credit": [{"name": "Linkin Park"}],
             "releases": [rel("Official", "2003-09-08", "Numb", "Single", vec![]),
                          rel("Bootleg", "2002-01-01", "Bootleg", "Album", vec![]),
                          rel("Official", "2003-03-25", "Meteora", "Album", vec![])]},
            {"score": 100, "title": "Numb", "artist-credit": [{"name": "U2"}],
             "releases": [rel("Official", "1993-07-06", "Zooropa", "Album", vec![])]},
        ]});
        let found = candidates(&result, "Linkin Park", "Numb");
        let albums: Vec<_> = found.iter().map(|c| c.album.as_str()).collect();
        assert_eq!(albums, ["Meteora", "Numb", "Live in Texas"]);
        assert_eq!(found[0].group, "rg-Meteora");
    }
}
