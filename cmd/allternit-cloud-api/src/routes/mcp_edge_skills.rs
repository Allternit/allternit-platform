//! MCP edge: Agent Skills over MCP (SEP-2640, plan v3 lane 47).
//!
//! mcp.allternit.com serves skills as `skill://` resources so a client loads a
//! skill only when it needs it and always gets the copy the server has now.
//!
//! URIs (`skill://<skill-path>/<file-path>`, the last skill-path segment is the
//! skill's frontmatter `name`):
//! - `skill://allternit/<name>/SKILL.md`: skills that ship with Allternit
//!   (Gizzi's bundled skills, embedded below). Every caller sees these.
//! - `skill://user/<name>/SKILL.md`: the caller's own skills. The URI carries
//!   no user id: it is always resolved against the signed-in caller, so one
//!   user can never name, list or read another user's skill.
//! - `skill://org/<org_id>/<name>/SKILL.md`: skills of an org the caller is a
//!   member of. An org the caller is not in answers "not found".
//!
//! Supporting files are readable at `skill://<skill-path>/<file-path>`. Only
//! `SKILL.md` goes into `resources/list`: gizzi (`runtime/skills/mcpSkills.ts`)
//! turns every listed `skill://` URI into a skill command, so a listed
//! `references/FORMS.md` would show up as a skill called "FORMS".
//!
//! `resources/read` of a `skill://` URI is answered here. `resources/list` is
//! still relayed (the runtime lists its own `ui://` app resources) and the
//! caller's skills are added to the first page of the answer.

use std::collections::HashMap;

use serde_json::{json, Value};

/// The SEP-2640 extension key servers advertise under `capabilities.extensions`.
pub const EXTENSION: &str = "io.modelcontextprotocol/skills";
const SCHEME: &str = "skill://";
const SKILL_FILE: &str = "SKILL.md";
const MARKDOWN: &str = "text/markdown";
/// MCP "Resource not found".
const RESOURCE_NOT_FOUND: i64 = -32002;
const METHOD_NOT_FOUND: i64 = -32601;

/// One skill: its frontmatter `name` and `description`, the SKILL.md body
/// (without frontmatter) and any supporting files.
#[derive(Clone, Debug, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub files: Vec<SkillFile>,
}

/// A supporting file, `path` relative to the skill root (`references/x.md`).
#[derive(Clone, Debug, PartialEq)]
pub struct SkillFile {
    pub path: String,
    pub mime_type: String,
    pub text: String,
}

/// Where user and org skills come from.
#[async_trait::async_trait]
pub trait SkillStore: Send + Sync {
    /// The skills `user_id` owns.
    async fn user_skills(&self, user_id: &str) -> Result<Vec<Skill>, String>;
    /// The skills of every org `user_id` is a member of, keyed by org id.
    /// Must return only orgs the user belongs to: the edge trusts this list.
    async fn org_skills(&self, user_id: &str) -> Result<HashMap<String, Vec<Skill>>, String>;
}

/// The store for backends with no database (the default `EdgeBackend::skills`).
/// Production uses `cloud_skills::PgSkillStore` (migration 163).
pub struct NoCloudStore;

#[async_trait::async_trait]
impl SkillStore for NoCloudStore {
    async fn user_skills(&self, _user_id: &str) -> Result<Vec<Skill>, String> {
        Ok(Vec::new())
    }
    async fn org_skills(&self, _user_id: &str) -> Result<HashMap<String, Vec<Skill>>, String> {
        Ok(HashMap::new())
    }
}

/// Who is asking. `personal` is false for a Platform API project key: it acts
/// for a project, not a person, so it only gets the built-in skills.
pub struct SkillCaller<'a> {
    pub user_id: &'a str,
    pub personal: bool,
}

impl<'a> SkillCaller<'a> {
    /// From the edge caller's `user_id` and `client` label.
    pub fn of(user_id: &'a str, client: &str) -> Self {
        Self { user_id, personal: !user_id.is_empty() && !client.starts_with("platform-key:") }
    }
}

