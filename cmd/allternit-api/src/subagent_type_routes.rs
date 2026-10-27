//! Subagent catalog — user-owned rows only.
//!
//! The 15 built-in subagent types live client-side and in gizzi. This API
//! stores just the caller's own rows: custom types (`is_custom = true`) and
//! overrides of a built-in keyed by the same slug (`is_custom = false`).
//! Deleting an override resets the built-in to its shipped defaults.

use axum::extract::Extension;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, patch},
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Map, Value};
use std::sync::Arc;
use tracing::warn;

use crate::auth::AuthUser;
use crate::AppState;

/// Slugs of the built-in subagent types (must match the client + gizzi lists).
/// planner, debugger, docs-writer and scribe were folded into the main loop as
/// gizzi skills (2026-10-27), so those slugs are free for custom types.
pub const BUILTIN_SUBAGENT_SLUGS: [&str; 11] = [
    "explorer",
    "builder",
    "code-reviewer",
    "verifier",
    "web-researcher",
    "browser-operator",
    "computer-operator",
    "data-analyst",
    "ui-designer",
    "release-engineer",
    "security-reviewer",
];

const MAX_PROMPT: usize = 20_000;
const MAX_NAME: usize = 60;
const MAX_DESCRIPTION: usize = 500;
const MODEL_CLASSES: [&str; 3] = ["small", "standard", "deep"];

pub fn subagent_type_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/subagent-types",
            get(list_subagent_types).post(create_subagent_type),
        )
        .route(
            "/subagent-types/:slug",
            patch(patch_subagent_type).delete(delete_subagent_type),
        )
}

// ─── Validation ───────────────────────────────────────────────────────────────

pub fn is_builtin_slug(slug: &str) -> bool {
    BUILTIN_SUBAGENT_SLUGS.contains(&slug)
}

/// `^[a-z0-9][a-z0-9-]{1,39}$`
pub fn valid_slug(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    if bytes.len() < 2 || bytes.len() > 40 {
        return false;
    }
    let ok = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    ok(bytes[0]) && bytes[1..].iter().all(|&b| ok(b) || b == b'-')
}

/// `^[a-z0-9-]{1,40}$` — look pack / face ids.
fn valid_look_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A field in a request body: absent (leave alone), null (clear), or a value.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Field<T> {
    #[default]
    Absent,
    Null,
    Set(T),
}

impl<T> Field<T> {
    fn is_absent(&self) -> bool {
        matches!(self, Field::Absent)
    }
}

/// Parsed + validated body fields. tools / look hold their JSON text.
#[derive(Debug, Default, PartialEq)]
pub struct Fields {
    pub name: Field<String>,
    pub description: Field<String>,
    pub prompt: Field<String>,
    pub model_class: Field<String>,
    pub tools: Field<String>,
    pub max_steps: Field<i64>,
    pub look: Field<String>,
}

fn text_field(body: &Map<String, Value>, key: &str, max: usize) -> Result<Field<String>, String> {
    match body.get(key) {
        None => Ok(Field::Absent),
        Some(Value::Null) => Ok(Field::Null),
        Some(Value::String(s)) => {
            if s.chars().count() > max {
                Err(format!("{key} must be at most {max} characters"))
            } else {
                Ok(Field::Set(s.clone()))
            }
        }
        Some(_) => Err(format!("{key} must be a string")),
    }
}

