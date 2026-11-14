//! The `meetings` MCP connector (docs/meetings/CONTRACT.md section 8).
//!
//! Served by the connectors MCP front (`/connectors/mcp` and
//! `/internal/connectors/mcp`, see `connector_routes.rs`) as built-in tools
//! named `meetings__<tool>`: `list_meetings`, `get_meeting`, `add_meeting`,
//! `join_meeting`, `set_auto_join`, `get_recap`, `get_transcript`,
//! `leave_meeting`. (Not `meeting_tools.rs`, which is the in-call agent's
//! tool runner.)
//!
//! Every tool calls cloud-api's Meetings routes with this runtime's own
//! device token (`Authorization: Bearer allternit_runtime_...`, which
//! cloud-api accepts on the meetings routes and resolves to the owner). The
//! output shapes are the ones `allternit-ai/src/lib/app-screens/packs/meetings.json`
//! renders; the free-plan answer is an MCP error `{error:"meetings_requires_paid_plan"}`.

use std::time::Duration;

use serde_json::{json, Map, Value};

pub const PREFIX: &str = "meetings__";
pub const PAGE_BASE: &str = "https://ai.allternit.com/meetings";
const TOOLS: [&str; 8] = ["list_meetings", "get_meeting", "add_meeting", "join_meeting", "set_auto_join", "get_recap", "get_transcript", "leave_meeting"];

pub fn is_tool(name: &str) -> bool {
    name.strip_prefix(PREFIX).is_some_and(|t| TOOLS.contains(&t))
}

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({ "name": format!("{PREFIX}{name}"), "description": description, "inputSchema": schema })
}

/// The tool list, schemas exactly as in the contract.
pub fn tools() -> Vec<Value> {
    let id = json!({ "type": "string" });
    vec![
        tool("list_meetings", "List the user's meetings (today by default), with whether Gizzi is joining each one.", json!({
            "type": "object",
            "properties": { "range": { "enum": ["now", "today", "week", "past"], "default": "today" } },
            "additionalProperties": false
        })),
        tool("get_meeting", "Look up one meeting by id or call link. Creates nothing.", json!({
            "type": "object",
            "properties": { "url": { "type": "string" }, "meeting_id": { "type": "string" } },
            "minProperties": 1,
            "additionalProperties": false
        })),
        tool("add_meeting", "Add a Google Meet, Zoom or Teams call link to the user's meetings.", json!({
            "type": "object",
            "properties": { "url": { "type": "string" }, "title": { "type": "string" }, "starts_at": { "type": "string", "format": "date-time" } },
            "required": ["url"],
            "additionalProperties": false
        })),
        tool("join_meeting", "Send Gizzi into a meeting, now or when it starts.", json!({
            "type": "object",
            "properties": { "meeting_id": id, "when": { "enum": ["now", "start"] } },
            "required": ["meeting_id", "when"],
            "additionalProperties": false
        })),
        tool("set_auto_join", "Turn Gizzi's automatic joining on or off for one meeting.", json!({
            "type": "object",
            "properties": { "meeting_id": id, "on": { "type": "boolean" } },
            "required": ["meeting_id", "on"],
            "additionalProperties": false
        })),
        tool("get_recap", "The recap of a finished meeting: summary, decisions and action items.", json!({
            "type": "object",
            "properties": { "meeting_id": id },
            "required": ["meeting_id"],
            "additionalProperties": false
        })),
        tool("get_transcript", "The transcript of a meeting's latest call, a page at a time.", json!({
            "type": "object",
            "properties": { "meeting_id": id, "after_seq": { "type": "integer", "minimum": 0 }, "limit": { "type": "integer", "minimum": 1, "maximum": 2000, "default": 500 } },
            "required": ["meeting_id"],
            "additionalProperties": false
        })),
        tool("leave_meeting", "Take Gizzi out of the call it is in.", json!({
            "type": "object",
            "properties": { "meeting_id": id },
            "required": ["meeting_id"],
            "additionalProperties": false
        })),
    ]
}

// ---------------------------------------------------------------------------
// Shapes (cloud-api JSON -> pack JSON), pure
// ---------------------------------------------------------------------------

fn s(v: &Value, k: &str) -> Value {
    v.get(k).cloned().unwrap_or(Value::Null)
}

fn page_url(id: &str) -> String {
    format!("{PAGE_BASE}/{id}")
}

