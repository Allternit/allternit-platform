//! Agent Preference API routes
//!
//! Per-user response-style preferences and profile (`/agent-preferences`).
//! The profile (Settings → Profile: name, what to call the user, their work,
//! personal preferences) is composed into every agent chat as an "About the
//! user" layer. On every
//! successful PUT the preferences are also synced into each of the user's
//! agent workspaces as a platform-managed `STYLE.md` (best-effort — sync
//! failures are logged but never fail the request).

use axum::{
    extract::{Extension, Json, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tracing::warn;

use crate::auth::AuthUser;
use crate::AppState;

const SUPPORTED_STYLES: [&str; 4] = ["concise", "balanced", "detailed", "custom"];

pub fn agent_preferences_router() -> Router<Arc<AppState>> {
    Router::new().route(
        "/agent-preferences",
        get(get_agent_preferences).put(set_agent_preferences),
    )
}

#[derive(Serialize, Default)]
struct AgentPreferencesPayload {
    response_style: String,
    custom_instructions: String,
    full_name: String,
    preferred_name: String,
    occupation: String,
    personal_preferences: String,
    updated_at: String,
}

/// Longest accepted profile field, in characters. Keeps the composed system
/// prompt bounded.
const PROFILE_FIELD_MAX: usize = 120;
const PERSONAL_PREFERENCES_MAX: usize = 4000;

/// The Settings → Profile fields.
#[derive(Clone, Default, Debug, PartialEq)]
pub(crate) struct UserProfile {
    pub full_name: String,
    pub preferred_name: String,
    pub occupation: String,
    pub personal_preferences: String,
}

/// "About the user" layer for agent-chat system instructions, or None when
/// the profile is empty.
pub(crate) fn user_profile_block(profile: &UserProfile) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    let full = profile.full_name.trim();
    let preferred = profile.preferred_name.trim();
    if !full.is_empty() {
        lines.push(format!("Name: {}", full));
    }
    if !preferred.is_empty() {
        lines.push(format!("Call them: {}", preferred));
    }
    let occupation = profile.occupation.trim();
    if !occupation.is_empty() {
        lines.push(format!("Their work: {}", occupation));
    }
    let prefs = profile.personal_preferences.trim();
    if !prefs.is_empty() {
        lines.push(format!("Their preferences:\n{}", prefs));
    }
    if lines.is_empty() {
        None
    } else {
        Some(format!("About the user:\n{}", lines.join("\n")))
    }
}

/// Load the caller's preferences row (None when they never saved one).
pub(crate) fn load_preferences_row(
    conn: &rusqlite::Connection,
    user_id: &str,
) -> rusqlite::Result<Option<(String, String, UserProfile)>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT response_style, custom_instructions, full_name, preferred_name,
                occupation, personal_preferences
         FROM user_agent_preferences WHERE user_id = ?1",
        params![user_id],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                UserProfile {
                    full_name: row.get(2)?,
                    preferred_name: row.get(3)?,
                    occupation: row.get(4)?,
                    personal_preferences: row.get(5)?,
                },
            ))
        },
    )
    .optional()
}

// ─── GET /agent-preferences ───────────────────────────────────────────────────

