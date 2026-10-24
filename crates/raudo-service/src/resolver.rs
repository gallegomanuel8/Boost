//! Fallback for pages yt-dlp does not support: many embedded players
//! (Lulustream, Filemoon, StreamWish, ...) hide the stream URL inside
//! Dean Edwards "packed" JavaScript: `eval(function(p,a,c,k,e,d){...}('…',62,90,'…'.split('|')))`.
//!
//! Resolving from the daemon also fixes stream tokens bound to an IP: the
//! browser may sit behind a VPN while downloads leave from the real address.

use std::sync::LazyLock;

use raudo_proto::Request;
use regex::Regex;
use reqwest::header::{COOKIE, REFERER, USER_AGENT};

static PACKED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)\}\('(.*?)',\s*(\d+),\s*(\d+),\s*'(.*?)'\.split\('\|'\)").expect("valid regex")
});
static IFRAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)<iframe[^>]+src=["']([^"']+)["']"#).expect("valid regex"));
static MEDIA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"https?://[^"'\s<>\\]+?\.(m3u8|mpd|mp4)(\?[^"'\s<>\\]*)?"#).expect("valid regex")
});
static TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").expect("valid regex"));
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\w+\b").expect("valid regex"));

const MAX_FRAMES: usize = 4;

/// Stream tokens are often bound to the user agent that loaded the player, so the
/// page fetch and the later download must present the same one.
pub const BROWSER_UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0";

#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub url: String,
    /// Origin of the document that referenced the stream, as a browser sends it
    /// on cross-origin requests; players' CDNs check it.
    pub referer: String,
    pub title: Option<String>,
}

fn origin_referer(url: &str) -> String {
    url::Url::parse(url)
        .map(|u| format!("{}/", u.origin().ascii_serialization()))
        .unwrap_or_else(|_| url.to_owned())
}

/// Base-`radix` digits as the packer writes them (0-9, a-z, A-Z).
fn encode(mut n: usize, radix: usize) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let radix = radix.clamp(2, DIGITS.len());
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[n % radix]);
        n /= radix;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).expect("ascii digits")
}

/// Every packed script in `html`, unpacked.
pub fn unpack_all(html: &str) -> Vec<String> {
    PACKED
        .captures_iter(html)
        .filter_map(|c| {
            let payload = c.get(1)?.as_str().replace("\\'", "'");
            let radix: usize = c.get(2)?.as_str().parse().ok()?;
            let count: usize = c.get(3)?.as_str().parse().ok()?;
            let words: Vec<&str> = c.get(4)?.as_str().split('|').collect();
            let table: std::collections::HashMap<String, &str> = (0..count.min(words.len()))
                .filter(|&i| !words[i].is_empty())
                .map(|i| (encode(i, radix), words[i]))
                .collect();
            Some(
                WORD.replace_all(&payload, |m: &regex::Captures| {
                    let w = &m[0];
                    table.get(w).map_or_else(|| w.to_owned(), |s| (*s).to_owned())
                })
                .into_owned(),
            )
        })
        .collect()
}

/// Stream URLs in a document, best first: HLS/DASH manifests before plain files.
pub fn find_media_all(text: &str) -> Vec<String> {
    // JSON-escaped URLs (`https:\/\/cdn\/v.mp4`) are common in player configs.
    let text = text.replace("\\/", "/");
    let mut found: Vec<(u8, String)> = MEDIA
        .captures_iter(&text)
        .map(|c| {
            let rank = match &c[1] {
                "m3u8" => 0,
                "mpd" => 1,
                _ => 2,
            };
            (rank, c[0].to_owned())
        })
        .collect();
    found.sort_by_key(|(r, _)| *r);
    let mut seen = std::collections::HashSet::new();
    found.into_iter().map(|(_, u)| u).filter(|u| seen.insert(u.clone())).collect()
}

#[cfg(test)]
pub fn find_media(text: &str) -> Option<String> {
    find_media_all(text).into_iter().next()
}

