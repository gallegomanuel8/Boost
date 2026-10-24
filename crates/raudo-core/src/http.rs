use percent_encoding::percent_decode_str;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_DISPOSITION, COOKIE, RANGE, REFERER, USER_AGENT};

use crate::model::Request;

const DEFAULT_UA: &str = concat!("raudo/", env!("CARGO_PKG_VERSION"));

pub fn build_headers(req: &Request) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (k, v) in &req.headers {
        if let (Ok(k), Ok(v)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v)) {
            map.insert(k, v);
        }
    }
    let mut set = |name, value: Option<&str>| {
        if let Some(v) = value.and_then(|v| HeaderValue::from_str(v).ok()) {
            map.insert(name, v);
        }
    };
    set(COOKIE, req.cookies.as_deref());
    set(REFERER, req.referer.as_deref());
    set(USER_AGENT, Some(req.user_agent.as_deref().unwrap_or(DEFAULT_UA)));
    // Ranges are byte offsets of the stored entity: compression would break them.
    map.insert(reqwest::header::ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    map
}

/// `Range` header for `[start, end)`; `end == None` means "to the end".
pub fn range_value(start: u64, end: Option<u64>) -> HeaderValue {
    let v = match end {
        Some(end) => format!("bytes={}-{}", start, end - 1),
        None => format!("bytes={start}-"),
    };
    HeaderValue::from_str(&v).expect("ascii range")
}

pub fn insert_range(map: &mut HeaderMap, start: u64, end: Option<u64>) {
    map.insert(RANGE, range_value(start, end));
}

/// Parses `bytes <first>-<last>/<total|*>`.
pub fn parse_content_range(v: &str) -> Option<(u64, u64, Option<u64>)> {
    let rest = v.trim().strip_prefix("bytes")?.trim_start();
    let (range, total) = rest.split_once('/')?;
    let (first, last) = range.split_once('-')?;
    let total = match total.trim() {
        "*" => None,
        t => Some(t.parse().ok()?),
    };
    Some((first.trim().parse().ok()?, last.trim().parse().ok()?, total))
}

/// File name from `Content-Disposition` (RFC 6266, `filename*` preferred).
pub fn filename_from_disposition(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(CONTENT_DISPOSITION)?.to_str().ok()?;
    let mut plain = None;
    for part in v.split(';').map(str::trim) {
        let Some((key, val)) = part.split_once('=') else { continue };
        let key = key.trim().to_ascii_lowercase();
        let val = val.trim().trim_matches('"');
        if key == "filename*" {
            // charset'lang'percent-encoded
            let encoded = val.splitn(3, '\'').nth(2).unwrap_or(val);
            return sanitize(&percent_decode_str(encoded).decode_utf8_lossy());
        }
        if key == "filename" {
            plain = sanitize(val);
        }
    }
    plain
}

pub fn filename_from_url(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    let last = url.path_segments()?.filter(|s| !s.is_empty()).last()?;
    sanitize(&percent_decode_str(last).decode_utf8_lossy())
}

/// True for names that tell the user nothing: CDN ids (`jdq9qqo9ir1gjg.mp4`,
/// `a3i6d2sua8~yCeMWVATGL`) or generic player names (`index.mp4`, `master.m3u8`).
pub fn looks_random(name: &str) -> bool {
    let stem = match name.rsplit_once('.') {
        Some((s, ext)) if !s.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()) => s,
        _ => name,
    };
    const GENERIC: [&str; 10] =
        ["index", "master", "video", "videoplayback", "playlist", "file", "download", "stream", "media", "manifest"];
    let lower = stem.to_ascii_lowercase();
    if GENERIC.contains(&lower.as_str()) || lower.starts_with("index_") {
        return true;
    }
    if stem.contains(' ') {
        return false;
    }
    if stem.contains('~') {
        return true;
    }
    let tokens = || stem.split(|c: char| !c.is_ascii_alphanumeric());
    // A real word: 4+ letters and no digits.
    let has_word = tokens().any(|t| t.len() >= 4 && t.chars().all(|c| c.is_ascii_alphabetic()));
    // Ids interleave letters and digits; dates and counters (IMG_20240101) do not.
    let has_id = tokens().any(|t| {
        t.len() >= 6
            && t.chars().filter(char::is_ascii_digit).count() >= 2
            && t.chars().filter(char::is_ascii_alphabetic).count() >= 2
    });
    !has_word && has_id
}

