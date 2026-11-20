//! Opt-in location for bot chats.
//!
//! When the user turns on Settings → Privacy → "Share my location with bots"
//! (off by default), the client adds `client_location` to each chat send:
//! `{ lat, lon, accuracy_m, time_zone, at }`, lat/lon already rounded to two
//! decimals (about 1 km). The server checks it again and sends it to gizzi
//! twice, for that turn only: as one short system layer for the model
//! ([`location_layer`], in the turn-only `turnSystem` field) and as a
//! structured value for tools ([`turn_value`], in the turn-only
//! `turnLocation` field; lookup's "here" reads it, never the prose).
//!
//! Never stored: neither is part of the composed `system` block or the
//! forwarded `metadata` (gizzi keeps both on the user message); gizzi holds
//! `turnSystem` and `turnLocation` in memory for the turn and drops them. The
//! values are never logged. Anything invalid is ignored; a bad location never
//! fails a send.

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

/// How old a shared location may be before it is ignored.
const MAX_AGE_MINUTES: i64 = 30;
/// Clock skew allowed for a timestamp slightly in the future.
const MAX_FUTURE_SKEW_MINUTES: i64 = 5;
/// The radius stated to the model is never finer than this.
const MIN_ACCURACY_M: f64 = 1000.0;

/// The `client_location` field of a chat send body, as the client sends it.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ClientLocation {
    pub lat: f64,
    pub lon: f64,
    #[serde(default)]
    pub accuracy_m: Option<f64>,
    pub time_zone: String,
    pub at: String,
    /// `"precise"` only when the person tapped "Share precise location" for this
    /// turn; anything else (or absent) is approximate.
    #[serde(default)]
    pub precision: Option<String>,
}

/// A location that passed every check, already rounded for the prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct SharedLocation {
    pub lat: f64,
    pub lon: f64,
    pub km: u64,
    pub time_zone: String,
    /// When the device took it (RFC 3339, UTC).
    pub at: String,
    /// Shared precise for this turn only (still never stored).
    pub precise: bool,
    /// Stated radius in metres (≥ 1000 approximate, ≥ 10 precise).
    pub accuracy_m: u64,
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// About 11 m: the finest a precise share is passed on.
fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

/// The radius stated for a precise share is never finer than this.
const MIN_PRECISE_ACCURACY_M: f64 = 10.0;

/// Read `client_location` from a send body. Absent, malformed or invalid →
/// None (the send goes on without a location layer).
pub fn from_body(body: &serde_json::Value, now: DateTime<Utc>) -> Option<SharedLocation> {
    let raw = body.get("client_location").filter(|v| v.is_object())?;
    let loc: ClientLocation = serde_json::from_value(raw.clone()).ok()?;
    validate(&loc, now)
}

/// Check and round a client location.
pub fn validate(loc: &ClientLocation, now: DateTime<Utc>) -> Option<SharedLocation> {
    if !loc.lat.is_finite() || !loc.lon.is_finite() {
        return None;
    }
    if !(-90.0..=90.0).contains(&loc.lat) || !(-180.0..=180.0).contains(&loc.lon) {
        return None;
    }
    let precise = loc.precision.as_deref() == Some("precise");
    let floor = if precise { MIN_PRECISE_ACCURACY_M } else { MIN_ACCURACY_M };
    let accuracy = match loc.accuracy_m {
        Some(a) if !a.is_finite() => return None,
        Some(a) => a.max(floor),
        None => floor,
    };
    let at = DateTime::parse_from_rfc3339(loc.at.trim())
        .ok()?
        .with_timezone(&Utc);
    if at < now - Duration::minutes(MAX_AGE_MINUTES)
        || at > now + Duration::minutes(MAX_FUTURE_SKEW_MINUTES)
    {
        return None;
    }
    let tz = loc.time_zone.trim();
    if tz.is_empty() || tz.len() > 64 || tz.parse::<chrono_tz::Tz>().is_err() {
        return None;
    }
    // Round again server-side; normalise -0.00 to 0.00.
    let round = if precise { round4 } else { round2 };
    let lat = round(loc.lat) + 0.0;
    let lon = round(loc.lon) + 0.0;
    Some(SharedLocation {
        lat,
        lon,
        km: (accuracy / 1000.0).round().max(1.0) as u64,
        time_zone: tz.to_string(),
        at: at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        precise,
        accuracy_m: if precise { accuracy.round() as u64 } else { (accuracy / 1000.0).round().max(1.0) as u64 * 1000 },
    })
}

/// The structured `turnLocation` value gizzi gets for the turn:
/// `{ lat, lon, accuracy_m, time_zone, at }`, the same rounded values and
/// radius as [`location_layer`] states (accuracy_m = the stated km × 1000).
/// gizzi's lookup tool reads this for "here"; it never parses the layer text.
pub fn turn_value(loc: &SharedLocation) -> serde_json::Value {
    serde_json::json!({
        "lat": loc.lat,
        "lon": loc.lon,
        "accuracy_m": loc.accuracy_m,
        "time_zone": loc.time_zone,
        "at": loc.at,
        "precision": if loc.precise { "precise" } else { "approximate" },
    })
}

