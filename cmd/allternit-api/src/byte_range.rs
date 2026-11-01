//! HTTP byte ranges for media served from memory (RFC 9110 §14), so players
//! can read a video's index and seek without downloading the whole file.
//!
//! Only a single range is honoured. A list of ranges, or a unit other than
//! bytes, gets the whole file: a server may ignore those. A malformed or
//! unsatisfiable `bytes=` range gets 416.

use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

#[derive(Debug, PartialEq, Eq)]
enum Requested {
    Whole,
    /// Inclusive first and last byte.
    Part(usize, usize),
    Unsatisfiable,
}

fn parse(value: &str, len: usize) -> Requested {
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return Requested::Whole;
    };
    if spec.contains(',') {
        return Requested::Whole;
    }
    let Some((first, last)) = spec.trim().split_once('-') else {
        return Requested::Unsatisfiable;
    };
    if len == 0 {
        return Requested::Unsatisfiable;
    }
    let number = |text: &str| text.parse::<usize>().ok();
    match (first.is_empty(), last.is_empty()) {
        // bytes=-500: the last 500 bytes, or all of a shorter file.
        (true, false) => match number(last) {
            Some(suffix) if suffix > 0 => Requested::Part(len.saturating_sub(suffix), len - 1),
            _ => Requested::Unsatisfiable,
        },
        // bytes=500-: from there to the end.
        (false, true) => match number(first) {
            Some(start) if start < len => Requested::Part(start, len - 1),
            _ => Requested::Unsatisfiable,
        },
        // bytes=0-499: a last byte past the end means the end.
        (false, false) => match (number(first), number(last)) {
            (Some(start), Some(end)) if start <= end && start < len => Requested::Part(start, end.min(len - 1)),
            _ => Requested::Unsatisfiable,
        },
        (true, true) => Requested::Unsatisfiable,
    }
}

/// The file, or the part of it the request's `Range` header asks for.
pub fn ranged_response(headers: &HeaderMap, content_type: String, bytes: Vec<u8>) -> Response {
    let len = bytes.len();
    // A header that isn't text can't name a range.
    let requested = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .map_or(Requested::Whole, |value| parse(value, len));
    match requested {
        Requested::Whole => (
            [
                (header::CONTENT_TYPE, content_type),
                (header::ACCEPT_RANGES, "bytes".to_string()),
                (header::CONTENT_LENGTH, len.to_string()),
            ],
            bytes,
        )
            .into_response(),
        Requested::Part(start, end) => (
            StatusCode::PARTIAL_CONTENT,
            [
                (header::CONTENT_TYPE, content_type),
                (header::ACCEPT_RANGES, "bytes".to_string()),
                (header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}")),
                (header::CONTENT_LENGTH, (end - start + 1).to_string()),
            ],
            bytes[start..=end].to_vec(),
        )
            .into_response(),
        Requested::Unsatisfiable => (
            StatusCode::RANGE_NOT_SATISFIABLE,
            [
                (header::ACCEPT_RANGES, "bytes".to_string()),
                (header::CONTENT_RANGE, format!("bytes */{len}")),
            ],
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &[u8] = b"0123456789";

    async fn get(range: Option<&str>) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut headers = HeaderMap::new();
        if let Some(range) = range {
            headers.insert(header::RANGE, range.parse().unwrap());
        }
        let response = ranged_response(&headers, "video/mp4".to_string(), FILE.to_vec());
        let (parts, body) = response.into_parts();
        let body = axum::body::to_bytes(body, usize::MAX).await.unwrap().to_vec();
        (parts.status, parts.headers, body)
    }

    fn text<'a>(headers: &'a HeaderMap, name: header::HeaderName) -> &'a str {
        headers.get(name).unwrap().to_str().unwrap()
    }

    #[tokio::test]
    async fn no_range_sends_the_whole_file_and_offers_ranges() {
        let (status, headers, body) = get(None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(text(&headers, header::ACCEPT_RANGES), "bytes");
        assert_eq!(text(&headers, header::CONTENT_TYPE), "video/mp4");
        assert_eq!(body, FILE);
    }

    #[tokio::test]
    async fn normal_range_sends_that_part() {
        let (status, headers, body) = get(Some("bytes=2-5")).await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(text(&headers, header::CONTENT_RANGE), "bytes 2-5/10");
        assert_eq!(text(&headers, header::CONTENT_LENGTH), "4");
        assert_eq!(text(&headers, header::ACCEPT_RANGES), "bytes");
        assert_eq!(body, b"2345");
    }

    #[tokio::test]
    async fn last_byte_past_the_end_stops_at_the_end() {
        let (status, headers, body) = get(Some("bytes=8-99")).await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(text(&headers, header::CONTENT_RANGE), "bytes 8-9/10");
        assert_eq!(body, b"89");
    }

    #[tokio::test]
    async fn open_ended_range_runs_to_the_end() {
        let (status, headers, body) = get(Some("bytes=7-")).await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(text(&headers, header::CONTENT_RANGE), "bytes 7-9/10");
        assert_eq!(body, b"789");
    }

    #[tokio::test]
    async fn suffix_range_sends_the_tail() {
        let (status, headers, body) = get(Some("bytes=-3")).await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(text(&headers, header::CONTENT_RANGE), "bytes 7-9/10");
        assert_eq!(body, b"789");
        let (_, headers, body) = get(Some("bytes=-99")).await;
        assert_eq!(text(&headers, header::CONTENT_RANGE), "bytes 0-9/10");
        assert_eq!(body, FILE);
    }

    #[tokio::test]
    async fn bad_ranges_get_416_with_the_length() {
        for range in ["bytes=10-", "bytes=10-20", "bytes=5-2", "bytes=-0", "bytes=-", "bytes=abc", "bytes=a-b"] {
            let (status, headers, body) = get(Some(range)).await;
            assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE, "{range}");
            assert_eq!(text(&headers, header::CONTENT_RANGE), "bytes */10", "{range}");
            assert!(body.is_empty(), "{range}");
        }
    }

    #[tokio::test]
    async fn range_lists_and_other_units_get_the_whole_file() {
        for range in ["bytes=0-1,4-5", "items=0-1"] {
            let (status, _, body) = get(Some(range)).await;
            assert_eq!(status, StatusCode::OK, "{range}");
            assert_eq!(body, FILE, "{range}");
        }
    }
}
