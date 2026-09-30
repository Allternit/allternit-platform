//! Review lifecycle + held-update state machine for MCP App listings.
//!
//! Ports `directory/submission.ts::reduceReviewState` and
//! `directory/held-update.ts` so the rules are enforced server-side and
//! survive restarts. Once a listing is approved its approved tool-metadata
//! snapshot stays live; new or changed tools (title, description, inputSchema,
//! annotations, _meta) and changed `ui://` resource metadata stay pending until
//! an admin approves. Removals only narrow scope, so they take effect at once.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ─── Review lifecycle ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    Draft,
    InReview,
    Approved,
    Rejected,
    Published,
}

impl ReviewState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::InReview => "in_review",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Published => "published",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "draft" => Self::Draft,
            "in_review" => Self::InReview,
            "approved" => Self::Approved,
            "rejected" => Self::Rejected,
            "published" => Self::Published,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewEvent {
    Submit,
    Approve,
    Reject { reason: String },
    Publish,
    Unpublish,
}

impl ReviewEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::Submit => "submit",
            Self::Approve => "approve",
            Self::Reject { .. } => "reject",
            Self::Publish => "publish",
            Self::Unpublish => "unpublish",
        }
    }
}

/// Illegal transitions return `Err`; reject requires a non-empty reason.
pub fn reduce_review_state(state: ReviewState, event: &ReviewEvent) -> Result<ReviewState, String> {
    use ReviewState::*;
    match (event, state) {
        (ReviewEvent::Submit, Draft | Rejected) => Ok(InReview),
        (ReviewEvent::Approve, InReview) => Ok(Approved),
        (ReviewEvent::Reject { reason }, InReview) => {
            if reason.trim().is_empty() {
                Err("A rejection reason is required.".into())
            } else {
                Ok(Rejected)
            }
        }
        // Developer "publish when ready" only after approval.
        (ReviewEvent::Publish, Approved) => Ok(Published),
        (ReviewEvent::Unpublish, Published) => Ok(Approved),
        _ => Err(format!("Cannot {} from state {}.", event.name(), state.as_str())),
    }
}

// ─── Held updates ───────────────────────────────────────────────────────────

/// `ListingSnapshot`: tool definitions and scanned resources, kept as opaque
/// JSON so unknown fields round-trip unchanged.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ListingSnapshot {
    #[serde(default)]
    pub tools: Vec<Value>,
    #[serde(default)]
    pub resources: Vec<Value>,
}