/// The URL really serves a stream: a playlist, or media bytes rather than an HTML page.
async fn is_stream(client: &reqwest::Client, url: &str, req: &Request, referer: &str) -> bool {
    let mut r = client.get(url).header(REFERER, referer).header(reqwest::header::RANGE, "bytes=0-2047");
    if let Some(ua) = &req.user_agent {
        r = r.header(USER_AGENT, ua);
    }
    let Ok(resp) = r.send().await else { return false };
    if !resp.status().is_success() {
        return false;
    }
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ctype.starts_with("text/html") {
        return false;
    }
    let Ok(head) = resp.bytes().await else { return false };
    head.starts_with(b"#EXTM3U")
        || ctype.contains("mpegurl")
        || ctype.contains("dash+xml")
        || ctype.starts_with("video/")
        || ctype.starts_with("audio/")
        || raudo_core::http::sniff_extension(&head).is_some()
}

fn page_title(html: &str) -> Option<String> {
    let t = TITLE.captures(html)?[1].trim().to_owned();
    (!t.is_empty()).then_some(t)
}

async fn fetch(client: &reqwest::Client, url: &str, req: &Request, referer: Option<&str>) -> Option<String> {
    let mut r = client.get(url);
    if let Some(ua) = &req.user_agent {
        r = r.header(USER_AGENT, ua);
    }
    if let Some(c) = &req.cookies {
        r = r.header(COOKIE, c);
    }
    if let Some(rf) = referer {
        r = r.header(REFERER, rf);
    }
    let resp = r.send().await.ok()?.error_for_status().ok()?;
    resp.text().await.ok()
}

/// Looks for the stream behind `req.url`: in the page itself and in its iframes.
pub async fn resolve(client: &reqwest::Client, req: &Request) -> Option<Resolved> {
    let page = fetch(client, &req.url, req, req.referer.as_deref()).await?;
    let title = page_title(&page);
    let base = url::Url::parse(&req.url).ok()?;

    let mut docs = vec![(req.url.clone(), page.clone())];
    for src in IFRAME.captures_iter(&page).take(MAX_FRAMES) {
        let Ok(abs) = base.join(&src[1]) else { continue };
        if !matches!(abs.scheme(), "http" | "https") {
            continue;
        }
        if let Some(html) = fetch(client, abs.as_str(), req, Some(&req.url)).await {
            docs.push((abs.into(), html));
        }
    }
    let doc_urls: Vec<String> = docs.iter().map(|(u, _)| u.split('#').next().unwrap_or(u).to_owned()).collect();
    let mut tried = 0;
    for (url, html) in &docs {
        let referer = origin_referer(url);
        let unpacked = unpack_all(html);
        let candidates = unpacked.iter().flat_map(|u| find_media_all(u)).chain(find_media_all(html));
        for media in candidates {
            // Pages whose own address ends in .mp4 (streamtape.com/v/…/name.mp4) are not streams.
            if doc_urls.iter().any(|d| d == &media) {
                continue;
            }
            tried += 1;
            if tried > 6 {
                return None;
            }
            if is_stream(client, &media, req, &referer).await {
                return Some(Resolved { url: media, referer, title: title.clone() });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpacks_packer_output() {
        // `var x=('https://cdn.example/v/master.m3u8');` packed with radix 10.
        let html = r#"<script>eval(function(p,a,c,k,e,d){while(c--)if(k[c])p=p.replace(new RegExp('\\b'+c.toString(a)+'\\b','g'),k[c]);return p}('0 1=(\'2://3.4/5/6.7\');',10,8,'var|x|https|cdn|example|v|master|m3u8'.split('|'),0,{}))</script>"#;
        let out = unpack_all(html);
        assert_eq!(out, ["var x=('https://cdn.example/v/master.m3u8');"]);
        assert_eq!(find_media(&out[0]).as_deref(), Some("https://cdn.example/v/master.m3u8"));
    }

    #[test]
    fn radix_digits() {
        assert_eq!(encode(0, 62), "0");
        assert_eq!(encode(10, 36), "a");
        assert_eq!(encode(61, 62), "Z");
        assert_eq!(encode(62, 62), "10");
    }

    #[test]
    fn prefers_manifests() {
        let t = r#"{file:"https://x.org/a.mp4"},{file:"https://x.org/hls/master.m3u8?t=1&s=2"}"#;
        assert_eq!(find_media(t).as_deref(), Some("https://x.org/hls/master.m3u8?t=1&s=2"));
        assert_eq!(find_media(r#"src":"https:\/\/x.org\/v.mp4""#).as_deref(), Some("https://x.org/v.mp4"));
    }
}