/// Square brand mark per platform (the web app's icon files), an absolute URL so chat cards can draw it.
pub fn platform_icon(platform: &str) -> &'static str {
    match platform {
        "zoom" => "https://ai.allternit.com/app-icons/zoom.svg",
        "teams" => "https://ai.allternit.com/app-icons/microsoftteams.svg",
        _ => "https://ai.allternit.com/app-icons/googlemeet.svg",
    }
}

/// `meetings[]` row: `{id,title,platform,platform_icon,platform_label,people_label,gizzi,starts_at,page_url}`.
pub fn list_row(m: &Value) -> Value {
    let id = m["id"].as_str().unwrap_or_default();
    json!({
        "id": s(m, "id"),
        "title": s(m, "title"),
        "platform": m["platform"].as_str().unwrap_or("meet"),
        "platform_icon": platform_icon(m["platform"].as_str().unwrap_or("meet")),
        "platform_label": s(m, "platform_label"),
        "people_label": s(m, "people_label"),
        "gizzi": s(m, "gizzi"),
        "starts_at": s(m, "starts_at"),
        "page_url": m.get("page_url").cloned().unwrap_or_else(|| json!(page_url(id))),
    })
}

/// The consent text the bot posts in the call (CONTRACT section 1 and the Teams display-name change).
pub fn notice(platform: &str) -> String {
    let join = match platform {
        "teams" => "Gizzi joins as \"Gizzi - Allternit\" from the lobby; someone in the call lets it in. It tells everyone it is taking notes and answers in the meeting chat.".to_string(),
        "zoom" => "Gizzi joins as \"Gizzi (Allternit)\" while you are in the Zoom call (Zoom requires it). Connect Zoom once in Settings \u{203a} Meetings. It tells everyone it is taking notes and answers in the meeting chat.".to_string(),
        _ => "Gizzi joins as \"Gizzi (Allternit)\", tells everyone it is taking notes, and answers in the meeting chat.".to_string(),
    };
    format!("{join} The recap comes back to this chat.")
}

/// `get_meeting` / `add_meeting`: `{id,title,platform_label,people_label,gizzi,starts_at,join_url,notice}`.
pub fn meeting_detail(m: &Value) -> Value {
    let mut people = m["people_label"].as_str().unwrap_or_default().to_string();
    if !people.is_empty() && m["source"].as_str().is_some_and(|src| src.starts_with("calendar_")) {
        people.push_str(", from your calendar");
    }
    json!({
        "id": s(m, "id"),
        "title": s(m, "title"),
        "platform": m["platform"].as_str().unwrap_or("meet"),
        "platform_icon": platform_icon(m["platform"].as_str().unwrap_or("meet")),
        "platform_label": s(m, "platform_label"),
        "people_label": people,
        "gizzi": s(m, "gizzi"),
        "starts_at": s(m, "starts_at"),
        "join_url": s(m, "join_url"),
        "notice": notice(m["platform"].as_str().unwrap_or("meet")),
    })
}

fn len(v: &Value) -> usize {
    v.as_array().map_or(0, Vec::len)
}

fn minutes(session: &Value) -> Option<i64> {
    let parse = |k: &str| chrono::DateTime::parse_from_rfc3339(session[k].as_str()?).ok();
    let secs = (parse("ended_at")? - parse("live_at")?).num_seconds().max(0);
    Some(((secs + 59) / 60).max(1))
}

/// `get_recap`: `{id,title,platform_label,gizzi,starts_at,decisions,action_items,artifacts,page_url,summary}`;
/// the three lists are counts.
pub fn recap(meeting: &Value, session: &Value, artifacts: usize) -> Value {
    let id = meeting["id"].as_str().unwrap_or_default();
    let base = meeting["platform_label"].as_str().unwrap_or("Meeting");
    let platform_label = match minutes(session) {
        Some(n) if session["state"] == "ended" => format!("{base} · {n} min"),
        _ => base.to_string(),
    };
    json!({
        "id": s(meeting, "id"),
        "title": s(meeting, "title"),
        "platform": meeting["platform"].as_str().unwrap_or("meet"),
        "platform_icon": platform_icon(meeting["platform"].as_str().unwrap_or("meet")),
        "platform_label": platform_label,
        "gizzi": s(session, "gizzi"),
        "starts_at": s(meeting, "starts_at"),
        "decisions": len(&session["decisions"]),
        "action_items": len(&session["action_items"]),
        "artifacts": artifacts,
        "page_url": page_url(id),
        "summary": s(session, "summary"),
    })
}

