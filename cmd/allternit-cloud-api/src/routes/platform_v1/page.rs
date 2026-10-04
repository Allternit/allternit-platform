//! Cursor pagination: `?limit=&after=` in, `{data, has_more, next_cursor}` out.
//!
//! List queries order by `(created_at, id)` ascending. The cursor is the hex of
//! `"<created_at micros>|<id>"` of the last returned row; treat it as opaque.

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use super::PlatformError;

pub const DEFAULT_LIMIT: i64 = 20;
pub const MAX_LIMIT: i64 = 100;

#[derive(Debug, Default, Deserialize)]
pub struct PageParams {
    /// A number, or a numeric string: query strings reach a `#[serde(flatten)]`ed
    /// `PageParams` as strings, so both must parse.
    #[serde(default, deserialize_with = "number_or_string")]
    pub limit: Option<i64>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Page<T: Serialize> {
    pub data: Vec<T>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
}

impl PageParams {
    pub fn limit(&self) -> Result<i64, PlatformError> {
        match self.limit {
            None => Ok(DEFAULT_LIMIT),
            Some(n) if (1..=MAX_LIMIT).contains(&n) => Ok(n),
            Some(_) => Err(PlatformError::invalid_request(
                "invalid_limit",
                format!("limit must be between 1 and {MAX_LIMIT}"),
            )
            .with_param("limit")),
        }
    }

    /// Decoded `after` cursor as `(created_at, id)`; `None` for the first page.
    pub fn cursor(&self) -> Result<Option<(DateTime<Utc>, String)>, PlatformError> {
        match self.after.as_deref() {
            None | Some("") => Ok(None),
            Some(raw) => decode_cursor(raw).map(Some).ok_or_else(|| {
                PlatformError::invalid_request("invalid_cursor", "after is not a valid cursor")
                    .with_param("after")
            }),
        }
    }
}

fn number_or_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        N(i64),
        S(String),
    }
    match Option::<Raw>::deserialize(d)? {
        None => Ok(None),
        Some(Raw::N(n)) => Ok(Some(n)),
        Some(Raw::S(s)) if s.trim().is_empty() => Ok(None),
        // A non-number is kept out of range so `limit()` answers `invalid_limit`.
        Some(Raw::S(s)) => Ok(Some(s.trim().parse().unwrap_or(-1))),
    }
}

pub fn encode_cursor(created_at: DateTime<Utc>, id: &str) -> String {
    hex::encode(format!("{}|{}", created_at.timestamp_micros(), id))
}

pub fn decode_cursor(raw: &str) -> Option<(DateTime<Utc>, String)> {
    let bytes = hex::decode(raw).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let (micros, id) = text.split_once('|')?;
    let micros: i64 = micros.parse().ok()?;
    let at = Utc.timestamp_micros(micros).single()?;
    if id.is_empty() {
        return None;
    }
    Some((at, id.to_string()))
}

/// Turn `limit + 1` fetched rows into a page. `key` extracts `(created_at, id)`.
pub fn build_page<T: Serialize>(
    mut rows: Vec<T>,
    limit: i64,
    key: impl Fn(&T) -> (DateTime<Utc>, String),
) -> Page<T> {
    let has_more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let next_cursor = if has_more {
        rows.last().map(|row| {
            let (at, id) = key(row);
            encode_cursor(at, &id)
        })
    } else {
        None
    };
    Page { data: rows, has_more, next_cursor }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_roundtrips() {
        let now = Utc::now();
        let (at, id) = decode_cursor(&encode_cursor(now, "acct_1")).unwrap();
        assert_eq!(at.timestamp_micros(), now.timestamp_micros());
        assert_eq!(id, "acct_1");
        assert!(decode_cursor("zz").is_none());
        assert!(decode_cursor(&hex::encode("nope")).is_none());
    }

    #[test]
    fn limit_validation() {
        assert_eq!(PageParams::default().limit().unwrap(), DEFAULT_LIMIT);
        assert!(PageParams { limit: Some(0), after: None }.limit().is_err());
        assert!(PageParams { limit: Some(101), after: None }.limit().is_err());
        assert_eq!(PageParams { limit: Some(100), after: None }.limit().unwrap(), 100);
    }
}