pub fn parse_fields(body: &Map<String, Value>) -> Result<Fields, String> {
    let name = text_field(body, "name", MAX_NAME)?;
    if let Field::Set(n) = &name {
        if n.trim().is_empty() {
            return Err("name must not be empty".into());
        }
    }
    let description = text_field(body, "description", MAX_DESCRIPTION)?;
    let prompt = text_field(body, "prompt", MAX_PROMPT)?;

    let model_class = text_field(body, "model_class", 16)?;
    if let Field::Set(m) = &model_class {
        if !MODEL_CLASSES.contains(&m.as_str()) {
            return Err("model_class must be one of small, standard, deep".into());
        }
    }

    let tools = match body.get("tools") {
        None => Field::Absent,
        Some(Value::Null) => Field::Null,
        Some(Value::Array(items)) => {
            if items.len() > 200 {
                return Err("tools may list at most 200 entries".into());
            }
            for item in items {
                match item.as_str() {
                    Some(s) if !s.is_empty() && s.len() <= 120 => {}
                    _ => return Err("tools must be an array of non-empty strings".into()),
                }
            }
            Field::Set(Value::Array(items.clone()).to_string())
        }
        Some(_) => return Err("tools must be an array of strings".into()),
    };

    let max_steps = match body.get("max_steps") {
        None => Field::Absent,
        Some(Value::Null) => Field::Null,
        Some(v) => match v.as_i64() {
            Some(n) if (1..=1000).contains(&n) => Field::Set(n),
            _ => return Err("max_steps must be an integer between 1 and 1000".into()),
        },
    };

    let look = match body.get("look") {
        None => Field::Absent,
        Some(Value::Null) => Field::Null,
        Some(Value::Object(o)) => {
            let pack = o.get("pack").and_then(Value::as_str).unwrap_or("");
            let face = o.get("face").and_then(Value::as_str).unwrap_or("");
            if !valid_look_id(pack) || !valid_look_id(face) {
                return Err("look.pack and look.face must match ^[a-z0-9-]{1,40}$".into());
            }
            let mut clean = Map::new();
            clean.insert("pack".into(), json!(pack));
            clean.insert("face".into(), json!(face));
            match o.get("accent") {
                None | Some(Value::Null) => {}
                Some(Value::String(a)) if a.len() <= 40 => {
                    clean.insert("accent".into(), json!(a));
                }
                Some(_) => return Err("look.accent must be a short string".into()),
            }
            Field::Set(Value::Object(clean).to_string())
        }
        Some(_) => return Err("look must be an object {pack, face, accent?}".into()),
    };

    Ok(Fields {
        name,
        description,
        prompt,
        model_class,
        tools,
        max_steps,
        look,
    })
}

// ─── Rows ─────────────────────────────────────────────────────────────────────

const SELECT: &str = "SELECT slug, name, description, prompt, model_class, tools, max_steps, look, is_custom \
                      FROM subagent_types";

fn json_text(s: Option<String>) -> Value {
    s.and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "slug": row.get::<_, String>(0)?,
        "name": row.get::<_, Option<String>>(1)?,
        "description": row.get::<_, Option<String>>(2)?,
        "prompt": row.get::<_, Option<String>>(3)?,
        "model_class": row.get::<_, Option<String>>(4)?,
        "tools": json_text(row.get(5)?),
        "max_steps": row.get::<_, Option<i64>>(6)?,
        "look": json_text(row.get(7)?),
        "is_custom": row.get::<_, i64>(8)? != 0,
    }))
}

fn get_row(conn: &rusqlite::Connection, user_id: &str, slug: &str) -> rusqlite::Result<Option<Value>> {
    conn.query_row(
        &format!("{SELECT} WHERE user_id = ?1 AND slug = ?2"),
        params![user_id, slug],
        read_row,
    )
    .optional()
}

/// Apply every non-absent field to the caller's row.
fn apply_fields(
    conn: &rusqlite::Connection,
    user_id: &str,
    slug: &str,
    f: &Fields,
) -> rusqlite::Result<()> {
    fn opt<T: Clone>(field: &Field<T>) -> Option<T> {
        match field {
            Field::Set(v) => Some(v.clone()),
            _ => None,
        }
    }
    let mut sets: Vec<&str> = Vec::new();
    let mut vals: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    macro_rules! push {
        ($field:expr, $col:literal) => {
            if !$field.is_absent() {
                sets.push(concat!($col, " = ?"));
                vals.push(Box::new(opt(&$field)));
            }
        };
    }
    push!(f.name, "name");
    push!(f.description, "description");
    push!(f.prompt, "prompt");
    push!(f.model_class, "model_class");
    push!(f.tools, "tools");
    push!(f.max_steps, "max_steps");
    push!(f.look, "look");
    if sets.is_empty() {
        return Ok(());
    }
    let sql = format!(
        "UPDATE subagent_types SET {}, updated_at = datetime('now') WHERE user_id = ? AND slug = ?",
        sets.join(", ")
    );
    vals.push(Box::new(user_id.to_string()));
    vals.push(Box::new(slug.to_string()));
    conn.execute(
        &sql,
        rusqlite::params_from_iter(vals.iter().map(|v| v.as_ref())),
    )?;
    Ok(())
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

enum Outcome {
    Ok(Value),
    Fail(StatusCode, String),
}

async fn run_db<F>(state: &Arc<AppState>, f: F) -> Outcome
where
    F: FnOnce(rusqlite::Connection) -> rusqlite::Result<Outcome> + Send + 'static,
{
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        f(conn)
    })
    .await
    {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(e)) => {
            warn!("subagent_types DB error: {}", e);
            Outcome::Fail(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
        Err(e) => {
            warn!("subagent_types DB task panicked: {}", e);
            Outcome::Fail(StatusCode::INTERNAL_SERVER_ERROR, "internal error".into())
        }
    }
}