/// `get_transcript`: `{id, segments:[{seq,speaker,start_ms,text}], next_after_seq}`.
pub fn transcript(meeting_id: &str, page: &Value) -> Value {
    let segments: Vec<Value> = page["segments"]
        .as_array()
        .map(|a| a.iter().map(|g| json!({ "seq": s(g, "seq"), "speaker": s(g, "speaker"), "start_ms": s(g, "start_ms"), "text": s(g, "text") })).collect())
        .unwrap_or_default();
    json!({ "id": meeting_id, "segments": segments, "next_after_seq": s(page, "next_after_seq") })
}

/// Where a person connects Zoom once; chat shows it as a link.
pub const SETTINGS_URL: &str = "https://ai.allternit.com/settings/meetings";

/// `409 zoom_not_connected`: tells the model what to say and where the link is.
fn zoom_not_connected() -> Value {
    json!({
        "error": "zoom_not_connected",
        "message": "Connect Zoom once in Settings \u{203a} Meetings, then ask again. Zoom only lets Gizzi in while you are in the call.",
        "settings_url": SETTINGS_URL,
    })
}

fn err(code: &str, message: &str) -> Value {
    json!({ "error": code, "message": message })
}

/// Lowercase, no query, fragment or trailing slash (to compare two call links).
fn normalise_link(url: &str) -> String {
    let u = url.trim().to_lowercase();
    let u = u.split(['?', '#']).next().unwrap_or("");
    u.trim_end_matches('/').to_string()
}

fn supported_link(url: &str) -> bool {
    let u = url.trim().to_lowercase();
    let host = u.strip_prefix("https://").and_then(|r| r.split('/').next()).unwrap_or("");
    host == "meet.google.com" || host == "zoom.us" || host.ends_with(".zoom.us") || host.ends_with(".zoom.com") || host == "zoom.com" || host == "teams.microsoft.com" || host == "teams.live.com"
}

// ---------------------------------------------------------------------------
// cloud-api client
// ---------------------------------------------------------------------------

pub struct Cloud<'a> {
    pub base: &'a str,
    pub token: &'a str,
}