impl ListingSnapshot {
    /// Every tool needs a string `name`, every resource a string `uri`, and
    /// names/URIs must be unique (a duplicate would let one entry mask another
    /// in the diff).
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = std::collections::BTreeSet::new();
        for t in &self.tools {
            let name = t.get("name").and_then(Value::as_str).filter(|n| !n.is_empty());
            let name = name.ok_or("Every tool in the snapshot needs a non-empty name.")?;
            if !seen.insert(name) {
                return Err(format!("Duplicate tool name in snapshot: {name}."));
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for r in &self.resources {
            let uri = r.get("uri").and_then(Value::as_str).filter(|n| !n.is_empty());
            let uri = uri.ok_or("Every resource in the snapshot needs a non-empty uri.")?;
            if !seen.insert(uri) {
                return Err(format!("Duplicate resource uri in snapshot: {uri}."));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HeldChange {
    /// "added" | "changed"
    pub kind: &'static str,
    pub tool: String,
    /// Which fields changed (empty for "added").
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeldUpdate {
    pub pending: bool,
    pub held: Vec<HeldChange>,
    pub held_resources: Vec<String>,
    pub removed: Vec<String>,
    /// What users see now: approved metadata, minus removed tools.
    pub live: ListingSnapshot,
}

const TOOL_FIELDS: [&str; 5] = ["title", "description", "inputSchema", "annotations", "_meta"];

fn name_of(v: &Value) -> &str {
    v.get("name").and_then(Value::as_str).unwrap_or("")
}
fn uri_of(v: &Value) -> &str {
    v.get("uri").and_then(Value::as_str).unwrap_or("")
}

/// A missing field and an explicit `null` are the same thing (the TS side
/// serialises `undefined` as absent).
fn field<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k).unwrap_or(&Value::Null)
}

pub fn changed_tool_fields(a: &Value, b: &Value) -> Vec<String> {
    TOOL_FIELDS
        .iter()
        .filter(|k| field(a, k) != field(b, k))
        .map(|k| k.to_string())
        .collect()
}

fn resource_key(r: &Value) -> Value {
    json!({
        "mimeType": field(r, "mimeType"),
        "ui": r.get("meta").and_then(|m| m.get("ui")).unwrap_or(&Value::Null),
    })
}

/// `approved` is `None` for a first submission: everything is held.
pub fn compute_held_update(approved: Option<&ListingSnapshot>, candidate: &ListingSnapshot) -> HeldUpdate {
    let Some(approved) = approved else {
        return HeldUpdate {
            pending: true,
            held: candidate
                .tools
                .iter()
                .map(|t| HeldChange { kind: "added", tool: name_of(t).to_string(), fields: vec![] })
                .collect(),
            held_resources: candidate
                .resources
                .iter()
                .filter(|r| uri_of(r).starts_with("ui://"))
                .map(|r| uri_of(r).to_string())
                .collect(),
            removed: vec![],
            live: ListingSnapshot::default(),
        };
    };

    let before: BTreeMap<&str, &Value> = approved.tools.iter().map(|t| (name_of(t), t)).collect();
    let after: BTreeMap<&str, &Value> = candidate.tools.iter().map(|t| (name_of(t), t)).collect();

    let mut held = Vec::new();
    // Candidate order, like the TS Map iteration.
    for t in &candidate.tools {
        let name = name_of(t);
        match before.get(name) {
            None => held.push(HeldChange { kind: "added", tool: name.to_string(), fields: vec![] }),
            Some(prev) => {
                let fields = changed_tool_fields(prev, t);
                if !fields.is_empty() {
                    held.push(HeldChange { kind: "changed", tool: name.to_string(), fields });
                }
            }
        }
    }
    let removed: Vec<String> = approved
        .tools
        .iter()
        .map(|t| name_of(t).to_string())
        .filter(|n| !after.contains_key(n.as_str()))
        .collect();

    let prev_res: BTreeMap<&str, &Value> = approved.resources.iter().map(|r| (uri_of(r), r)).collect();
    let cand_res: BTreeMap<&str, &Value> = candidate.resources.iter().map(|r| (uri_of(r), r)).collect();
    let held_resources: Vec<String> = candidate
        .resources
        .iter()
        .filter(|r| {
            let u = uri_of(r);
            u.starts_with("ui://") && prev_res.get(u).map_or(true, |p| resource_key(p) != resource_key(r))
        })
        .map(|r| uri_of(r).to_string())
        .collect();

    let live = ListingSnapshot {
        tools: approved.tools.iter().filter(|t| after.contains_key(name_of(t))).cloned().collect(),
        resources: approved.resources.iter().filter(|r| cand_res.contains_key(uri_of(r))).cloned().collect(),
    };
    HeldUpdate { pending: !held.is_empty() || !held_resources.is_empty(), held, held_resources, removed, live }
}

/// Reviewer approval promotes the candidate snapshot to the live one.
pub fn approve_held_update(candidate: &ListingSnapshot) -> ListingSnapshot {
    candidate.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, desc: &str) -> Value {
        json!({ "name": name, "description": desc, "inputSchema": { "type": "object", "properties": {} } })
    }
    fn ui(uri: &str, csp: &str) -> Value {
        json!({ "uri": uri, "mimeType": "text/html;profile=mcp-app", "meta": { "ui": { "csp": { "connectDomains": [csp] } } } })
    }
    fn snap(tools: Vec<Value>, resources: Vec<Value>) -> ListingSnapshot {
        ListingSnapshot { tools, resources }
    }

    // ── review lifecycle ──────────────────────────────────────────────────

    #[test]
    fn lifecycle_happy_path() {
        use ReviewState::*;
        let s = reduce_review_state(Draft, &ReviewEvent::Submit).unwrap();
        assert_eq!(s, InReview);
        let s = reduce_review_state(s, &ReviewEvent::Approve).unwrap();
        assert_eq!(s, Approved);
        let s = reduce_review_state(s, &ReviewEvent::Publish).unwrap();
        assert_eq!(s, Published);
        assert_eq!(reduce_review_state(s, &ReviewEvent::Unpublish).unwrap(), Approved);
    }

    #[test]
    fn publish_only_from_approved() {
        use ReviewState::*;
        for s in [Draft, InReview, Rejected, Published] {
            assert!(reduce_review_state(s, &ReviewEvent::Publish).is_err(), "{s:?}");
        }
    }

    #[test]
    fn reject_needs_reason_and_can_resubmit() {
        use ReviewState::*;
        assert!(reduce_review_state(InReview, &ReviewEvent::Reject { reason: "  ".into() }).is_err());
        let s = reduce_review_state(InReview, &ReviewEvent::Reject { reason: "no privacy policy".into() }).unwrap();
        assert_eq!(s, Rejected);
        assert_eq!(reduce_review_state(s, &ReviewEvent::Submit).unwrap(), InReview);
        assert!(reduce_review_state(Approved, &ReviewEvent::Reject { reason: "x".into() }).is_err());
        assert!(reduce_review_state(InReview, &ReviewEvent::Submit).is_err());
        assert!(reduce_review_state(Draft, &ReviewEvent::Approve).is_err());
    }

    #[test]
    fn state_strings_round_trip() {
        for s in ["draft", "in_review", "approved", "rejected", "published"] {
            assert_eq!(ReviewState::parse(s).unwrap().as_str(), s);
        }
        assert!(ReviewState::parse("live").is_none());
    }

    // ── held updates ──────────────────────────────────────────────────────

    #[test]
    fn first_submission_holds_everything() {
        let cand = snap(vec![tool("a", "x"), tool("b", "y")], vec![ui("ui://a", "https://x"), json!({"uri":"file://n"})]);
        let h = compute_held_update(None, &cand);
        assert!(h.pending);
        assert_eq!(h.held.len(), 2);
        assert_eq!(h.held_resources, vec!["ui://a"]);
        assert!(h.live.tools.is_empty() && h.live.resources.is_empty());
    }

    #[test]
    fn unchanged_candidate_is_not_pending() {
        let a = snap(vec![tool("a", "x")], vec![ui("ui://a", "https://x")]);
        let h = compute_held_update(Some(&a), &a.clone());
        assert!(!h.pending && h.held.is_empty() && h.held_resources.is_empty() && h.removed.is_empty());
        assert_eq!(h.live, a);
    }

    #[test]
    fn new_tool_is_held_and_not_live() {
        let a = snap(vec![tool("a", "x")], vec![]);
        let c = snap(vec![tool("a", "x"), tool("b", "y")], vec![]);
        let h = compute_held_update(Some(&a), &c);
        assert!(h.pending);
        assert_eq!(h.held, vec![HeldChange { kind: "added", tool: "b".into(), fields: vec![] }]);
        assert_eq!(h.live.tools.len(), 1, "live stays the approved snapshot");
    }

    #[test]
    fn changed_tool_reports_which_fields_and_stays_on_old_metadata() {
        let a = snap(vec![tool("a", "old")], vec![]);
        let mut changed = tool("a", "new description that now exfiltrates");
        changed["annotations"] = json!({ "destructiveHint": true });
        let h = compute_held_update(Some(&a), &snap(vec![changed], vec![]));
        assert!(h.pending);
        assert_eq!(h.held[0].kind, "changed");
        assert_eq!(h.held[0].fields, vec!["description", "annotations"]);
        assert_eq!(h.live.tools[0]["description"], "old");
    }

    #[test]
    fn key_order_and_missing_vs_null_do_not_count_as_changes() {
        let a = snap(vec![json!({"name":"a","inputSchema":{"a":1,"b":2}})], vec![]);
        let c = snap(vec![json!({"inputSchema":{"b":2,"a":1},"name":"a","title":null})], vec![]);
        assert!(!compute_held_update(Some(&a), &c).pending);
    }

    #[test]
    fn removal_takes_effect_immediately_and_is_not_pending() {
        let a = snap(vec![tool("a", "x"), tool("b", "y")], vec![ui("ui://a", "https://x")]);
        let c = snap(vec![tool("a", "x")], vec![]);
        let h = compute_held_update(Some(&a), &c);
        assert!(!h.pending);
        assert_eq!(h.removed, vec!["b"]);
        assert_eq!(h.live.tools.len(), 1);
        assert!(h.live.resources.is_empty());
    }

    #[test]
    fn ui_csp_change_and_new_ui_resource_are_held_others_are_not() {
        let a = snap(vec![], vec![ui("ui://a", "https://x")]);
        let c = snap(vec![], vec![ui("ui://a", "https://evil"), ui("ui://b", "https://x"), json!({"uri":"https://plain","mimeType":"text/plain"})]);
        let h = compute_held_update(Some(&a), &c);
        assert!(h.pending);
        assert_eq!(h.held_resources, vec!["ui://a", "ui://b"]);
        assert_eq!(h.live.resources.len(), 1);
        assert_eq!(h.live.resources[0]["meta"]["ui"]["csp"]["connectDomains"][0], "https://x");
    }

    #[test]
    fn approving_promotes_candidate_then_nothing_is_pending() {
        let a = snap(vec![tool("a", "x")], vec![]);
        let c = snap(vec![tool("a", "x"), tool("b", "y")], vec![]);
        assert!(compute_held_update(Some(&a), &c).pending);
        let promoted = approve_held_update(&c);
        assert!(!compute_held_update(Some(&promoted), &c).pending);
    }

    #[test]
    fn snapshot_validation_rejects_nameless_and_duplicate_entries() {
        assert!(snap(vec![json!({"description":"no name"})], vec![]).validate().is_err());
        assert!(snap(vec![tool("a", "x"), tool("a", "y")], vec![]).validate().is_err());
        assert!(snap(vec![], vec![json!({"mimeType":"x"})]).validate().is_err());
        assert!(snap(vec![], vec![ui("ui://a", "x"), ui("ui://a", "y")]).validate().is_err());
        assert!(snap(vec![tool("a", "x")], vec![ui("ui://a", "x")]).validate().is_ok());
    }
}
