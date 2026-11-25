//! Cloud skills (lane 47b): the checks, then CRUD scoping and the MCP edge
//! store against a test schema with migration 162. No network.

use axum::http::StatusCode;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::routes::mcp_edge_skills::{resources, SkillCaller};
use crate::routes::test_support::{authed_request, test_state, MockGateway, DEV_USER};

const SKILL: &str = "---\nname: pdf-notes\ndescription: \"Turn a PDF into notes\"\n---\n\n# PDF notes\n\nRead the file first.\n";

fn md(name: &str) -> String {
    format!("---\nname: {name}\ndescription: Does {name}\n---\n\nBody of {name}\n")
}

// ----------------------------------------------------------- decisions ----

#[test]
fn skill_md_frontmatter_is_read_and_checked() {
    let (name, description, body) = parse_skill_md(SKILL).unwrap();
    assert_eq!((name.as_str(), description.as_str()), ("pdf-notes", "Turn a PDF into notes"));
    assert!(body.starts_with("# PDF notes"));
    assert!(parse_skill_md("# no frontmatter").is_err());
    assert!(parse_skill_md("---\nname: x\n---\nbody").is_err(), "description is required");
    assert!(parse_skill_md("---\ndescription: d\n---\nbody").is_err(), "name is required");
    assert!(parse_skill_md("---\nname: Bad Name\ndescription: d\n---\nbody").is_err());
    assert!(parse_skill_md("---\nname: ../x\ndescription: d\n---\nbody").is_err());
    assert!(parse_skill_md("---\nname: x\ndescription: d\nbody").is_err(), "unclosed block");
}

#[test]
fn size_count_and_paths_are_capped() {
    let mut files = BTreeMap::new();
    files.insert("references/a.md".to_string(), "ok".to_string());
    assert!(check_skill_size(SKILL, &files).is_ok());
    for bad in ["../x", "/abs", "a//b", "SKILL.md", ""] {
        let mut f = BTreeMap::new();
        f.insert(bad.to_string(), "x".to_string());
        assert!(check_skill_size(SKILL, &f).is_err(), "{bad}");
    }
    let mut big = BTreeMap::new();
    big.insert("references/big.md".to_string(), "x".repeat(MAX_TOTAL_BYTES));
    assert!(check_skill_size(SKILL, &big).is_err());
    let many: BTreeMap<String, String> = (0..=MAX_FILES).map(|i| (format!("f{i}.md"), "x".into())).collect();
    assert!(check_skill_size(SKILL, &many).is_err());
}

#[test]
fn org_members_read_and_only_admins_write() {
    let m = vec![
        OrgMembership { id: "org_a".into(), role: "org:admin".into() },
        OrgMembership { id: "org_b".into(), role: "org:member".into() },
    ];
    assert!(org_access(&m, "org_a", true) && org_access(&m, "org_a", false));
    assert!(org_access(&m, "org_b", false) && !org_access(&m, "org_b", true));
    assert!(!org_access(&m, "org_c", false), "not a member: not even a read");
}

// --------------------------------------------------------- with a db ----

async fn setup() -> Arc<ApiState> {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    let sql = include_str!("../../migrations_pg/162_cloud_skills.sql").replace("public.", "");
    sqlx::raw_sql(&sql).execute(&state.db).await.expect("migration");
    sqlx::raw_sql(&sql).execute(&state.db).await.expect("migration twice");
    sqlx::query("DELETE FROM cloud_skills").execute(&state.db).await.unwrap();
    state
}

fn dev_token_on() -> std::sync::MutexGuard<'static, ()> {
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    let g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
    g
}

