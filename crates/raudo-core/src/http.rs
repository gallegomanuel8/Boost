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
    fn names_are_sanitized() {
        assert_eq!(sanitize("../../etc/passwd").as_deref(), Some("_.._etc_passwd"));
        assert_eq!(filename_from_url("https://x.org/dir/file%20name.iso?x=1").as_deref(), Some("file name.iso"));
        assert_eq!(sanitize("..."), None);
    }
}