/// Gizzi's bundled skills, embedded from gizzi-code so both ship the same text.
fn builtin() -> Vec<Skill> {
    const BUNDLED: &[(&str, &str, &str)] = &[
        (
            "planning",
            "Plan a non-trivial change in your own context: read the code first, then a numbered step plan with files, checks, rejected options and risks. Load before multi-file or hard-to-redo work.",
            include_str!("../../../gizzi-code/src/runtime/skills/bundled/planning.md"),
        ),
        (
            "debugging",
            "Find and fix the root cause of a bug, error, failing test or odd log from the evidence: hypotheses, file:line cause, smallest fix, the same fix in sibling code, proof it works.",
            include_str!("../../../gizzi-code/src/runtime/skills/bundled/debugging.md"),
        ),
        (
            "docs-writing",
            "Write or update user and developer docs in the same change as the feature: match the existing pages, runnable examples, nav updated, docs checks run.",
            include_str!("../../../gizzi-code/src/runtime/skills/bundled/docs-writing.md"),
        ),
        (
            "summarizing",
            "Write summaries, notes and handoffs from transcripts, logs, diffs or docs that keep every decision, open item, owner and file path.",
            include_str!("../../../gizzi-code/src/runtime/skills/bundled/summarizing.md"),
        ),
    ];
    BUNDLED
        .iter()
        .map(|(name, description, body)| Skill { name: (*name).into(), description: (*description).into(), body: (*body).into(), files: Vec::new() })
        .collect()
}

/// Agent Skills names: 1-64 of `a-z`, `0-9`, `-`, not starting or ending with `-`.
pub(crate) fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// A supporting-file path: relative, no empty, `.` or `..` segments.
pub(crate) fn valid_file_path(path: &str) -> bool {
    !path.is_empty() && !path.starts_with('/') && path.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
}

/// One skill the caller may see, with its URI root (`skill://user/notes`).
struct Visible {
    root: String,
    skill: Skill,
}

/// Every skill the caller may see. Store errors drop that scope (logged), never
/// the built-ins. Skills with names unsafe for a URI are skipped.
async fn visible(store: &dyn SkillStore, caller: &SkillCaller<'_>) -> Vec<Visible> {
    let mut out: Vec<Visible> = builtin().into_iter().map(|skill| Visible { root: format!("{SCHEME}allternit/{}", skill.name), skill }).collect();
    if !caller.personal {
        return out;
    }
    match store.user_skills(caller.user_id).await {
        Ok(skills) => out.extend(skills.into_iter().filter(|s| valid_name(&s.name)).map(|skill| Visible { root: format!("{SCHEME}user/{}", skill.name), skill })),
        Err(e) => tracing::warn!(error = %e, "mcp edge: user skills unavailable"),
    }
    match store.org_skills(caller.user_id).await {
        Ok(orgs) => {
            let mut orgs: Vec<_> = orgs.into_iter().filter(|(org, _)| valid_file_path(org) && !org.contains('/')).collect();
            orgs.sort_by(|a, b| a.0.cmp(&b.0));
            for (org, skills) in orgs {
                out.extend(skills.into_iter().filter(|s| valid_name(&s.name)).map(|skill| Visible { root: format!("{SCHEME}org/{org}/{}", skill.name), skill }));
            }
        }
        Err(e) => tracing::warn!(error = %e, "mcp edge: org skills unavailable"),
    }
    out
}

/// SKILL.md as served: frontmatter, then the body.
fn skill_md(skill: &Skill) -> String {
    let description = skill.description.replace(['\r', '\n'], " ");
    format!("---\nname: {}\ndescription: {}\n---\n\n{}", skill.name, serde_json::to_string(&description).unwrap_or_default(), skill.body)
}

/// The `resources/list` entries for the caller's skills.
pub async fn resources(store: &dyn SkillStore, caller: &SkillCaller<'_>) -> Vec<Value> {
    visible(store, caller)
        .await
        .into_iter()
        .map(|v| {
            json!({
                "uri": format!("{}/{SKILL_FILE}", v.root),
                "name": v.skill.name,
                "description": v.skill.description,
                "mimeType": MARKDOWN,
            })
        })
        .collect()
}