fn respond(outcome: Outcome, ok: StatusCode) -> Response {
    match outcome {
        Outcome::Ok(v) => (ok, Json(v)).into_response(),
        Outcome::Fail(status, msg) => err(status, msg),
    }
}

async fn list_subagent_types(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    let user_id = user.user_id;
    let outcome = run_db(&state, move |conn| {
        let mut stmt = conn.prepare(&format!("{SELECT} WHERE user_id = ?1 ORDER BY slug"))?;
        let rows = stmt
            .query_map(params![user_id], read_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Outcome::Ok(json!({ "types": rows })))
    })
    .await;
    respond(outcome, StatusCode::OK)
}

async fn create_subagent_type(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<Value>,
) -> Response {
    let Some(map) = body.as_object() else {
        return err(StatusCode::BAD_REQUEST, "body must be a JSON object");
    };
    let slug = map.get("slug").and_then(Value::as_str).unwrap_or("").to_string();
    if !valid_slug(&slug) {
        return err(
            StatusCode::BAD_REQUEST,
            "slug must match ^[a-z0-9][a-z0-9-]{1,39}$",
        );
    }
    if is_builtin_slug(&slug) {
        return err(
            StatusCode::CONFLICT,
            "slug is a built-in subagent type; PATCH it to override",
        );
    }
    let fields = match parse_fields(map) {
        Ok(f) => f,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    if !matches!(fields.name, Field::Set(_)) {
        return err(StatusCode::BAD_REQUEST, "name is required");
    }
    let user_id = user.user_id;
    let outcome = run_db(&state, move |conn| {
        if get_row(&conn, &user_id, &slug)?.is_some() {
            return Ok(Outcome::Fail(
                StatusCode::CONFLICT,
                "a subagent type with this slug already exists".into(),
            ));
        }
        conn.execute(
            "INSERT INTO subagent_types (id, user_id, slug, is_custom) VALUES (?1, ?2, ?3, 1)",
            params![uuid::Uuid::new_v4().to_string(), user_id, slug],
        )?;
        apply_fields(&conn, &user_id, &slug, &fields)?;
        let row = get_row(&conn, &user_id, &slug)?.unwrap_or(Value::Null);
        Ok(Outcome::Ok(json!({ "type": row })))
    })
    .await;
    respond(outcome, StatusCode::CREATED)
}

async fn patch_subagent_type(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(slug): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let Some(map) = body.as_object() else {
        return err(StatusCode::BAD_REQUEST, "body must be a JSON object");
    };
    if !valid_slug(&slug) {
        return err(StatusCode::NOT_FOUND, "subagent type not found");
    }
    let fields = match parse_fields(map) {
        Ok(f) => f,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let user_id = user.user_id;
    let outcome = run_db(&state, move |conn| {
        let existing = get_row(&conn, &user_id, &slug)?;
        match existing {
            Some(row) => {
                // A custom type keeps a name; clearing it is not allowed.
                if row["is_custom"] == json!(true) && fields.name == Field::Null {
                    return Ok(Outcome::Fail(
                        StatusCode::BAD_REQUEST,
                        "a custom subagent type needs a name".into(),
                    ));
                }
            }
            None if is_builtin_slug(&slug) => {
                conn.execute(
                    "INSERT INTO subagent_types (id, user_id, slug, is_custom) VALUES (?1, ?2, ?3, 0)",
                    params![uuid::Uuid::new_v4().to_string(), user_id, slug],
                )?;
            }
            None => {
                return Ok(Outcome::Fail(
                    StatusCode::NOT_FOUND,
                    "subagent type not found".into(),
                ))
            }
        }
        apply_fields(&conn, &user_id, &slug, &fields)?;
        let row = get_row(&conn, &user_id, &slug)?.unwrap_or(Value::Null);
        Ok(Outcome::Ok(json!({ "type": row })))
    })
    .await;
    respond(outcome, StatusCode::OK)
}

async fn delete_subagent_type(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(slug): Path<String>,
) -> Response {
    let user_id = user.user_id;
    let outcome = run_db(&state, move |conn| {
        conn.execute(
            "DELETE FROM subagent_types WHERE user_id = ?1 AND slug = ?2",
            params![user_id, slug],
        )?;
        Ok(Outcome::Ok(Value::Null))
    })
    .await;
    match outcome {
        Outcome::Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Outcome::Fail(status, msg) => err(status, msg),
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::beta_session_routes::tests as beta_test;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn slug_rules() {
        assert!(valid_slug("my-agent"));
        assert!(valid_slug("a1"));
        assert!(!valid_slug("a"));
        assert!(!valid_slug("-lead"));
        assert!(!valid_slug("Upper"));
        assert!(!valid_slug("has space"));
        assert!(!valid_slug(&"a".repeat(41)));
        assert!(valid_slug(&"a".repeat(40)));
        assert!(is_builtin_slug("verifier"));
        assert!(!is_builtin_slug("scribe"));
        assert!(!is_builtin_slug("my-agent"));
    }

    #[test]
    fn field_validation() {
        assert!(parse_fields(&obj(json!({"model_class": "huge"}))).is_err());
        assert!(parse_fields(&obj(json!({"name": "x".repeat(61)}))).is_err());
        assert!(parse_fields(&obj(json!({"description": "x".repeat(501)}))).is_err());
        assert!(parse_fields(&obj(json!({"prompt": "x".repeat(20_001)}))).is_err());
        assert!(parse_fields(&obj(json!({"tools": ["read", 3]}))).is_err());
        assert!(parse_fields(&obj(json!({"look": {"pack": "Bad Pack", "face": "a"}}))).is_err());
        assert!(parse_fields(&obj(json!({"max_steps": 0}))).is_err());

        let f = parse_fields(&obj(json!({
            "name": "Lead",
            "model_class": "deep",
            "tools": ["read", "grep"],
            "look": {"pack": "dot-friends", "face": "happy", "accent": "#ff0"},
            "prompt": null
        })))
        .unwrap();
        assert_eq!(f.model_class, Field::Set("deep".into()));
        assert_eq!(f.tools, Field::Set(r#"["read","grep"]"#.into()));
        assert_eq!(f.prompt, Field::Null);
        assert_eq!(f.description, Field::Absent);
    }

    async fn call(
        router: &Router,
        method: &str,
        path: &str,
        body: Value,
        user: &str,
    ) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .extension(beta_test::test_user(user))
            .body(Body::from(body.to_string()))
            .unwrap();
        let res = router.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn create_rejects_builtin_and_duplicate() {
        let temp = beta_test::temp_dir("subagent-types-create");
        let state = beta_test::test_app_state(&temp).await;
        let router = subagent_type_router().with_state(state);

        let (s, _) = call(&router, "POST", "/subagent-types", json!({"slug": "explorer", "name": "P"}), "u1").await;
        assert_eq!(s, StatusCode::CONFLICT);

        let body = json!({"slug": "lead", "name": "Lead", "tools": ["read"], "look": {"pack": "dot", "face": "a"}});
        let (s, p) = call(&router, "POST", "/subagent-types", body.clone(), "u1").await;
        assert_eq!(s, StatusCode::CREATED);
        assert_eq!(p["type"]["is_custom"], true);
        assert_eq!(p["type"]["tools"], json!(["read"]));
        assert_eq!(p["type"]["look"]["pack"], "dot");
        assert_eq!(p["type"]["prompt"], Value::Null);

        let (s, _) = call(&router, "POST", "/subagent-types", body, "u1").await;
        assert_eq!(s, StatusCode::CONFLICT);

        // Rows are per user.
        let (_, p) = call(&router, "GET", "/subagent-types", json!({}), "u2").await;
        assert_eq!(p["types"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn patch_upserts_builtin_override_and_delete_resets() {
        let temp = beta_test::temp_dir("subagent-types-patch");
        let state = beta_test::test_app_state(&temp).await;
        let router = subagent_type_router().with_state(state);

        let (s, _) = call(&router, "PATCH", "/subagent-types/nope", json!({"model_class": "deep"}), "u1").await;
        assert_eq!(s, StatusCode::NOT_FOUND);

        let (s, p) = call(&router, "PATCH", "/subagent-types/explorer", json!({"model_class": "deep"}), "u1").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(p["type"]["is_custom"], false);
        assert_eq!(p["type"]["model_class"], "deep");
        assert_eq!(p["type"]["name"], Value::Null);

        // Partial update leaves other fields alone.
        let (_, p) = call(&router, "PATCH", "/subagent-types/explorer", json!({"max_steps": 12}), "u1").await;
        assert_eq!(p["type"]["model_class"], "deep");
        assert_eq!(p["type"]["max_steps"], 12);

        let (s, _) = call(&router, "DELETE", "/subagent-types/explorer", json!({}), "u1").await;
        assert_eq!(s, StatusCode::NO_CONTENT);
        let (_, p) = call(&router, "GET", "/subagent-types", json!({}), "u1").await;
        assert_eq!(p["types"].as_array().unwrap().len(), 0);
    }
}
