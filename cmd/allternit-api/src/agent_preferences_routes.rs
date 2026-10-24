//! Agent Preference API routes
//!
//! Per-user response-style preferences and profile (`/agent-preferences`).
//! The profile (Settings → Profile: name, what to call the user, their work,
//! personal preferences) is composed into every agent chat as an "About the
//! user" layer. On every
//! successful PUT the preferences are also synced into each of the user's
//! agent workspaces as a platform-managed `STYLE.md` (best-effort — sync
//! failures are logged but never fail the request).
//!
//! "Write in my style" (on by default): a short writing-style profile, built
//! without a model by `POST /agent-preferences/writing-style/learn` from
//! pasted samples and the user's own past messages, is composed into agent
//! chats as a "Writing as the user" layer so drafts the user sends as
//! themselves (emails, messages, posts) sound like them.

use axum::{
    extract::{Extension, Json, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
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
    Router::new()
        .route(
            "/agent-preferences",
            get(get_agent_preferences).put(set_agent_preferences),
        )
        .route(
            "/agent-preferences/writing-style/learn",
            post(learn_writing_style),
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
    write_in_my_style: bool,
    writing_style: String,
    writing_style_learned_at: Option<String>,
    updated_at: String,
}

impl AgentPreferencesPayload {
    fn profile(&self) -> UserProfile {
        UserProfile {
            full_name: self.full_name.clone(),
            preferred_name: self.preferred_name.clone(),
            occupation: self.occupation.clone(),
            personal_preferences: self.personal_preferences.clone(),
        }
    }

    fn writing(&self) -> WritingStyle {
        WritingStyle {
            enabled: self.write_in_my_style,
            profile: self.writing_style.clone(),
            learned_at: self.writing_style_learned_at.clone(),
        }
    }
}

/// Full GET payload for the caller (defaults when they never saved a row).
fn load_payload(
    conn: &rusqlite::Connection,
    user_id: &str,
) -> rusqlite::Result<AgentPreferencesPayload> {
    let row = load_preferences_row(conn, user_id)?;
    let updated_at: Option<String> = conn
        .query_row(
            "SELECT updated_at FROM user_agent_preferences WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )
        .ok();
    let writing = load_writing_style(conn, user_id)?;
    let (response_style, custom_instructions, profile) =
        row.unwrap_or(("balanced".to_string(), String::new(), UserProfile::default()));
    Ok(AgentPreferencesPayload {
        response_style,
        custom_instructions,
        full_name: profile.full_name,
        preferred_name: profile.preferred_name,
        occupation: profile.occupation,
        personal_preferences: profile.personal_preferences,
        write_in_my_style: writing.enabled,
        writing_style: writing.profile,
        writing_style_learned_at: writing.learned_at,
        updated_at: updated_at.unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
    })
}

/// Longest stored writing-style profile, in characters.
const WRITING_STYLE_MAX: usize = 4000;

/// The "Write in my style" setting. On by default; the profile is empty until
/// the user learns or writes one.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WritingStyle {
    pub enabled: bool,
    pub profile: String,
    pub learned_at: Option<String>,
}

impl Default for WritingStyle {
    fn default() -> Self {
        Self { enabled: true, profile: String::new(), learned_at: None }
    }
}

/// Load the caller's writing-style setting (defaults when no row).
pub(crate) fn load_writing_style(
    conn: &rusqlite::Connection,
    user_id: &str,
) -> rusqlite::Result<WritingStyle> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row(
            "SELECT write_in_my_style, writing_style, writing_style_learned_at
             FROM user_agent_preferences WHERE user_id = ?1",
            params![user_id],
            |row| {
                Ok(WritingStyle {
                    enabled: row.get::<_, i64>(0)? != 0,
                    profile: row.get(1)?,
                    learned_at: row.get(2)?,
                })
            },
        )
        .optional()?
        .unwrap_or_default())
}

/// Body of the "Writing as the user" layer (shared by agent-chat composition
/// and STYLE.md), or None when the setting is off or the profile is empty.
fn writing_style_body(writing: &WritingStyle) -> Option<String> {
    let profile = writing.profile.trim();
    if !writing.enabled || profile.is_empty() {
        return None;
    }
    Some(format!(
        "When you write something the user will send as themselves (emails, messages, posts, replies, drafts), write it the way they write. Their style:\n{}\nDon't use this style for your own replies to them.",
        profile
    ))
}