/// The text of `uri` if the caller may read it: `(mimeType, text)`.
async fn read(store: &dyn SkillStore, caller: &SkillCaller<'_>, uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix(SCHEME)?;
    if !valid_file_path(rest) {
        return None;
    }
    for v in visible(store, caller).await {
        let Some(file) = uri.strip_prefix(v.root.as_str()).and_then(|r| r.strip_prefix('/')) else { continue };
        if file == SKILL_FILE {
            return Some((MARKDOWN.into(), skill_md(&v.skill)));
        }
        if let Some(f) = v.skill.files.iter().find(|f| f.path == file && valid_file_path(&f.path)) {
            return Some((f.mime_type.clone(), f.text.clone()));
        }
    }
    None
}

/// Called by `mcp_edge::serve` for `resources/list` and `resources/read`,
/// after the apps hook. `Some(reply)` is answered by the edge; `None` relays.
/// Only a `resources/read` of a `skill://` URI is answered here.
pub async fn intercept_resources(store: &dyn SkillStore, caller: &SkillCaller<'_>, rpc_method: &str, id: &Value, params: &Value) -> Option<Value> {
    if rpc_method != "resources/read" {
        return None;
    }
    let uri = params["uri"].as_str().filter(|u| u.starts_with(SCHEME))?;
    Some(match read(store, caller, uri).await {
        Some((mime_type, text)) => json!({ "jsonrpc": "2.0", "id": id, "result": { "contents": [{ "uri": uri, "mimeType": mime_type, "text": text }] } }),
        None => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": RESOURCE_NOT_FOUND, "message": "Resource not found", "data": { "uri": uri } } }),
    })
}

/// A relayed `resources/list` answer plus the caller's skills, on the first
/// page only (no `cursor`). A runtime without resources (method not found)
/// gets a list of just the skills.
pub async fn with_skills(reply: Vec<u8>, store: &dyn SkillStore, caller: &SkillCaller<'_>, params: &Value) -> Vec<u8> {
    if params.get("cursor").is_some_and(|c| !c.is_null()) {
        return reply;
    }
    let Ok(mut v) = serde_json::from_slice::<Value>(&reply) else { return reply };
    let skills = resources(store, caller).await;
    if v.pointer("/error/code").and_then(Value::as_i64) == Some(METHOD_NOT_FOUND) {
        let id = v["id"].clone();
        return serde_json::to_vec(&json!({ "jsonrpc": "2.0", "id": id, "result": { "resources": skills } })).unwrap_or(reply);
    }
    let Some(list) = v.pointer_mut("/result/resources").and_then(Value::as_array_mut) else { return reply };
    list.retain(|r| !r["uri"].as_str().is_some_and(|u| u.starts_with(SCHEME)));
    list.extend(skills);
    serde_json::to_vec(&v).unwrap_or(reply)
}

/// Adds `resources` (kept if the runtime set it) and the skills extension to a
/// JSON-RPC reply's `result.capabilities`. Used for `initialize` and
/// `server/discover`; lane 48's server card should call [`capability`].
pub fn advertise_value(mut v: Value) -> Value {
    if let Some(caps) = v.pointer_mut("/result/capabilities").and_then(Value::as_object_mut) {
        caps.entry("resources").or_insert_with(|| json!({}));
        let ext = caps.entry("extensions").or_insert_with(|| json!({}));
        if let Some(ext) = ext.as_object_mut() {
            let (key, value) = capability();
            ext.insert(key.into(), value);
        }
    }
    v
}

/// [`advertise_value`] for a relayed body.
pub fn advertise(reply: Vec<u8>) -> Vec<u8> {
    match serde_json::from_slice::<Value>(&reply) {
        Ok(v) if v.pointer("/result/capabilities").is_some_and(Value::is_object) => serde_json::to_vec(&advertise_value(v)).unwrap_or(reply),
        _ => reply,
    }
}