impl Cloud<'_> {
    async fn call(&self, method: reqwest::Method, path: &str, query: &[(&str, String)], body: Option<Value>) -> Result<Value, Value> {
        let url = format!("{}/api/v1{path}", self.base.trim_end_matches('/'));
        let mut req = crate::net_client::client().request(method, &url).bearer_auth(self.token).timeout(Duration::from_secs(20)).query(query);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await.map_err(|e| err("cloud_unreachable", &format!("Allternit Cloud could not be reached: {e}")))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status.is_success() {
            return Ok(value);
        }
        // Pass cloud-api's `{error, message}` through (`402 meetings_requires_paid_plan` included).
        let code = value["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("cloud_{}", status.as_u16()));
        if code == "zoom_not_connected" {
            return Err(zoom_not_connected());
        }
        let message = value["message"].as_str().unwrap_or("Allternit Cloud refused the request.");
        Err(err(&code, message))
    }

    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value, Value> {
        self.call(reqwest::Method::GET, path, query, None).await
    }

    async fn find_by_link(&self, url: &str) -> Result<Value, Value> {
        if !supported_link(url) {
            return Err(err("unsupported_link", "That isn't a Google Meet, Zoom or Microsoft Teams meeting link."));
        }
        let want = normalise_link(url);
        for range in ["now", "today", "upcoming", "past"] {
            let page = self.get("/meetings", &[("range", range.into()), ("limit", "100".into())]).await?;
            if let Some(found) = page["meetings"].as_array().and_then(|a| a.iter().find(|m| normalise_link(m["join_url"].as_str().unwrap_or("")) == want)) {
                return Ok(found.clone());
            }
        }
        Err(err("meeting_not_found", "No meeting with that link is in your list. Use add_meeting to add it."))
    }

    async fn meeting(&self, args: &Map<String, Value>) -> Result<Value, Value> {
        if let Some(id) = args.get("meeting_id").and_then(Value::as_str) {
            return self.get(&format!("/meetings/{id}"), &[]).await;
        }
        match args.get("url").and_then(Value::as_str) {
            Some(url) => self.find_by_link(url).await,
            None => Err(err("invalid_request", "Give a meeting_id or a url.")),
        }
    }

    pub async fn run(&self, name: &str, args: &Value) -> Result<Value, Value> {
        let tool = name.strip_prefix(PREFIX).unwrap_or(name);
        let empty = Map::new();
        let a = args.as_object().unwrap_or(&empty);
        let id_arg = || a.get("meeting_id").and_then(Value::as_str).map(str::to_string).ok_or_else(|| err("invalid_request", "meeting_id is required."));
        match tool {
            "list_meetings" => {
                let range = a.get("range").and_then(Value::as_str).unwrap_or("today");
                if !["now", "today", "week", "past"].contains(&range) {
                    return Err(err("invalid_request", "range must be now, today, week or past."));
                }
                let page = self.get("/meetings", &[("range", range.into()), ("limit", "50".into())]).await?;
                let rows: Vec<Value> = page["meetings"].as_array().map(|m| m.iter().map(list_row).collect()).unwrap_or_default();
                Ok(json!({ "meetings": rows }))
            }
            "get_meeting" => Ok(meeting_detail(&self.meeting(a).await?)),
            "add_meeting" => {
                let url = a.get("url").and_then(Value::as_str).ok_or_else(|| err("invalid_request", "url is required."))?;
                let mut body = json!({ "url": url });
                for k in ["title", "starts_at"] {
                    if let Some(v) = a.get(k).filter(|v| v.is_string()) {
                        body[k] = v.clone();
                    }
                }
                Ok(meeting_detail(&self.call(reqwest::Method::POST, "/meetings", &[], Some(body)).await?))
            }
            "join_meeting" => {
                let id = id_arg()?;
                let when = a.get("when").and_then(Value::as_str).filter(|w| ["now", "start"].contains(w)).ok_or_else(|| err("invalid_request", "when must be now or start."))?;
                let session = self.call(reqwest::Method::POST, &format!("/meetings/{id}/sessions"), &[], Some(json!({ "when": when }))).await?;
                Ok(json!({ "id": id, "gizzi": s(&session, "gizzi"), "session_id": s(&session, "id"), "page_url": page_url(&id) }))
            }
            "set_auto_join" => {
                let id = id_arg()?;
                let on = a.get("on").and_then(Value::as_bool).ok_or_else(|| err("invalid_request", "on must be true or false."))?;
                let m = self.call(reqwest::Method::PATCH, &format!("/meetings/{id}"), &[], Some(json!({ "auto_join": on }))).await?;
                Ok(json!({ "id": id, "gizzi": s(&m, "gizzi"), "auto_join": s(&m, "auto_join") }))
            }
            "get_recap" => {
                let id = id_arg()?;
                let m = self.get(&format!("/meetings/{id}"), &[]).await?;
                let sid = m["latest_session"]["id"].as_str().ok_or_else(|| err("recap_not_ready", "Gizzi hasn't been in this meeting yet."))?;
                let detail = self.get(&format!("/meeting-sessions/{sid}"), &[]).await?;
                let session = &detail["session"];
                if session["state"] != "ended" {
                    return Err(err("recap_not_ready", "The meeting is still going; the recap comes when it ends."));
                }
                let artifacts = self.get(&format!("/meeting-sessions/{sid}/artifacts"), &[]).await.map(|v| len(&v["artifacts"])).unwrap_or(0);
                Ok(recap(&m, session, artifacts))
            }
            "get_transcript" => {
                let id = id_arg()?;
                let m = self.get(&format!("/meetings/{id}"), &[]).await?;
                let sid = m["latest_session"]["id"].as_str().ok_or_else(|| err("session_not_found", "Gizzi hasn't been in this meeting yet."))?;
                let mut q = vec![("limit", a.get("limit").and_then(Value::as_u64).unwrap_or(500).clamp(1, 2000).to_string())];
                if let Some(after) = a.get("after_seq").and_then(Value::as_u64) {
                    q.push(("after_seq", after.to_string()));
                }
                let page = self.get(&format!("/meeting-sessions/{sid}/transcript"), &q).await?;
                Ok(transcript(&id, &page))
            }
            "leave_meeting" => {
                let id = id_arg()?;
                let m = self.get(&format!("/meetings/{id}"), &[]).await?;
                let live = m["latest_session"].as_object().filter(|s| s.get("state").and_then(Value::as_str) != Some("ended"));
                let sid = live.and_then(|s| s.get("id")).and_then(Value::as_str).ok_or_else(|| err("session_not_active", "Gizzi isn't in this meeting."))?;
                let session = self.call(reqwest::Method::POST, &format!("/meeting-sessions/{sid}/control"), &[], Some(json!({ "action": "leave" }))).await?;
                Ok(json!({ "id": id, "gizzi": s(&session, "gizzi") }))
            }
            other => Err(err("unknown_tool", &format!("No meetings tool named {other}."))),
        }
    }
}

