//! Operational alerts emailed to the team.
//!
//! cloud-api has no mail sender of its own; alerts go to the services
//! worker's `/ops-alert` endpoint (allternit-websites,
//! `projects/services.allternit.com/worker`), which emails hello@allternit.com.
//! Configure with `ALLTERNIT_OPS_ALERT_URL` and `ALLTERNIT_OPS_ALERT_TOKEN`;
//! unset means alerts are only logged.
//!
//! Alerts are fire-and-forget and deduplicated per key for
//! [`REPEAT_AFTER`], because the same condition (a computer waiting for a
//! server) is re-detected on every retry sweep and Stripe redelivery.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const ENV_URL: &str = "ALLTERNIT_OPS_ALERT_URL";
const ENV_TOKEN: &str = "ALLTERNIT_OPS_ALERT_TOKEN";
/// The same alert key is sent again only after this long.
pub const REPEAT_AFTER: Duration = Duration::from_secs(6 * 3600);

fn sent() -> &'static Mutex<HashMap<String, Instant>> {
    static SENT: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    SENT.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Whether `key` should alert now; records it when it should.
fn claim(key: &str, now: Instant) -> bool {
    let mut sent = sent().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    sent.retain(|_, at| now.duration_since(*at) < REPEAT_AFTER);
    if sent.contains_key(key) {
        return false;
    }
    sent.insert(key.to_string(), now);
    true
}

/// Email `subject`/`body` to the team, at most once per `key` every
/// [`REPEAT_AFTER`]. Never blocks or fails the caller.
pub fn send_once(key: &str, subject: String, body: String) {
    if !claim(key, Instant::now()) {
        return;
    }
    tracing::warn!(alert_key = %key, %subject, "ops alert");
    let (Ok(url), Ok(token)) = (std::env::var(ENV_URL), std::env::var(ENV_TOKEN)) else {
        return;
    };
    if url.trim().is_empty() || token.trim().is_empty() {
        return;
    }
    let key = key.to_string();
    tokio::spawn(async move {
        let result = reqwest::Client::new()
            .post(url.trim())
            .bearer_auth(token.trim())
            .timeout(Duration::from_secs(15))
            .json(&serde_json::json!({ "subject": subject, "body": body }))
            .send()
            .await;
        match result {
            Ok(response) if response.status().is_success() => {}
            Ok(response) => {
                tracing::error!(alert_key = %key, status = %response.status(), "ops alert not delivered");
            }
            Err(error) => tracing::error!(alert_key = %key, %error, "ops alert not delivered"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_key_alerts_once_per_window() {
        let start = Instant::now();
        assert!(claim("test:dedupe", start));
        assert!(!claim("test:dedupe", start + Duration::from_secs(60)));
        assert!(claim("test:other", start));
        assert!(claim("test:dedupe", start + REPEAT_AFTER + Duration::from_secs(1)));
    }
}