/// The extension entry: `directoryRead` is false (no `resources/directory/read`).
pub fn capability() -> (&'static str, Value) {
    (EXTENSION, json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str, body: &str) -> Skill {
        Skill { name: name.into(), description: format!("{name} skill"), body: body.into(), files: vec![SkillFile { path: "references/notes.md".into(), mime_type: MARKDOWN.into(), text: format!("{name} notes") }] }
    }

    /// user-a owns `alpha`, user-b owns `beta`; user-a is in org-1, user-b in org-2.
    struct FakeStore;

    #[async_trait::async_trait]
    impl SkillStore for FakeStore {
        async fn user_skills(&self, user_id: &str) -> Result<Vec<Skill>, String> {
            Ok(match user_id {
                "user-a" => vec![skill("alpha", "A's private body")],
                "user-b" => vec![skill("beta", "B's private body")],
                _ => vec![],
            })
        }
        async fn org_skills(&self, user_id: &str) -> Result<HashMap<String, Vec<Skill>>, String> {
            let mut m = HashMap::new();
            match user_id {
                "user-a" => m.insert("org-1".into(), vec![skill("org-one", "org 1 body")]),
                "user-b" => m.insert("org-2".into(), vec![skill("org-two", "org 2 body")]),
                _ => None,
            };
            Ok(m)
        }
    }

    fn user(id: &str) -> SkillCaller<'_> {
        SkillCaller::of(id, "claude-ai")
    }

    fn uris(list: &[Value]) -> Vec<String> {
        list.iter().map(|r| r["uri"].as_str().unwrap().to_string()).collect()
    }

    async fn read_reply(caller: &SkillCaller<'_>, uri: &str) -> Value {
        intercept_resources(&FakeStore, caller, "resources/read", &json!(7), &json!({ "uri": uri })).await.expect("skill:// read is answered at the edge")
    }

    #[tokio::test]
    async fn two_users_see_different_skill_lists() {
        let a = uris(&resources(&FakeStore, &user("user-a")).await);
        let b = uris(&resources(&FakeStore, &user("user-b")).await);
        assert!(a.contains(&"skill://user/alpha/SKILL.md".into()) && a.contains(&"skill://org/org-1/org-one/SKILL.md".into()), "{a:?}");
        assert!(!a.iter().any(|u| u.contains("beta") || u.contains("org-2")), "{a:?}");
        assert!(b.contains(&"skill://user/beta/SKILL.md".into()) && b.contains(&"skill://org/org-2/org-two/SKILL.md".into()), "{b:?}");
        assert!(!b.iter().any(|u| u.contains("alpha") || u.contains("org-1")), "{b:?}");
        for list in [&a, &b] {
            assert!(list.contains(&"skill://allternit/planning/SKILL.md".into()), "{list:?}");
            assert!(!list.iter().any(|u| u.ends_with("notes.md")), "supporting files stay out of the list: {list:?}");
        }
    }

    #[tokio::test]
    async fn cross_user_skill_read_is_not_found() {
        let b = user("user-b");
        for uri in ["skill://user/alpha/SKILL.md", "skill://user/alpha/references/notes.md", "skill://org/org-1/org-one/SKILL.md", "skill://user/../user-a/alpha/SKILL.md"] {
            let r = read_reply(&b, uri).await;
            assert_eq!(r["error"]["code"], json!(RESOURCE_NOT_FOUND), "{uri}: {r}");
            assert!(r.get("result").is_none(), "{uri}: {r}");
            assert!(!r.to_string().contains("private body"), "{uri}: {r}");
        }
        let own = read_reply(&user("user-a"), "skill://user/alpha/SKILL.md").await;
        assert_eq!(own["result"]["contents"][0]["mimeType"], json!(MARKDOWN));
        assert!(own["result"]["contents"][0]["text"].as_str().unwrap().contains("A's private body"));
    }

    #[tokio::test]
    async fn read_serves_frontmatter_and_supporting_files() {
        let a = user("user-a");
        let r = read_reply(&a, "skill://allternit/planning/SKILL.md").await;
        let text = r["result"]["contents"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("---\nname: planning\ndescription: \""), "{text}");
        assert!(text.contains("# Planning"), "{text}");
        assert_eq!(r["id"], json!(7));
        let f = read_reply(&a, "skill://user/alpha/references/notes.md").await;
        assert_eq!(f["result"]["contents"][0]["text"], json!("alpha notes"));
    }

    #[tokio::test]
    async fn platform_key_gets_only_builtin_skills() {
        let key = SkillCaller::of("user-a", "platform-key:k1");
        let list = uris(&resources(&FakeStore, &key).await);
        assert!(list.iter().all(|u| u.starts_with("skill://allternit/")), "{list:?}");
        assert_eq!(read_reply(&key, "skill://user/alpha/SKILL.md").await["error"]["code"], json!(RESOURCE_NOT_FOUND));
    }

    #[tokio::test]
    async fn non_skill_reads_and_lists_relay() {
        let a = user("user-a");
        assert!(intercept_resources(&FakeStore, &a, "resources/read", &json!(1), &json!({ "uri": "ui://app/x" })).await.is_none());
        assert!(intercept_resources(&FakeStore, &a, "resources/list", &json!(1), &json!({})).await.is_none());
    }

    #[tokio::test]
    async fn with_skills_merges_and_fills_method_not_found() {
        let a = user("user-a");
        let relayed = br#"{"jsonrpc":"2.0","id":3,"result":{"resources":[{"uri":"ui://app/x","name":"x"},{"uri":"skill://user/beta/SKILL.md","name":"spoof"}]}}"#.to_vec();
        let v: Value = serde_json::from_slice(&with_skills(relayed, &FakeStore, &a, &json!({})).await).unwrap();
        let list = uris(v["result"]["resources"].as_array().unwrap());
        assert!(list.contains(&"ui://app/x".into()) && list.contains(&"skill://user/alpha/SKILL.md".into()), "{list:?}");
        assert!(!list.iter().any(|u| u.contains("beta")), "runtime-supplied skill:// entries are dropped: {list:?}");

        let missing = br#"{"jsonrpc":"2.0","id":4,"error":{"code":-32601,"message":"Method not found"}}"#.to_vec();
        let v: Value = serde_json::from_slice(&with_skills(missing, &FakeStore, &a, &json!({})).await).unwrap();
        assert_eq!(v["id"], json!(4));
        assert!(!v["result"]["resources"].as_array().unwrap().is_empty());

        let page2 = br#"{"jsonrpc":"2.0","id":5,"result":{"resources":[]}}"#.to_vec();
        assert_eq!(with_skills(page2.clone(), &FakeStore, &a, &json!({ "cursor": "p2" })).await, page2);
    }

    #[test]
    fn advertise_adds_resources_and_skills_extension() {
        let reply = br#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"tools":{},"resources":{"subscribe":true}}}}"#.to_vec();
        let v: Value = serde_json::from_slice(&advertise(reply)).unwrap();
        assert_eq!(v["result"]["capabilities"]["resources"], json!({ "subscribe": true }));
        assert_eq!(v["result"]["capabilities"]["extensions"][EXTENSION], json!({}));
        let bare = advertise_value(json!({ "result": { "capabilities": { "tools": {} } } }));
        assert_eq!(bare["result"]["capabilities"]["resources"], json!({}));
        let err = br#"{"jsonrpc":"2.0","id":1,"error":{"code":-1,"message":"x"}}"#.to_vec();
        assert_eq!(advertise(err.clone()), err);
    }

    #[test]
    fn names_and_paths_are_checked() {
        assert!(valid_name("docs-writing") && !valid_name("Docs") && !valid_name("-x") && !valid_name("a/b") && !valid_name(""));
        assert!(valid_file_path("user/a/SKILL.md") && !valid_file_path("user/../a") && !valid_file_path("/a") && !valid_file_path("a//b"));
    }
}