/// "Writing as the user" layer for agent-chat system instructions, composed
/// after "About the user". None when off or empty.
pub(crate) fn writing_style_block(writing: &WritingStyle) -> Option<String> {
    let body = writing_style_body(writing)?;
    let mut chars = body.chars();
    let first = chars.next().map(|c| c.to_ascii_lowercase()).unwrap_or_default();
    Some(format!("Writing as the user: {}{}", first, chars.as_str()))
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

    let result = tokio::task::spawn_blocking(move || -> rusqlite::Result<AgentPreferencesPayload> {
        let conn = db.connect()?;
        load_payload(&conn, &user_id)
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
    /// Omitted keeps the stored value (older clients never send it).
    write_in_my_style: Option<bool>,
    /// Hand-edited profile; omitted keeps the stored value, "" clears it.
    writing_style: Option<String>,
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
        ("writing_style", &body.writing_style.as_ref().map(|v| v.trim().to_string()), WRITING_STYLE_MAX),
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

    let result = tokio::task::spawn_blocking(move || -> rusqlite::Result<AgentPreferencesPayload> {
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

        apply_writing_style_update(&conn, &user_id, body.write_in_my_style, body.writing_style)?;

        load_payload(&conn, &user_id)
    })
    .await;

    match result {
        Ok(Ok(pref)) => {
            // Best-effort STYLE.md sync into every agent workspace the user owns.
            sync_payload_style_md(&state, user_id_for_sync, &pref).await;
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

/// Apply the optional "Write in my style" fields of a PUT to the (already
/// upserted) row. Omitted fields keep their stored values; clearing the
/// profile also clears when it was learned.
fn apply_writing_style_update(
    conn: &rusqlite::Connection,
    user_id: &str,
    enabled: Option<bool>,
    writing_style: Option<String>,
) -> rusqlite::Result<()> {
    if let Some(on) = enabled {
        conn.execute(
            "UPDATE user_agent_preferences SET write_in_my_style = ?2 WHERE user_id = ?1",
            params![user_id, on as i64],
        )?;
    }
    if let Some(text) = writing_style {
        let text = text.trim().to_string();
        if text.is_empty() {
            conn.execute(
                "UPDATE user_agent_preferences
                 SET writing_style = '', writing_style_learned_at = NULL WHERE user_id = ?1",
                params![user_id],
            )?;
        } else {
            conn.execute(
                "UPDATE user_agent_preferences SET writing_style = ?2 WHERE user_id = ?1",
                params![user_id, text],
            )?;
        }
    }
    Ok(())
}

/// STYLE.md sync from a saved payload.
async fn sync_payload_style_md(state: &AppState, user_id: String, pref: &AgentPreferencesPayload) {
    sync_style_md(
        state.db.clone(),
        user_id,
        pref.response_style.clone(),
        pref.custom_instructions.clone(),
        pref.profile(),
        pref.writing(),
    )
    .await;
}

// ─── POST /agent-preferences/writing-style/learn ─────────────────────────────

const LEARN_MAX_SAMPLES: usize = 10;
const LEARN_SAMPLE_MAX_CHARS: usize = 8000;
/// How many of the user's most recent own messages are read.
const LEARN_HISTORY_LIMIT: usize = 300;
const LEARN_MIN_CHARS: usize = 40;
const PROFILE_MAX_CHARS: usize = 2500;
const EXCERPT_MAX_CHARS: usize = 280;

#[derive(Deserialize, Default)]
struct LearnWritingStyleBody {
    #[serde(default)]
    samples: Option<Vec<String>>,
}

/// The user's own most recent messages, newest first: web/desktop
/// conversations (`conversation_messages` joined to `conversations.user_id`),
/// project chat with Al (`project_messages`) and Cowork Al transcripts
/// (`cowork_al_messages`), user role only.
fn load_user_messages(conn: &rusqlite::Connection, user_id: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT content FROM (
            SELECT m.content AS content, m.created_at AS created_at
              FROM conversation_messages m
              JOIN conversations c ON c.id = m.conversation_id
             WHERE c.user_id = ?1 AND m.role = 'user'
            UNION ALL
            SELECT text AS content, created_at FROM project_messages
             WHERE user_id = ?1 AND role = 'user'
            UNION ALL
            SELECT content, created_at FROM cowork_al_messages
             WHERE user_id = ?1 AND role = 'user'
         ) ORDER BY created_at DESC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![user_id, LEARN_HISTORY_LIMIT as i64], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// True when the text is prose the user wrote: long enough, not mostly code,
/// not just links, not a pasted log.
fn is_usable_prose(text: &str) -> bool {
    let text = text.trim();
    let chars = text.chars().count();
    if chars < LEARN_MIN_CHARS || text.contains("```") {
        return false;
    }
    let code_marks = text.chars().filter(|c| matches!(c, '{' | '}' | ';' | '<' | '>' | '=')).count();
    if code_marks * 25 > chars {
        return false;
    }
    let words: Vec<&str> = text
        .split_whitespace()
        .filter(|w| !w.contains("://") && !w.starts_with("www."))
        .filter(|w| w.chars().any(|c| c.is_alphabetic()))
        .collect();
    if words.len() < 6 {
        return false;
    }
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() >= 4 {
        let logish = lines
            .iter()
            .filter(|l| {
                let t = l.trim_start();
                let lower = t.to_ascii_lowercase();
                t.starts_with(|c: char| c.is_ascii_digit())
                    || t.starts_with('[')
                    || t.starts_with("at ")
                    || lower.starts_with("error")
                    || lower.starts_with("warn")
                    || lower.starts_with("info")
                    || lower.starts_with("debug")
                    || lower.contains("traceback")
                    || lower.contains("exception")
            })
            .count();
        if logish * 2 > lines.len() {
            return false;
        }
    }
    true
}

/// True for a word that looks like a credential: a known key prefix, or a
/// long run mixing letters and digits.
fn looks_like_secret(word: &str) -> bool {
    let w = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-');
    let lower = w.to_ascii_lowercase();
    for prefix in ["sk-", "sk_", "pk_", "rk_", "ghp_", "gho_", "github_pat_", "xox", "akia", "aiza", "glpat-", "eyj"] {
        if lower.starts_with(prefix) && w.len() >= 12 {
            return true;
        }
    }
    let has_alpha = w.chars().any(|c| c.is_ascii_alphabetic());
    let has_digit = w.chars().any(|c| c.is_ascii_digit());
    w.len() >= 20 && has_alpha && has_digit && w.chars().all(|c| c.is_ascii_alphanumeric() || "_-+/=".contains(c))
}

fn looks_like_email(word: &str) -> bool {
    let w = word.trim_matches(|c: char| !c.is_alphanumeric());
    match w.split_once('@') {
        Some((local, domain)) => !local.is_empty() && domain.contains('.'),
        None => false,
    }
}

/// Replace phone numbers (runs of digits and phone punctuation holding at
/// least 9 digits) with "[phone]".
fn redact_phones(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let starts_run = i == 0 || !chars[i - 1].is_alphanumeric();
        let run_char = chars[i].is_ascii_digit()
            || chars[i] == '('
            || (chars[i] == '+' && chars.get(i + 1).map_or(false, |c| c.is_ascii_digit()));
        if starts_run && run_char {
            let mut j = i;
            let mut digits = 0;
            while j < chars.len() && (chars[j].is_ascii_digit() || " ()+-.".contains(chars[j])) {
                if chars[j].is_ascii_digit() {
                    digits += 1;
                }
                j += 1;
            }
            if digits >= 9 {
                // Keep trailing spaces/punctuation outside the redaction.
                let mut end = j;
                while end > i && !chars[end - 1].is_ascii_digit() && chars[end - 1] != ')' {
                    end -= 1;
                }
                out.push_str("[phone]");
                out.extend(&chars[end..j]);
                i = j;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Strip emails, phone numbers and anything that looks like a key or
/// password from an excerpt. Whitespace is collapsed to single spaces.
fn redact_excerpt(text: &str) -> String {
    let text = redact_phones(text);
    let mut out: Vec<String> = Vec::new();
    let mut redact_next = false;
    for word in text.split_whitespace() {
        let lower = word.to_ascii_lowercase();
        if redact_next {
            out.push("[redacted]".into());
            redact_next = false;
            continue;
        }
        if looks_like_email(word) {
            out.push("[email]".into());
        } else if looks_like_secret(word) {
            out.push("[redacted]".into());
        } else if let Some((key, value)) = word.split_once([':', '=']) {
            let k = key.to_ascii_lowercase();
            if ["password", "passwd", "pwd", "token", "secret", "apikey", "api_key", "api-key"].contains(&k.as_str()) {
                out.push(format!("{}{}", &word[..key.len() + 1], if value.is_empty() { "" } else { " [redacted]" }));
                redact_next = value.is_empty();
            } else {
                out.push(word.to_string());
            }
        } else {
            out.push(word.to_string());
            redact_next = matches!(lower.as_str(), "password" | "passwd" | "password:" | "pw:" | "token:" | "secret:");
        }
    }
    out.join(" ")
}

/// Cut to at most `max` chars on a word boundary, adding "…" when cut.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max.saturating_sub(1)).collect();
    let cut = match cut.rfind(' ') {
        Some(idx) if idx > max / 2 => cut[..idx].to_string(),
        _ => cut,
    };
    format!("{}…", cut.trim_end())
}

fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x1F300..=0x1FAFF | 0x2600..=0x27BF | 0x1F000..=0x1F2FF)
}

fn habit(count: usize, total: usize) -> &'static str {
    if total == 0 || count == 0 {
        "never"
    } else if count * 3 >= total {
        "often"
    } else if count * 10 >= total {
        "sometimes"
    } else {
        "rarely"
    }
}

const GREETINGS: [&str; 8] = ["hi", "hey", "hello", "dear", "good morning", "good afternoon", "morning", "yo"];
const SIGN_OFFS: [&str; 10] = [
    "thanks", "thank you", "cheers", "best", "regards", "best regards", "kind regards", "talk soon", "thx", "ty",
];

/// Build a plain-words writing-style profile (markdown, at most 2500 chars)
/// from pasted samples and the user's own past messages, without a model.
/// Samples are preferred for the verbatim excerpts. None when there is not
/// enough usable prose.
fn build_writing_profile(samples: &[String], history: &[String]) -> Option<String> {
    let samples: Vec<&str> = samples.iter().map(|s| s.trim()).filter(|s| is_usable_prose(s)).collect();
    let history: Vec<&str> = history.iter().map(|s| s.trim()).filter(|s| is_usable_prose(s)).collect();
    let texts: Vec<&str> = samples.iter().chain(history.iter()).copied().collect();
    let total_words: usize = texts.iter().map(|t| t.split_whitespace().count()).sum();
    if texts.is_empty() || total_words < 20 {
        return None;
    }
    let n = texts.len();

    // Greeting and sign-off habits.
    let mut greeting_counts: Vec<(&str, usize)> = Vec::new();
    let mut sign_off_counts: Vec<(&str, usize)> = Vec::new();
    for text in &texts {
        let first = text.lines().next().unwrap_or("").trim().to_lowercase();
        if let Some(g) = GREETINGS.iter().copied().find(|g| {
            first.starts_with(*g)
                && first[g.len()..].chars().next().map_or(true, |c| !c.is_alphanumeric())
        }) {
            match greeting_counts.iter_mut().find(|e| e.0 == g) {
                Some(e) => e.1 += 1,
                None => greeting_counts.push((g, 1)),
            }
        }
        let last_lines: Vec<String> =
            text.lines().rev().filter(|l| !l.trim().is_empty()).take(2).map(|l| l.trim().to_lowercase()).collect();
        if let Some(s) = SIGN_OFFS.iter().rev().copied().find(|s| {
            last_lines.iter().any(|l| {
                l.starts_with(*s) && l.len() <= s.len() + 20
                    && l[s.len()..].chars().next().map_or(true, |c| !c.is_alphanumeric())
            })
        }) {
            match sign_off_counts.iter_mut().find(|e| e.0 == s) {
                Some(e) => e.1 += 1,
                None => sign_off_counts.push((s, 1)),
            }
        }
    }
    greeting_counts.sort_by(|a, b| b.1.cmp(&a.1));
    sign_off_counts.sort_by(|a, b| b.1.cmp(&a.1));

    // Sentences: length and capitalization.
    let mut sentence_words: Vec<usize> = Vec::new();
    let mut lower_starts = 0usize;
    let mut cased_starts = 0usize;
    let mut lowercase_i = 0usize;
    for text in &texts {
        for sentence in text.split(|c: char| matches!(c, '.' | '!' | '?' | '\n')) {
            let words: Vec<&str> = sentence.split_whitespace().collect();
            if words.len() < 2 {
                continue;
            }
            sentence_words.push(words.len());
            if let Some(c) = words[0].chars().find(|c| c.is_alphabetic()) {
                cased_starts += 1;
                if c.is_lowercase() {
                    lower_starts += 1;
                }
            }
            lowercase_i += words.iter().filter(|w| **w == "i" || w.starts_with("i'")).count();
        }
    }
    let avg = if sentence_words.is_empty() {
        0.0
    } else {
        sentence_words.iter().sum::<usize>() as f64 / sentence_words.len() as f64
    };
    let length = if avg < 11.0 {
        "short"
    } else if avg < 20.0 {
        "medium"
    } else {
        "long"
    };

    let count_texts = |pred: &dyn Fn(&str) -> bool| texts.iter().filter(|t| pred(**t)).count();
    let exclaims = count_texts(&|t: &str| t.contains('!'));
    let dashes = count_texts(&|t: &str| t.contains('—') || t.contains(" -- ") || t.contains(" – "));
    let ellipses = count_texts(&|t: &str| t.contains("...") || t.contains('…'));
    let emoji = count_texts(&|t: &str| t.chars().any(is_emoji));
    let contractions = count_texts(&|t: &str| {
        let chars: Vec<char> = t.chars().collect();
        chars.windows(3).any(|w| w[0].is_alphabetic() && (w[1] == '\'' || w[1] == '’') && w[2].is_alphabetic())
    });

    let mut out = String::from("## How they write\n\n");
    match greeting_counts.first() {
        Some((g, c)) if *c * 5 >= n || *c >= 2 => {
            out.push_str(&format!("- Greeting: often opens with \"{}\".\n", capitalize(g)))
        }
        _ => out.push_str("- Greeting: usually none; gets straight to the point.\n"),
    }
    match sign_off_counts.first() {
        Some((s, c)) if *c * 5 >= n || *c >= 2 => {
            out.push_str(&format!("- Sign-off: often ends with \"{}\".\n", capitalize(s)))
        }
        _ => out.push_str("- Sign-off: usually none.\n"),
    }
    out.push_str(&format!(
        "- Sentences: {} (about {} words on average).\n",
        length,
        avg.round() as usize
    ));
    let caps = if cased_starts > 0 && lower_starts * 10 >= cased_starts * 6 {
        "mostly lowercase, including sentence starts"
    } else if cased_starts > 0 && lower_starts * 4 >= cased_starts {
        "mixed; often starts sentences in lowercase"
    } else {
        "standard capitalization"
    };
    out.push_str(&format!("- Capitalization: {}", caps));
    if lowercase_i >= 2 {
        out.push_str(", writes \"i\" in lowercase");
    }
    out.push_str(".\n");
    out.push_str(&format!(
        "- Punctuation: exclamation marks {}, dashes {}, ellipses {}.\n",
        habit(exclaims, n),
        habit(dashes, n),
        habit(ellipses, n)
    ));
    let emoji_use = match habit(emoji, n) {
        "never" => "none",
        "often" => "often",
        _ => "some",
    };
    out.push_str(&format!("- Emoji: {}.\n", emoji_use));
    out.push_str(&format!(
        "- Contractions: {}.\n",
        match habit(contractions, n) {
            "often" => "uses them freely (don't, I'm, it's)",
            "never" => "avoids them",
            _ => "uses some",
        }
    ));

    // Verbatim excerpts, samples first.
    let mut excerpts: Vec<String> = Vec::new();
    for text in samples.iter().chain(history.iter()) {
        if excerpts.len() >= 5 {
            break;
        }
        let paragraph = text
            .split("\n\n")
            .map(str::trim)
            .find(|p| is_usable_prose(p))
            .unwrap_or(*text);
        let excerpt = clip(&redact_excerpt(paragraph), EXCERPT_MAX_CHARS);
        if excerpt.chars().count() >= 20 && !excerpts.contains(&excerpt) {
            excerpts.push(excerpt);
        }
    }
    let header = "\n## Examples of how they write\n\n";
    let mut profile = out.clone();
    if !excerpts.is_empty() {
        profile.push_str(header);
        for excerpt in &excerpts {
            let line = format!("> {}\n\n", excerpt);
            if profile.chars().count() + line.chars().count() > PROFILE_MAX_CHARS {
                break;
            }
            profile.push_str(&line);
        }
    }
    Some(clip(profile.trim_end(), PROFILE_MAX_CHARS))
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

async fn learn_writing_style(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: Option<Json<LearnWritingStyleBody>>,
) -> impl IntoResponse {
    let samples = body.and_then(|Json(b)| b.samples).unwrap_or_default();
    if samples.len() > LEARN_MAX_SAMPLES {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "too_many_samples", "max_samples": LEARN_MAX_SAMPLES })),
        )
            .into_response();
    }
    if samples.iter().any(|s| s.chars().count() > LEARN_SAMPLE_MAX_CHARS) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "field_too_long", "field": "samples", "max_chars": LEARN_SAMPLE_MAX_CHARS })),
        )
            .into_response();
    }

    let db = state.db.clone();
    let user_id = user.user_id;
    let user_id_for_sync = user_id.clone();
    let result = tokio::task::spawn_blocking(move || -> rusqlite::Result<Option<AgentPreferencesPayload>> {
        let conn = db.connect()?;
        let history = load_user_messages(&conn, &user_id)?;
        let Some(profile) = build_writing_profile(&samples, &history) else {
            return Ok(None);
        };
        save_learned_style(&conn, &user_id, &profile, &chrono::Utc::now().to_rfc3339())?;
        load_payload(&conn, &user_id).map(Some)
    })
    .await;

    match result {
        Ok(Ok(Some(pref))) => {
            sync_payload_style_md(&state, user_id_for_sync, &pref).await;
            Json(pref).into_response()
        }
        Ok(Ok(None)) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "not_enough_text" })),
        )
            .into_response(),
        Ok(Err(e)) => {
            warn!("DB error learning writing style: {}", e);
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

/// Save a learned profile (creating the row with defaults when the user never
/// saved preferences).
fn save_learned_style(
    conn: &rusqlite::Connection,
    user_id: &str,
    profile: &str,
    learned_at: &str,
) -> rusqlite::Result<()> {
    let profile = clip(profile.trim(), WRITING_STYLE_MAX);
    conn.execute(
        "INSERT INTO user_agent_preferences (user_id, writing_style, writing_style_learned_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(user_id) DO UPDATE SET
            writing_style = excluded.writing_style,
            writing_style_learned_at = excluded.writing_style_learned_at,
            updated_at = CURRENT_TIMESTAMP",
        params![user_id, profile, learned_at],
    )?;
    Ok(())
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
fn render_style_md(
    style: &str,
    custom_instructions: &str,
    profile: &UserProfile,
    writing: &WritingStyle,
) -> String {
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
    if let Some(body) = writing_style_body(writing) {
        out.push_str("\n## Writing as the User\n\n");
        out.push_str(&body);
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
    writing: WritingStyle,
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

        let content = render_style_md(&style, &custom_instructions, &profile, &writing);

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
        let md = render_style_md("balanced", "", &p, &WritingStyle::default());
        assert!(md.contains("## About the User\n\nCall them: Eoj"));
    }

    #[test]
    fn payload_defaults_write_in_my_style_on_with_no_row() {
        let db = crate::db::DbHandle::new_memory().expect("db");
        let conn = db.connect().expect("conn");
        let p = load_payload(&conn, "nobody").unwrap();
        assert!(p.write_in_my_style);
        assert_eq!(p.writing_style, "");
        assert_eq!(p.writing_style_learned_at, None);
        assert_eq!(p.response_style, "balanced");
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["write_in_my_style"], true);
        assert!(v["writing_style_learned_at"].is_null());
    }

    #[test]
    fn put_partial_body_keeps_writing_fields() {
        let db = crate::db::DbHandle::new_memory().expect("db");
        let conn = db.connect().expect("conn");
        save_learned_style(&conn, "u1", "- Sentences: short.", "2026-10-24T00:00:00+00:00").unwrap();
        apply_writing_style_update(&conn, "u1", Some(false), None).unwrap();
        // An older client's PUT omits both fields: the upsert it runs never
        // names the new columns, and the writing update is a no-op.
        conn.execute(
            "INSERT INTO user_agent_preferences (user_id, response_style) VALUES ('u1', 'concise')
             ON CONFLICT(user_id) DO UPDATE SET response_style = excluded.response_style",
            [],
        )
        .unwrap();
        apply_writing_style_update(&conn, "u1", None, None).unwrap();
        let p = load_payload(&conn, "u1").unwrap();
        assert_eq!(p.response_style, "concise");
        assert!(!p.write_in_my_style);
        assert_eq!(p.writing_style, "- Sentences: short.");
        assert_eq!(p.writing_style_learned_at.as_deref(), Some("2026-10-24T00:00:00+00:00"));

        apply_writing_style_update(&conn, "u1", Some(true), Some("  ".into())).unwrap();
        let p = load_payload(&conn, "u1").unwrap();
        assert!(p.write_in_my_style);
        assert_eq!(p.writing_style, "");
        assert_eq!(p.writing_style_learned_at, None);
    }

    #[test]
    fn learn_profile_from_samples_redacts_excerpts() {
        let samples = vec![
            "hey sam,\n\nquick one — can you send the deck to jo@example.com before friday? i'm out after that. the key is sk-live-abcdef1234567890XYZ\n\nthanks".to_string(),
            "hi team, we're moving the call to 3pm... don't worry about prep, it's a short one!".to_string(),
        ];
        let profile = build_writing_profile(&samples, &[]).expect("profile");
        assert!(profile.chars().count() <= PROFILE_MAX_CHARS);
        assert!(profile.contains("## How they write"));
        assert!(profile.contains("Sentences:"));
        assert!(profile.contains("Capitalization: mostly lowercase"));
        assert!(profile.contains("## Examples of how they write"));
        assert!(profile.contains("[email]"));
        assert!(!profile.contains("jo@example.com"));
        assert!(!profile.contains("sk-live-abcdef1234567890XYZ"));
        assert!(redact_excerpt("call me on +1 (555) 123-4567 ok").contains("[phone]"));
        assert_eq!(redact_excerpt("password: hunter2 please"), "password: [redacted] please");
    }

    #[test]
    fn learn_needs_enough_prose() {
        let junk = vec![
            "ok".to_string(),
            "```rust\nfn main() { println!(\"hi\"); }\n```".to_string(),
            "https://example.com/a https://example.com/b".to_string(),
            "2026-10-24 ERROR boom\n2026-10-24 ERROR boom\n2026-10-24 WARN x\n2026-10-24 INFO y".to_string(),
        ];
        assert_eq!(build_writing_profile(&junk, &junk), None);
        assert_eq!(build_writing_profile(&[], &[]), None);
    }

    #[test]
    fn learn_reads_only_the_users_own_messages() {
        let db = crate::db::DbHandle::new_memory().expect("db");
        let conn = db.connect().expect("conn");
        conn.execute_batch(
            "INSERT INTO conversations (id, user_id) VALUES ('c1', 'u1'), ('c2', 'u2');
             INSERT INTO conversation_messages (id, conversation_id, role, content)
               VALUES ('m1', 'c1', 'user', 'mine'), ('m2', 'c1', 'assistant', 'model'),
                      ('m3', 'c2', 'user', 'theirs');",
        )
        .unwrap();
        assert_eq!(load_user_messages(&conn, "u1").unwrap(), vec!["mine".to_string()]);
    }

    #[test]
    fn style_md_includes_writing_style_when_on() {
        let on = WritingStyle { enabled: true, profile: "- Emoji: none.".into(), learned_at: None };
        let md = render_style_md("balanced", "", &UserProfile::default(), &on);
        assert!(md.contains("\n## Writing as the User\n\nWhen you write something the user will send as themselves"));
        assert!(md.contains("Their style:\n- Emoji: none.\nDon't use this style for your own replies to them."));
        let off = WritingStyle { enabled: false, ..on };
        assert!(!render_style_md("balanced", "", &UserProfile::default(), &off).contains("Writing as the User"));
    }
}