/// The system layer for a shared location.
pub fn location_layer(loc: &SharedLocation) -> String {
    if loc.precise {
        return format!(
            "Where the user is (shared precisely from their device for this turn only): {:.4}, {:.4}, within about {} m. Their time zone: {}. Use it only for what they asked. Don't repeat the coordinates unless needed, and don't save it to memory.",
            loc.lat, loc.lon, loc.accuracy_m, loc.time_zone
        );
    }
    format!(
        "Where the user is (shared from their device just now, approximate): about {:.2}, {:.2}, within roughly {} km. Their time zone: {}. Use this only when it helps, for local time, nearby places, weather or directions. Don't mention it otherwise, and don't save it to memory.",
        loc.lat, loc.lon, loc.km, loc.time_zone
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-24T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn body(lat: f64, lon: f64, acc: f64, tz: &str, at: &str) -> serde_json::Value {
        json!({
            "chatId": "c1",
            "message": "what's the weather",
            "client_location": { "lat": lat, "lon": lon, "accuracy_m": acc, "time_zone": tz, "at": at }
        })
    }

    #[test]
    fn valid_location_gives_layer_text() {
        let loc = from_body(
            &body(
                40.71,
                -74.01,
                35.0,
                "America/New_York",
                "2026-10-24T11:55:00Z",
            ),
            now(),
        )
        .unwrap();
        assert_eq!(
            location_layer(&loc),
            "Where the user is (shared from their device just now, approximate): about 40.71, -74.01, within roughly 1 km. Their time zone: America/New_York. Use this only when it helps, for local time, nearby places, weather or directions. Don't mention it otherwise, and don't save it to memory."
        );
    }

    #[test]
    fn turn_value_is_the_rounded_structured_location() {
        let loc = from_body(
            &body(
                40.712_8,
                -74.006_1,
                4600.0,
                "America/New_York",
                "2026-10-24T11:55:00Z",
            ),
            now(),
        )
        .unwrap();
        assert_eq!(
            turn_value(&loc),
            json!({
                "lat": 40.71, "lon": -74.01, "accuracy_m": 5000,
                "time_zone": "America/New_York", "at": "2026-10-24T11:55:00Z",
                "precision": "approximate"
            })
        );
    }

    #[test]
    fn precise_share_keeps_about_eleven_metres_for_this_turn() {
        let mut raw = body(40.712_84, -74.006_13, 4.0, "America/New_York", "2026-10-24T11:55:00Z");
        raw["client_location"]["precision"] = json!("precise");
        let loc = from_body(&raw, now()).unwrap();
        assert!(loc.precise);
        let v = turn_value(&loc);
        assert_eq!(v["lat"], json!(40.7128));
        assert_eq!(v["lon"], json!(-74.0061));
        assert_eq!(v["accuracy_m"], json!(10));
        assert_eq!(v["precision"], json!("precise"));
        assert!(location_layer(&loc).contains("for this turn only"));
    }

    #[test]
    fn rounds_again_and_clamps_accuracy() {
        let loc = from_body(
            &body(
                51.507_412,
                -0.127_758,
                4600.0,
                "Europe/London",
                "2026-10-24T11:59:00Z",
            ),
            now(),
        )
        .unwrap();
        assert_eq!((loc.lat, loc.lon, loc.km), (51.51, -0.13, 5));
        let tiny = from_body(
            &body(-0.001, 0.004, 3.0, "UTC", "2026-10-24T12:00:00Z"),
            now(),
        )
        .unwrap();
        assert_eq!((tiny.lat, tiny.lon, tiny.km), (0.0, 0.0, 1));
        assert!(location_layer(&tiny).contains("about 0.00, 0.00, within roughly 1 km"));
    }

    #[test]
    fn out_of_range_is_ignored() {
        assert_eq!(
            from_body(&body(90.5, 0.0, 10.0, "UTC", "2026-10-24T11:59:00Z"), now()),
            None
        );
        assert_eq!(
            from_body(
                &body(0.0, -180.5, 10.0, "UTC", "2026-10-24T11:59:00Z"),
                now()
            ),
            None
        );
    }

    #[test]
    fn stale_or_future_is_ignored() {
        assert_eq!(
            from_body(&body(1.0, 1.0, 10.0, "UTC", "2026-10-24T11:29:00Z"), now()),
            None
        );
        assert_eq!(
            from_body(&body(1.0, 1.0, 10.0, "UTC", "2026-10-24T12:10:00Z"), now()),
            None
        );
        assert_eq!(
            from_body(&body(1.0, 1.0, 10.0, "UTC", "yesterday"), now()),
            None
        );
    }

    #[test]
    fn bad_time_zone_is_ignored() {
        assert_eq!(
            from_body(
                &body(1.0, 1.0, 10.0, "Mars/Olympus_Mons", "2026-10-24T11:59:00Z"),
                now()
            ),
            None
        );
        assert_eq!(
            from_body(&body(1.0, 1.0, 10.0, "", "2026-10-24T11:59:00Z"), now()),
            None
        );
        assert_eq!(
            from_body(
                &body(1.0, 1.0, 10.0, &"A".repeat(65), "2026-10-24T11:59:00Z"),
                now()
            ),
            None
        );
    }

    #[test]
    fn absent_or_malformed_is_ignored() {
        assert_eq!(
            from_body(&json!({ "chatId": "c1", "message": "hi" }), now()),
            None
        );
        assert_eq!(
            from_body(&json!({ "client_location": "here" }), now()),
            None
        );
        assert_eq!(
            from_body(&json!({ "client_location": { "lat": "x" } }), now()),
            None
        );
    }
}