/// File extension for a `Content-Type`, for servers that send names without one.
pub fn ext_from_mime(mime: &str) -> Option<&'static str> {
    let mime = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    Some(match mime.as_str() {
        "video/mp4" | "video/x-m4v" => "mp4",
        "video/webm" => "webm",
        "video/x-matroska" => "mkv",
        "video/quicktime" => "mov",
        "video/x-msvideo" | "video/avi" => "avi",
        "video/x-flv" => "flv",
        "video/mp2t" => "ts",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/x-m4a" => "m4a",
        "audio/webm" => "weba",
        "audio/ogg" => "ogg",
        "audio/flac" => "flac",
        "audio/wav" | "audio/x-wav" => "wav",
        "application/zip" => "zip",
        "application/pdf" => "pdf",
        "application/x-7z-compressed" => "7z",
        "application/vnd.rar" | "application/x-rar-compressed" => "rar",
        "application/gzip" | "application/x-gzip" => "gz",
        "application/x-iso9660-image" => "iso",
        _ => return None,
    })
}

pub fn has_extension(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(s, e)| !s.is_empty() && (1..=5).contains(&e.len()) && e.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Strips path separators and control characters; rejects names that would escape the folder.
pub fn sanitize(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range() {
        assert_eq!(parse_content_range("bytes 0-99/1000"), Some((0, 99, Some(1000))));
        assert_eq!(parse_content_range("bytes 5-9/*"), Some((5, 9, None)));
        assert_eq!(parse_content_range("items 0-1/2"), None);
    }

    #[test]
    fn disposition() {
        let mut h = HeaderMap::new();
        h.insert(CONTENT_DISPOSITION, HeaderValue::from_static("attachment; filename=\"a b.zip\""));
        assert_eq!(filename_from_disposition(&h).as_deref(), Some("a b.zip"));
        h.insert(
            CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment; filename=x.zip; filename*=UTF-8''%C3%B1and%C3%BA.zip"),
        );
        assert_eq!(filename_from_disposition(&h).as_deref(), Some("ñandú.zip"));
    }

    #[test]
    fn random_names() {
        for n in ["jdq9qqo9ir1gjg.mp4", "a3i6d2sua8~yCeMWVATGL", "index.mp4", "master.m3u8", "index_1912x1072.mp4", "videoplayback"] {
            assert!(looks_random(n), "{n} should look random");
        }
        for n in [
            "Jack_and_Jill_Exotic-Beauty-Final.mp4",
            "linux-6.19.tar.xz",
            "My Holiday 2024.mp4",
            "report.pdf",
            "IMG_20240101_123456.jpg",
            "ubuntu-24.04-desktop-amd64.iso",
        ] {
            assert!(!looks_random(n), "{n} should look meaningful");
        }
    }

    #[test]
    fn extensions() {
        assert_eq!(ext_from_mime("video/mp4; codecs=avc1"), Some("mp4"));
        assert_eq!(ext_from_mime("text/html"), None);
        assert!(has_extension("a.mp4"));
        assert!(!has_extension("a3i6d2sua8~yCeMWVATGL"));
        assert!(!has_extension(".bashrc"));
    }

    #[test]
    fn names_are_sanitized() {
        assert_eq!(sanitize("../../etc/passwd").as_deref(), Some("_.._etc_passwd"));
        assert_eq!(filename_from_url("https://x.org/dir/file%20name.iso?x=1").as_deref(), Some("file name.iso"));
        assert_eq!(sanitize("..."), None);
    }
}