async fn send(state: &Arc<ApiState>, method: &str, path: &str, body: &str) -> (StatusCode, Value) {
    let res = routes().with_state(state.clone()).oneshot(authed_request(method, path, body)).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn create_body(skill_md: &str) -> String {
    json!({ "skillMd": skill_md, "files": { "references/notes.md": "notes" } }).to_string()
}

/// A skill another user owns, written straight into the table.
async fn seed_other_users_skill(state: &Arc<ApiState>) -> String {
    let id = new_id("skl_");
    sqlx::query("INSERT INTO cloud_skills (id, owner_user_id, name, description, skill_md) VALUES ($1, 'someone-else', 'secret', 'Not yours', $2)")
        .bind(&id)
        .bind(md("secret"))
        .execute(&state.db)
        .await
        .unwrap();
    id
}

#[tokio::test]
#[serial_test::serial]
async fn cloud_skills_crud_is_scoped_to_the_owner() {
    let _g = dev_token_on();
    let state = setup().await;
    let (status, made) = send(&state, "POST", "/api/v1/skills", &create_body(SKILL)).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["name"], "pdf-notes");
    assert_eq!(made["version"], 1);
    assert!(made["id"].as_str().unwrap().starts_with("skl_"));
    let id = made["id"].as_str().unwrap().to_string();

    let (status, dup) = send(&state, "POST", "/api/v1/skills", &create_body(SKILL)).await;
    assert_eq!((status, dup["error"].as_str()), (StatusCode::CONFLICT, Some("name_taken")));
    let (status, bad) = send(&state, "POST", "/api/v1/skills", r#"{"skillMd":"no frontmatter"}"#).await;
    assert_eq!((status, bad["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_skill_md")));
    let (status, _) = send(&state, "POST", "/api/v1/skills", &json!({ "skillMd": SKILL, "files": { "../x": "y" } }).to_string()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, list) = send(&state, "GET", "/api/v1/skills", "").await;
    assert_eq!(list["skills"].as_array().unwrap().len(), 1);
    assert!(list["skills"][0].get("skillMd").is_none(), "the list is summaries");

    let (status, one) = send(&state, "GET", &format!("/api/v1/skills/{id}"), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(one["files"]["references/notes.md"], "notes");

    let (status, put) = send(&state, "PUT", &format!("/api/v1/skills/{id}"), &json!({ "skillMd": md("pdf-notes") }).to_string()).await;
    assert_eq!((status, put["version"].as_i64()), (StatusCode::OK, Some(2)), "{put}");
    assert_eq!(put["files"]["references/notes.md"], "notes", "files kept when not sent");

    // Another user's skill: 404 on read, change and delete, and not in the list.
    let other = seed_other_users_skill(&state).await;
    for (method, body) in [("GET", ""), ("PUT", r#"{"skillMd":"x"}"#), ("DELETE", "")] {
        let (status, body) = send(&state, method, &format!("/api/v1/skills/{other}"), body).await;
        assert_eq!((status, body["error"].as_str()), (StatusCode::NOT_FOUND, Some("not_found")), "{method}");
    }
    let (_, list) = send(&state, "GET", "/api/v1/skills", "").await;
    assert_eq!(list["skills"].as_array().unwrap().len(), 1);
    let still_there: i64 = sqlx::query_scalar("SELECT count(*) FROM cloud_skills WHERE id = $1").bind(&other).fetch_one(&state.db).await.unwrap();
    assert_eq!(still_there, 1);

    let (status, gone) = send(&state, "DELETE", &format!("/api/v1/skills/{id}"), "").await;
    assert_eq!((status, gone["removed"].as_bool()), (StatusCode::OK, Some(true)));
    let (status, _) = send(&state, "GET", &format!("/api/v1/skills/{id}"), "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[serial_test::serial]
async fn the_mcp_edge_lists_and_reads_a_stored_user_skill() {
    let _g = dev_token_on();
    let state = setup().await;
    let (status, _) = send(&state, "POST", "/api/v1/skills", &create_body(SKILL)).await;
    assert_eq!(status, StatusCode::CREATED);
    seed_other_users_skill(&state).await;

    let store = PgSkillStore { db: &state.db };
    let caller = SkillCaller::of(DEV_USER, "claude-ai");
    let list = resources(&store, &caller).await;
    let uris: Vec<&str> = list.iter().map(|r| r["uri"].as_str().unwrap()).collect();
    assert!(uris.contains(&"skill://user/pdf-notes/SKILL.md"), "{uris:?}");
    assert!(!uris.iter().any(|u| u.contains("secret")), "another user's skill never shows: {uris:?}");

    let reply = crate::routes::mcp_edge_skills::intercept_resources(&store, &caller, "resources/read", &json!(1), &json!({ "uri": "skill://user/pdf-notes/references/notes.md" }))
        .await
        .expect("answered at the edge");
    assert_eq!(reply["result"]["contents"][0]["text"], "notes");
    let skill_md = crate::routes::mcp_edge_skills::intercept_resources(&store, &caller, "resources/read", &json!(2), &json!({ "uri": "skill://user/pdf-notes/SKILL.md" }))
        .await
        .unwrap();
    let text = skill_md["result"]["contents"][0]["text"].as_str().unwrap();
    assert!(text.contains("name: pdf-notes") && text.contains("Read the file first."), "{text}");
}