/// A `tools/call` for one of these tools, as a JSON-RPC response (MCP tool errors are `isError: true`).
pub async fn call_rpc(base: &str, token: Option<&str>, id: &Value, name: &str, args: &Value) -> Value {
    let out = match token {
        Some(token) => Cloud { base, token }.run(name, args).await,
        None => Err(err("not_paired", "This computer isn't signed in to Allternit Cloud, so Meetings can't be reached.")),
    };
    let (payload, is_error) = match out {
        Ok(v) => (v, false),
        Err(e) => (e, true),
    };
    json!({ "jsonrpc": "2.0", "id": id, "result": { "content": [{ "type": "text", "text": payload.to_string() }], "isError": is_error } })
}

/// Append the meetings tools to a `tools/list` response envelope.
pub fn extend_list(resp: &mut Value) {
    if let Some(list) = resp.get_mut("result").and_then(|r| r.get_mut("tools")).and_then(Value::as_array_mut) {
        list.extend(tools());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::Query, http::HeaderMap, routing::get, Json, Router};
    use std::collections::HashMap;

    fn keys(v: &Value) -> Vec<String> {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    }

    fn cloud_meeting() -> Value {
        json!({
            "id": "m-1", "user_id": "u", "platform": "zoom", "platform_label": "Zoom", "title": "Pricing sync",
            "join_url": "https://zoom.us/j/123456789", "starts_at": "2026-11-12T17:00:00.000Z", "source": "calendar_google",
            "gizzi": "ended", "people_label": "5 people", "page_url": "https://ai.allternit.com/meetings/m-1", "auto_join": null,
        })
    }

    /// The pack's `sample` blocks (when the web worktree is next to this one) fix the key sets.
    fn pack_keys(screen: &str) -> Option<Vec<String>> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let text = ["allternit-ai-meetings-finish", "allternit-ai-meetings"]
            .iter()
            .find_map(|d| std::fs::read_to_string(dir.join(format!("../../../{d}/src/lib/app-screens/packs/meetings.json"))).ok())?;
        let pack: Value = serde_json::from_str(&text).ok()?;
        let sample = pack["screens"].as_array()?.iter().find(|s| s["id"] == screen)?["sample"].clone();
        Some(keys(if screen == "upcoming" { &sample["meetings"][0] } else { &sample }))
    }

    #[test]
    fn meetings_mcp_output_shapes_match_the_pack() {
        let m = cloud_meeting();
        let list = list_row(&m);
        let want_list = vec!["gizzi", "id", "page_url", "people_label", "platform", "platform_icon", "platform_label", "starts_at", "title"];
        assert_eq!(keys(&list), want_list);
        let detail = meeting_detail(&m);
        let want_detail = vec!["gizzi", "id", "join_url", "notice", "people_label", "platform", "platform_icon", "platform_label", "starts_at", "title"];
        assert_eq!(keys(&detail), want_detail);
        assert_eq!(detail["people_label"], "5 people, from your calendar");
        let session = json!({ "state": "ended", "gizzi": "ended", "live_at": "2026-11-12T17:00:00.000Z", "ended_at": "2026-11-12T17:42:00.000Z",
            "summary": "Keep the free tier.", "decisions": ["a", "b", "c", "d"], "action_items": [{"text": "x"}, {"text": "y"}, {"text": "z"}] });
        let r = recap(&m, &session, 1);
        let want_recap = vec!["action_items", "artifacts", "decisions", "gizzi", "id", "page_url", "platform", "platform_icon", "platform_label", "starts_at", "summary", "title"];
        assert_eq!(keys(&r), want_recap);
        assert_eq!((r["decisions"].clone(), r["action_items"].clone(), r["artifacts"].clone()), (json!(4), json!(3), json!(1)), "counts, not lists");
        assert_eq!(r["platform_label"], "Zoom · 42 min");
        assert_eq!(r["platform_icon"], "https://ai.allternit.com/app-icons/zoom.svg");
        assert!(notice("zoom").contains("while you are in the Zoom call") && notice("teams").contains("\"Gizzi - Allternit\" from the lobby") && notice("meet").contains("\"Gizzi (Allternit)\""));
        for (screen, want) in [("upcoming", want_list), ("meeting", want_detail), ("recap", want_recap)] {
            if let Some(pack) = pack_keys(screen) {
                // The pack's sample may omit a field the tool also returns; every pack field must be there.
                assert!(pack.iter().all(|k| want.contains(&k.as_str())), "{screen}: pack {pack:?} vs tool {want:?}");
            }
        }
        let t = transcript("m-1", &json!({ "segments": [{ "seq": 1, "speaker": "Ann", "start_ms": 5, "end_ms": 9, "text": "hi", "confidence": 0.9 }], "next_after_seq": 1 }));
        assert_eq!(keys(&t["segments"][0]), vec!["seq", "speaker", "start_ms", "text"]);
        assert_eq!(keys(&t), vec!["id", "next_after_seq", "segments"]);
    }

    #[test]
    fn meetings_mcp_zoom_not_connected_points_to_settings() {
        let e = zoom_not_connected();
        assert_eq!(e["error"], "zoom_not_connected");
        assert!(e["message"].as_str().unwrap().contains("Connect Zoom once in Settings"));
        assert_eq!(e["settings_url"], "https://ai.allternit.com/settings/meetings");
    }

    #[test]
    fn meetings_mcp_lists_the_eight_tools_with_prefixed_names() {
        let names: Vec<String> = tools().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names.len(), 8);
        assert!(names.contains(&"meetings__list_meetings".to_string()));
        assert!(names.iter().all(|n| is_tool(n)));
        assert!(!is_tool("meetings__nope") && !is_tool("allternit_mail.send"));
    }

    /// A stand-in for cloud-api (the edge): checks the bearer, serves two meetings, refuses the free plan.
    async fn fake_cloud() -> String {
        async fn meetings(headers: HeaderMap, Query(q): Query<HashMap<String, String>>) -> (axum::http::StatusCode, Json<Value>) {
            if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer allternit_runtime_test") {
                return (axum::http::StatusCode::UNAUTHORIZED, Json(json!({ "error": "unauthorized" })));
            }
            if q.get("range").map(String::as_str) == Some("week") {
                return (axum::http::StatusCode::PAYMENT_REQUIRED, Json(json!({ "error": "meetings_requires_paid_plan", "message": "Meetings need a paid plan." })));
            }
            (axum::http::StatusCode::OK, Json(json!({ "meetings": [{
                "id": "m-1", "platform": "meet", "platform_label": "Google Meet", "title": "Standup", "join_url": "https://meet.google.com/abc-defg-hij?authuser=0",
                "starts_at": null, "gizzi": "scheduled", "people_label": "6 people", "source": "link", "extra": 1
            }], "next_cursor": null })))
        }
        let app = Router::new().route("/api/v1/meetings", get(meetings));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn meetings_mcp_calls_cloud_with_the_runtime_token_and_maps_errors() {
        let base = fake_cloud().await;
        let cloud = Cloud { base: &base, token: "allternit_runtime_test" };
        let out = cloud.run("meetings__list_meetings", &json!({})).await.unwrap();
        assert_eq!(keys(&out["meetings"][0]), vec!["gizzi", "id", "page_url", "people_label", "platform", "platform_icon", "platform_label", "starts_at", "title"]);
        assert_eq!(out["meetings"][0]["page_url"], "https://ai.allternit.com/meetings/m-1");
        // A link finds the meeting by its call URL, ignoring the query.
        let found = cloud.run("meetings__get_meeting", &json!({ "url": "https://meet.google.com/abc-defg-hij" })).await.unwrap();
        assert_eq!(found["id"], "m-1");
        assert!(found["notice"].as_str().unwrap().contains("Gizzi (Allternit)"));
        let bad = cloud.run("meetings__get_meeting", &json!({ "url": "https://example.com/x" })).await.unwrap_err();
        assert_eq!(bad["error"], "unsupported_link");
        // The free plan reaches the model as an MCP error.
        let rpc = call_rpc(&base, Some("allternit_runtime_test"), &json!(1), "meetings__list_meetings", &json!({ "range": "week" })).await;
        assert_eq!(rpc["result"]["isError"], true);
        assert!(rpc["result"]["content"][0]["text"].as_str().unwrap().contains("meetings_requires_paid_plan"));
        // No pairing, no call.
        let rpc = call_rpc(&base, None, &json!(2), "meetings__list_meetings", &json!({})).await;
        assert!(rpc["result"]["content"][0]["text"].as_str().unwrap().contains("not_paired"));
    }
}
