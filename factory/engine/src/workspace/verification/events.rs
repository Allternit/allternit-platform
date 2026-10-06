//! Visual Verification Events
//!
//! Events emitted by the visual verification system for audit trails
//! and observability.

use crate::core::types::{AllternitEvent, Actor};
use chrono::Utc;
use serde_json::json;

/// Create a VisualVerificationRequested event
pub fn visual_verification_requested(
    wih_id: &str,
    provider_type: &str,
    timeout_seconds: u64,
) -> AllternitEvent {
    AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor: system_actor("visual-verification"),
        scope: None,
        r#type: "VisualVerificationRequested".to_string(),
        payload: json!({
            "wih_id": wih_id,
            "provider_type": provider_type,
            "timeout_seconds": timeout_seconds,
        }),
        provenance: None,
    }
}

/// Create a VisualVerificationStarted event
pub fn visual_verification_started(
    wih_id: &str,
    provider_type: &str,
) -> AllternitEvent {
    AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor: system_actor("visual-verification"),
        scope: None,
        r#type: "VisualVerificationStarted".to_string(),
        payload: json!({
            "wih_id": wih_id,
            "provider_type": provider_type,
        }),
        provenance: None,
    }
}

/// Create a VisualVerificationCompleted event
pub fn visual_verification_completed(
    wih_id: &str,
    success: bool,
    confidence: f64,
    artifact_count: usize,
    duration_ms: u64,
) -> AllternitEvent {
    AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor: system_actor("visual-verification"),
        scope: None,
        r#type: "VisualVerificationCompleted".to_string(),
        payload: json!({
            "wih_id": wih_id,
            "success": success,
            "confidence": confidence,
            "artifact_count": artifact_count,
            "duration_ms": duration_ms,
        }),
        provenance: None,
    }
}

/// Create a VisualVerificationFailed event
pub fn visual_verification_failed(
    wih_id: &str,
    error: &str,
    error_code: &str,
) -> AllternitEvent {
    AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor: system_actor("visual-verification"),
        scope: None,
        r#type: "VisualVerificationFailed".to_string(),
        payload: json!({
            "wih_id": wih_id,
            "error": error,
            "error_code": error_code,
        }),
        provenance: None,
    }
}

/// Create a VisualVerificationBypassed event
pub fn visual_verification_bypassed(
    wih_id: &str,
    actor_id: &str,
    reason: &str,
) -> AllternitEvent {
    AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor: Actor {
            actor_type: "agent".to_string(),
            actor_id: actor_id.to_string(),
        },
        scope: None,
        r#type: "VisualVerificationBypassed".to_string(),
        payload: json!({
            "wih_id": wih_id,
            "bypassed_by": actor_id,
            "reason": reason,
        }),
        provenance: None,
    }
}

/// Create a GateVisualVerified event (emitted after gate passes)
pub fn gate_visual_verified(
    wih_id: &str,
    confidence: f64,
    threshold: f64,
    artifact_count: usize,
) -> AllternitEvent {
    AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor: system_actor("gate"),
        scope: None,
        r#type: "GateVisualVerified".to_string(),
        payload: json!({
            "wih_id": wih_id,
            "confidence": confidence,
            "threshold": threshold,
            "artifact_count": artifact_count,
            "passed": confidence >= threshold,
        }),
        provenance: None,
    }
}

/// Create a GateVisualRejected event
pub fn gate_visual_rejected(
    wih_id: &str,
    confidence: f64,
    threshold: f64,
    reason: &str,
) -> AllternitEvent {
    AllternitEvent {
        event_id: create_event_id(),
        ts: Utc::now().to_rfc3339(),
        actor: system_actor("gate"),
        scope: None,
        r#type: "GateVisualRejected".to_string(),
        payload: json!({
            "wih_id": wih_id,
            "confidence": confidence,
            "threshold": threshold,
            "reason": reason,
        }),
        provenance: None,
    }
}

fn system_actor(name: &str) -> Actor {
    Actor {
        actor_type: "system".to_string(),
        actor_id: name.to_string(),
    }
}

fn create_event_id() -> String {
    format!("evt_{}_{}", Utc::now().timestamp_millis(), rand::random::<u32>() % 10000)
}
