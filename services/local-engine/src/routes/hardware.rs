//! `GET /hardware`: the device profile and model budget the on-device fit
//! function reads (contract: plan "On-device models", section 3).
//!
//! GPU and engine probes run external commands (up to 2 s each), so the full
//! report is cached for [`FULL_TTL`]; free RAM / VRAM / disk are re-read at
//! most every [`LIVE_TTL`]. [`invalidate`] forces a full re-probe, e.g. after
//! an engine pack is installed.

use crate::hardware::{self, EngineState, HardwareReport};
use crate::AppState;
use axum::{extract::State, response::Json, routing::get, Router};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// How long GPUs and engines are trusted before a full re-probe.
pub const FULL_TTL: Duration = Duration::from_secs(5 * 60);
/// Re-probe sooner when an engine could not be asked (a probe timed out).
pub const RETRY_TTL: Duration = Duration::from_secs(30);
/// How long the free-memory fields are trusted.
pub const LIVE_TTL: Duration = Duration::from_secs(3);

struct Cached {
    report: HardwareReport,
    probed_at: Instant,
    live_at: Instant,
}

fn cache() -> &'static Mutex<Option<Cached>> {
    static CACHE: OnceLock<Mutex<Option<Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Drop the cached report so the next request probes again.
pub async fn invalidate() {
    *cache().lock().await = None;
}

pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/hardware", get(get_hardware))
        .with_state(state)
}

async fn get_hardware(State(state): State<Arc<AppState>>) -> Json<HardwareReport> {
    // Held across the probe so concurrent first requests probe once.
    let mut guard = cache().lock().await;
    let now = Instant::now();
    let models_dir = state.models_dir.clone();

    let needs_full = guard.as_ref().map_or(true, |c| {
        let incomplete = c.report.engines.iter().any(|e| e.state == EngineState::Installed);
        now.duration_since(c.probed_at) >= if incomplete { RETRY_TTL } else { FULL_TTL }
    });
    if needs_full {
        let profile = state.hardware_profile.clone();
        let dir = models_dir.clone();
        let report = match tokio::task::spawn_blocking(move || hardware::probe_report(&profile, &dir)).await {
            Ok(report) => report,
            // The probe never panics by design; if it somehow did, answer
            // with a fresh probe on this thread rather than a 500.
            Err(_) => hardware::probe_report(&state.hardware_profile, &models_dir),
        };
        let at = Instant::now();
        *guard = Some(Cached { report, probed_at: at, live_at: at });
    } else if let Some(cached) = guard.as_mut() {
        if now.duration_since(cached.live_at) >= LIVE_TTL {
            let mut report = cached.report.clone();
            let dir = models_dir.clone();
            if let Ok(fresh) = tokio::task::spawn_blocking(move || {
                hardware::refresh_live(&mut report, &dir);
                report
            })
            .await
            {
                cached.report = fresh;
            }
            cached.live_at = Instant::now();
        }
    }

    match guard.as_ref() {
        Some(c) => Json(c.report.clone()),
        // Unreachable: both branches above leave the cache filled.
        None => Json(hardware::probe_report(&state.hardware_profile, &models_dir)),
    }
}