async fn get_agent_preferences(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let row = load_preferences_row(&conn, &user_id)?;
        let updated_at: Option<String> = conn
            .query_row(
                "SELECT updated_at FROM user_agent_preferences WHERE user_id = ?1",
                params![user_id],
                |row| row.get(0),
            )
            .ok();
        let (response_style, custom_instructions, profile) =
            row.unwrap_or(("balanced".to_string(), String::new(), UserProfile::default()));
        Ok::<_, rusqlite::Error>(AgentPreferencesPayload {
            response_style,
            custom_instructions,
            full_name: profile.full_name,
            preferred_name: profile.preferred_name,
            occupation: profile.occupation,
            personal_preferences: profile.personal_preferences,
            updated_at: updated_at.unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
        })
    })
    .await;

    match result {
        Ok(Ok(pref)) => Json(pref).into_response(),
        Ok(Err(e)) => {
            warn!("DB error reading agent preferences: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ─── PUT /agent-preferences ───────────────────────────────────────────────────

#[derive(Deserialize)]
struct SetAgentPreferencesBody {
    response_style: Option<String>,
    custom_instructions: Option<String>,
    full_name: Option<String>,
    preferred_name: Option<String>,
    occupation: Option<String>,
    personal_preferences: Option<String>,
}

fn too_long(field: &Option<String>, max: usize) -> bool {
    field.as_ref().map(|v| v.chars().count() > max).unwrap_or(false)
}

async fn set_agent_preferences(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<SetAgentPreferencesBody>,
) -> impl IntoResponse {
    let style = body.response_style.clone();
    if let Some(ref s) = style {
        if !SUPPORTED_STYLES.contains(&s.as_str()) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "invalid_response_style",
                    "supported_styles": SUPPORTED_STYLES,
                })),
            )
                .into_response();
        }
    }

    for (name, value, max) in [
        ("full_name", &body.full_name, PROFILE_FIELD_MAX),
        ("preferred_name", &body.preferred_name, PROFILE_FIELD_MAX),
        ("occupation", &body.occupation, PROFILE_FIELD_MAX),
        ("personal_preferences", &body.personal_preferences, PERSONAL_PREFERENCES_MAX),
    ] {
        if too_long(value, max) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "field_too_long", "field": name, "max_chars": max })),
            )
                .into_response();
        }
    }

    let db = state.db.clone();
    let user_id = user.user_id;
    let user_id_for_sync = user_id.clone();
    let style_for_sync = style.clone();
    let instructions_for_sync = body.custom_instructions.clone();

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;

        // Merge with the existing row so omitted fields keep their values.
        let (cur_style, cur_instructions, cur_profile) = load_preferences_row(&conn, &user_id)?
            .unwrap_or(("balanced".to_string(), String::new(), UserProfile::default()));

        let response_style = style.unwrap_or(cur_style);
        let custom_instructions = body.custom_instructions.unwrap_or(cur_instructions);
        let profile = UserProfile {
            full_name: body.full_name.map(|v| v.trim().to_string()).unwrap_or(cur_profile.full_name),
            preferred_name: body
                .preferred_name
                .map(|v| v.trim().to_string())
                .unwrap_or(cur_profile.preferred_name),
            occupation: body.occupation.map(|v| v.trim().to_string()).unwrap_or(cur_profile.occupation),
            personal_preferences: body
                .personal_preferences
                .unwrap_or(cur_profile.personal_preferences),
        };

        conn.execute(
            "INSERT INTO user_agent_preferences
                (user_id, response_style, custom_instructions, full_name, preferred_name,
                 occupation, personal_preferences)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(user_id) DO UPDATE SET
                response_style = excluded.response_style,
                custom_instructions = excluded.custom_instructions,
                full_name = excluded.full_name,
                preferred_name = excluded.preferred_name,
                occupation = excluded.occupation,
                personal_preferences = excluded.personal_preferences,
                updated_at = CURRENT_TIMESTAMP",
            params![
                user_id,
                response_style,
                custom_instructions,
                profile.full_name,
                profile.preferred_name,
                profile.occupation,
                profile.personal_preferences
            ],
        )?;

        let updated_at: String = conn.query_row(
            "SELECT updated_at FROM user_agent_preferences WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )?;

        Ok::<_, rusqlite::Error>(AgentPreferencesPayload {
            response_style,
            custom_instructions,
            full_name: profile.full_name,
            preferred_name: profile.preferred_name,
            occupation: profile.occupation,
            personal_preferences: profile.personal_preferences,
            updated_at,
        })
    })
    .await;

    match result {
        Ok(Ok(pref)) => {
            // Best-effort STYLE.md sync into every agent workspace the user owns.
            let profile = UserProfile {
                full_name: pref.full_name.clone(),
                preferred_name: pref.preferred_name.clone(),
                occupation: pref.occupation.clone(),
                personal_preferences: pref.personal_preferences.clone(),
            };
            sync_style_md(
                state.db.clone(),
                user_id_for_sync,
                style_for_sync.unwrap_or_else(|| pref.response_style.clone()),
                instructions_for_sync.unwrap_or_else(|| pref.custom_instructions.clone()),
                profile,
            )
            .await;
            Json(pref).into_response()
        }
        Ok(Err(e)) => {
            warn!("DB error setting agent preferences: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ─── STYLE.md sync ────────────────────────────────────────────────────────────

/// One-line response-style directive written into the managed STYLE.md.
fn style_directive(style: &str) -> &'static str {
    match style {
        "concise" => {
            "Respond concisely. Keep answers short and direct; skip preamble and unnecessary elaboration."
        }
        "detailed" => {
            "Respond in detail. Explain your reasoning, include relevant context, and cover edge cases."
        }
        // "custom" has no directive of its own — the custom instructions carry it.
        "custom" => "",
        _ => "Respond with balanced detail: thorough enough to be useful, concise enough to stay readable.",
    }
}

/// Response-style directive injected into agent-chat system instructions at
/// send time. These strings must match what the iOS client injected before
/// composition moved server-side — change them in lockstep with the clients.
/// Returns None for styles without a directive ("balanced", "custom").
pub(crate) fn chat_style_directive(style: &str) -> Option<&'static str> {
    match style {
        "concise" => Some(
            "Response style: keep responses brief and to the point — no preamble, no recap, no filler.",
        ),
        "detailed" => Some(
            "Response style: give thorough, detailed responses — full context, reasoning, and examples where they help.",
        ),
        _ => None,
    }
}

/// Render the fully platform-managed STYLE.md content.
fn render_style_md(style: &str, custom_instructions: &str, profile: &UserProfile) -> String {
    let mut out = String::from(
        "<!-- Managed by Allternit response-style settings. \
         This file is regenerated on every settings change and will be \
         overwritten — do not hand-edit. -->\n\n# Response Style\n\n",
    );
    let directive = style_directive(style);
    if !directive.is_empty() {
        out.push_str(directive);
        out.push_str("\n\n");
    }
    if !custom_instructions.is_empty() {
        out.push_str("## Custom Instructions\n\n");
        out.push_str(custom_instructions);
        out.push('\n');
    }
    if let Some(block) = user_profile_block(profile) {
        out.push_str("\n## About the User\n\n");
        out.push_str(block.trim_start_matches("About the user:\n"));
        out.push('\n');
    }
    out
}

/// Write the managed STYLE.md into every agent workspace owned by the user.
/// Best-effort: any failure is logged and skipped, never propagated.
async fn sync_style_md(
    db: crate::db::DbHandle,
    user_id: String,
    style: String,
    custom_instructions: String,
    profile: UserProfile,
) {
    let result = tokio::task::spawn_blocking(move || {
        let agent_ids: Vec<String> = {
            let conn = db.connect()?;
            let mut stmt = conn.prepare("SELECT id FROM agents WHERE user_id = ?1")?;
            let ids = stmt
                .query_map(params![user_id], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids
        };

        let content = render_style_md(&style, &custom_instructions, &profile);

        for agent_id in agent_ids {
            let workspace_dir = crate::agent_workspace_paths::workspace_dir_for(&agent_id);
            if let Err(e) = std::fs::create_dir_all(&workspace_dir)
                .and_then(|_| std::fs::write(workspace_dir.join("STYLE.md"), &content))
            {
                warn!(
                    "STYLE.md sync failed for agent {}: {}",
                    agent_id, e
                );
            }
        }
        Ok::<_, rusqlite::Error>(())
    })
    .await;

    if let Err(e) = result {
        warn!("STYLE.md sync task panicked: {}", e);
    } else if let Ok(Err(e)) = result {
        warn!("STYLE.md sync DB error: {}", e);
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    #[test]
    fn empty_profile_has_no_block() {
        assert_eq!(user_profile_block(&UserProfile::default()), None);
        let blank = UserProfile { full_name: "  ".into(), ..Default::default() };
        assert_eq!(user_profile_block(&blank), None);
    }

    #[test]
    fn profile_block_lists_filled_fields_only() {
        let p = UserProfile {
            full_name: "Joseph Cartlidge".into(),
            preferred_name: "Eoj".into(),
            occupation: String::new(),
            personal_preferences: "Plain answers.".into(),
        };
        assert_eq!(
            user_profile_block(&p).as_deref(),
            Some("About the user:\nName: Joseph Cartlidge\nCall them: Eoj\nTheir preferences:\nPlain answers.")
        );
    }

    #[test]
    fn migration_adds_profile_columns_and_row_reads_them() {
        let db = crate::db::DbHandle::new_memory().expect("db");
        let conn = db.connect().expect("conn");
        assert_eq!(load_preferences_row(&conn, "u1").unwrap(), None);
        conn.execute(
            "INSERT INTO user_agent_preferences (user_id, response_style, custom_instructions, preferred_name, occupation)
             VALUES ('u1', 'concise', 'x', 'Eoj', 'Design')",
            [],
        )
        .unwrap();
        let (style, instr, profile) = load_preferences_row(&conn, "u1").unwrap().unwrap();
        assert_eq!((style.as_str(), instr.as_str()), ("concise", "x"));
        assert_eq!(profile.preferred_name, "Eoj");
        assert_eq!(profile.occupation, "Design");
        assert_eq!(profile.full_name, "");
    }

    #[test]
    fn style_md_includes_profile() {
        let p = UserProfile { preferred_name: "Eoj".into(), ..Default::default() };
        let md = render_style_md("balanced", "", &p);
        assert!(md.contains("## About the User\n\nCall them: Eoj"));
    }
}
