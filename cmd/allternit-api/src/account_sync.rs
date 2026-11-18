//! Account projects → this computer (cloud-api `account_projects`, migration
//! 149; docs: surfaces/docs/api/account-projects.mdx).
//!
//! A person with no computer makes projects and bots on their phone; cloud-api
//! keeps them on the account. When this computer is paired it pulls them on
//! boot and every [`PULL_INTERVAL`] (sooner after a failure, so a fresh pairing
//! syncs within [`RETRY_INTERVAL`]):
//!
//! * **Unadopted, live row:** insert it into `cowork_projects` under the SAME
//!   id (idempotent), then `POST /api/v1/account-projects/:id/adopted` with
//!   this runtime's id. If another computer won (409), drop the copy we just
//!   made. The first computer adopts.
//! * **Adopted by this computer, tombstoned:** delete the local project.
//! * **Adopted by this computer, live:** the computer wins. A local project
//!   that is gone is deleted on the cloud; a changed title, description or
//!   instructions is PATCHed back.
//!
//! Calls are signed with the runtime's device token (`relay_auth`), the same
//! scheme as the presence heartbeat and the Files drive, so no user session is
//! needed. Inert until the runtime is paired.

use std::time::Duration;

use async_trait::async_trait;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::cowork_routes::{delete_project_rows, insert_project, valid_project_id, CreateOutcome};
use crate::db::DbHandle;

pub const PULL_INTERVAL: Duration = Duration::from_secs(300);
const RETRY_INTERVAL: Duration = Duration::from_secs(60);
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ANSWER_BYTES: usize = 4 * 1024 * 1024;
const BASE: &str = "/api/v1/account-projects";

static CHANGED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Pull now (e.g. right after this computer pairs or reconnects).
pub fn nudge() {
    CHANGED.notify_one();
}

/// One request to cloud-api: `(status, body)`. `Err` = not sent. Tests
/// replace it (the network is the edge).
#[async_trait]
pub trait AccountCloud: Send + Sync {
    async fn send(&self, method: &str, path: &str, body: Vec<u8>) -> Result<(u16, Vec<u8>), String>;
}

/// Production: `ALLTERNIT_CLOUD_API_URL` (default api.allternit.com), signed
/// with this runtime's device token.
pub struct SignedAccountCloud;

#[async_trait]
impl AccountCloud for SignedAccountCloud {
    async fn send(&self, method: &str, path: &str, body: Vec<u8>) -> Result<(u16, Vec<u8>), String> {
        let secret = crate::relay_auth::process_secret();
        let (Some(token), Some(owner), Some(runtime_id)) = (secret.device_token(), secret.paired_owner(), secret.runtime_id()) else {
            return Err("this computer is not paired".into());
        };
        // cloud-api verifies the signature over the path without its query.
        let signed_path = path.split('?').next().unwrap_or(path);
        let method = method.to_ascii_uppercase();
        let m = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
        let mut req = crate::net_client::client()
            .request(m, format!("{}{path}", crate::phone_sync::cloud_base()))
            .timeout(TIMEOUT)
            .header("content-type", "application/json");
        for (k, v) in crate::relay_auth::signed_headers(&token, &owner, &method, signed_path, &body) {
            req = req.header(k, v);
        }
        req = req.header(crate::relay_auth::RUNTIME_ID_HEADER, runtime_id);
        if !body.is_empty() {
            req = req.body(body);
        }
        let res = req.send().await.map_err(|e| e.to_string())?;
        let status = res.status().as_u16();
        let bytes = res.bytes().await.map_err(|e| e.to_string())?;
        if bytes.len() > MAX_ANSWER_BYTES {
            return Err("cloud-api answered with too much data".into());
        }
        Ok((status, bytes.to_vec()))
    }
}

#[derive(Debug, Deserialize)]
struct Remote {
    id: String,
    kind: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    instructions: Option<String>,
    #[serde(default)]
    metadata: Value,
    #[serde(default)]
    adopted_computer_id: Option<String>,
    #[serde(default)]
    deleted_at: Option<String>,
}

#[derive(Deserialize)]
struct Listing {
    #[serde(default)]
    projects: Vec<Remote>,
}

#[derive(Debug, Default)]
pub struct Report {
    pub adopted: Vec<String>,
    pub deleted: Vec<String>,
    pub mirrored: Vec<String>,
    pub skipped: Vec<String>,
}

/// The local `metadata` for an adopted row: the cloud metadata plus
/// `accountProject` = `{kind}`, so clients can tell a bot from a code project.
fn local_metadata(remote: &Remote) -> String {
    let mut map = remote.metadata.as_object().cloned().unwrap_or_default();
    map.insert("accountProject".into(), json!({ "kind": remote.kind }));
    Value::Object(map).to_string()
}

fn ok(status: u16) -> bool {
    (200..300).contains(&status)
}

/// One pass. `owner` is the paired account (cowork rows are stored under the
/// Clerk user id), `runtime_id` this computer's cloud id.
pub async fn sync_once(db: &DbHandle, owner: &str, runtime_id: &str, cloud: &dyn AccountCloud) -> Result<Report, String> {
    let (status, body) = cloud.send("GET", &format!("{BASE}?include_adopted=true&include_deleted=true"), Vec::new()).await?;
    match status {
        200..=299 => {}
        404 => return Ok(Report::default()), // cloud-api without the routes yet
        401 | 403 => return Err(format!("the cloud refused this runtime's credential ({status})")),
        s => return Err(format!("the cloud answered {s} for account projects")),
    }
    let listing: Listing = serde_json::from_slice(&body).map_err(|e| format!("unreadable account project listing: {e}"))?;
    let mut report = Report::default();
    for remote in listing.projects {
        if !valid_project_id(&remote.id) {
            report.skipped.push(remote.id);
            continue;
        }
        let mine = remote.adopted_computer_id.as_deref() == Some(runtime_id);
        match (remote.adopted_computer_id.is_some(), remote.deleted_at.is_some(), mine) {
            (false, false, _) => adopt(db, owner, runtime_id, cloud, &remote, &mut report).await?,
            (true, true, true) => {
                let conn = db.connect().map_err(|e| e.to_string())?;
                if delete_project_rows(&conn, &remote.id, owner).map_err(|e| e.to_string())? {
                    report.deleted.push(remote.id);
                }
            }
            (true, false, true) => mirror(db, owner, cloud, &remote, &mut report).await?,
            _ => {}
        }
    }
    Ok(report)
}

async fn adopt(db: &DbHandle, owner: &str, runtime_id: &str, cloud: &dyn AccountCloud, remote: &Remote, report: &mut Report) -> Result<(), String> {
    let outcome = {
        let conn = db.connect().map_err(|e| e.to_string())?;
        insert_project(
            &conn,
            &remote.id,
            owner,
            &remote.title,
            remote.description.as_deref(),
            remote.instructions.as_deref(),
            Some(&local_metadata(remote)),
            None,
            None,
        )
        .map_err(|e| e.to_string())?
    };
    if matches!(outcome, CreateOutcome::Taken) {
        report.skipped.push(remote.id.clone());
        return Ok(());
    }
    let body = json!({ "computer_id": runtime_id }).to_string().into_bytes();
    let (status, _) = cloud.send("POST", &format!("{BASE}/{}/adopted", remote.id), body).await?;
    if ok(status) {
        report.adopted.push(remote.id.clone());
    } else if status == 409 && matches!(outcome, CreateOutcome::Created) {
        // Another computer adopted it first; drop the copy made just now.
        let conn = db.connect().map_err(|e| e.to_string())?;
        delete_project_rows(&conn, &remote.id, owner).map_err(|e| e.to_string())?;
        report.skipped.push(remote.id.clone());
    } else {
        // Kept locally; the next pass adopts it again (the insert is idempotent).
        report.skipped.push(remote.id.clone());
    }
    Ok(())
}

async fn mirror(db: &DbHandle, owner: &str, cloud: &dyn AccountCloud, remote: &Remote, report: &mut Report) -> Result<(), String> {
    let local: Option<(String, Option<String>, Option<String>)> = {
        let conn = db.connect().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT title, description, instructions FROM cowork_projects WHERE id = ?1 AND user_id = ?2",
            params![remote.id, owner],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
    };
    let path = format!("{BASE}/{}", remote.id);
    let Some((title, description, instructions)) = local else {
        // Deleted on this computer: the computer wins.
        let (status, _) = cloud.send("DELETE", &path, Vec::new()).await?;
        if ok(status) || status == 404 {
            report.mirrored.push(remote.id.clone());
        }
        return Ok(());
    };
    let blank = |s: &Option<String>| s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let mut patch = Map::new();
    if !title.trim().is_empty() && title.trim() != remote.title {
        patch.insert("title".into(), json!(title.trim()));
    }
    if blank(&description) != blank(&remote.description) {
        patch.insert("description".into(), json!(blank(&description)));
    }
    if blank(&instructions) != blank(&remote.instructions) {
        patch.insert("instructions".into(), json!(blank(&instructions)));
    }
    if patch.is_empty() {
        return Ok(());
    }
    let (status, _) = cloud.send("PATCH", &path, Value::Object(patch).to_string().into_bytes()).await?;
    if ok(status) {
        report.mirrored.push(remote.id.clone());
    }
    Ok(())
}

/// Pull on boot, every [`PULL_INTERVAL`], and on [`nudge`]. Inert while unpaired.
pub fn spawn(db: DbHandle) {
    tokio::spawn(async move {
        let mut wait = Duration::from_secs(10);
        loop {
            tokio::select! {
                _ = CHANGED.notified() => {}
                _ = tokio::time::sleep(wait) => {}
            }
            let secret = crate::relay_auth::process_secret();
            let (Some(owner), Some(runtime_id)) = (secret.paired_owner(), secret.runtime_id()) else {
                wait = RETRY_INTERVAL;
                continue;
            };
            wait = match sync_once(&db, &owner, &runtime_id, &SignedAccountCloud).await {
                Ok(r) => {
                    if !r.adopted.is_empty() || !r.deleted.is_empty() || !r.mirrored.is_empty() {
                        tracing::info!(adopted = r.adopted.len(), deleted = r.deleted.len(), mirrored = r.mirrored.len(), "account projects synced");
                    }
                    PULL_INTERVAL
                }
                Err(e) => {
                    tracing::debug!("account project sync: {e}");
                    RETRY_INTERVAL
                }
            };
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// cloud-api stand-in: answers the listing and records every call.
    struct FakeCloud {
        listing: Value,
        adopt_status: u16,
        calls: Mutex<Vec<(String, String, String)>>,
    }

    #[async_trait]
    impl AccountCloud for FakeCloud {
        async fn send(&self, method: &str, path: &str, body: Vec<u8>) -> Result<(u16, Vec<u8>), String> {
            self.calls.lock().unwrap().push((method.into(), path.into(), String::from_utf8(body).unwrap()));
            Ok(match method {
                "GET" => (200, self.listing.to_string().into_bytes()),
                "POST" => (self.adopt_status, b"{}".to_vec()),
                _ => (200, b"{}".to_vec()),
            })
        }
    }

    fn project(id: &str, title: &str, adopted: Option<&str>, deleted: bool) -> Value {
        json!({ "id": id, "kind": "bot", "title": title, "description": null, "instructions": "Be brief",
                "metadata": { "brief": { "status": "ready" } }, "adopted_computer_id": adopted,
                "deleted_at": if deleted { json!("2026-11-18T10:00:00Z") } else { Value::Null } })
    }

    async fn test_db(tag: &str) -> DbHandle {
        let dir = std::env::temp_dir().join(format!("allternit-acctsync-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::test_helpers::app_state(&dir).await.db.clone()
    }

    fn local(db: &DbHandle, id: &str) -> Option<(String, String)> {
        db.connect().unwrap().query_row("SELECT title, metadata FROM cowork_projects WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?))).optional().unwrap()
    }

    #[tokio::test]
    async fn adopts_under_the_same_id_deletes_tombstones_and_mirrors_local_edits() {
        let db = test_db("sync").await;
        let a = "11111111-1111-4111-8111-111111111111";
        let gone = "22222222-2222-4222-8222-222222222222";
        let renamed = "33333333-3333-4333-8333-333333333333";
        let removed = "44444444-4444-4444-8444-444444444444";
        let theirs = "55555555-5555-4555-8555-555555555555";
        {
            let conn = db.connect().unwrap();
            for (id, title) in [(gone, "Old"), (renamed, "New name")] {
                insert_project(&conn, id, "user_1", title, None, Some("Be brief"), None, None, None).unwrap();
            }
        }
        let cloud = FakeCloud {
            listing: json!({ "projects": [
                project(a, "Morning helper", None, false),
                project(gone, "Old", Some("rt_me"), true),
                project(renamed, "Old name", Some("rt_me"), false),
                project(removed, "Removed here", Some("rt_me"), false),
                project(theirs, "Other computer", Some("rt_other"), false),
            ] }),
            adopt_status: 200,
            calls: Mutex::new(Vec::new()),
        };
        let report = sync_once(&db, "user_1", "rt_me", &cloud).await.unwrap();
        assert_eq!(report.adopted, vec![a.to_string()]);
        assert_eq!(report.deleted, vec![gone.to_string()]);

        let (title, metadata) = local(&db, a).expect("adopted under the cloud id");
        assert_eq!(title, "Morning helper");
        let metadata: Value = serde_json::from_str(&metadata).unwrap();
        assert_eq!((metadata["accountProject"]["kind"].as_str(), metadata["brief"]["status"].as_str()), (Some("bot"), Some("ready")));
        assert!(local(&db, gone).is_none());
        assert!(local(&db, theirs).is_none(), "another computer's project is not copied");

        let calls = cloud.calls.lock().unwrap().clone();
        assert!(calls.contains(&("POST".into(), format!("{BASE}/{a}/adopted"), json!({ "computer_id": "rt_me" }).to_string())));
        assert!(calls.contains(&("PATCH".into(), format!("{BASE}/{renamed}"), json!({ "title": "New name" }).to_string())));
        assert!(calls.contains(&("DELETE".into(), format!("{BASE}/{removed}"), String::new())));
        assert!(!calls.iter().any(|(_, path, _)| path.contains(theirs)));

        // A second pass adds nothing: the insert is idempotent.
        sync_once(&db, "user_1", "rt_me", &cloud).await.unwrap();
        let count: i64 = db.connect().unwrap().query_row("SELECT COUNT(*) FROM cowork_projects WHERE id = ?1", params![a], |r| r.get(0)).unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn losing_the_adoption_race_drops_the_copy() {
        let db = test_db("race").await;
        let id = "66666666-6666-4666-8666-666666666666";
        let cloud = FakeCloud { listing: json!({ "projects": [project(id, "Race", None, false)] }), adopt_status: 409, calls: Mutex::new(Vec::new()) };
        let report = sync_once(&db, "user_1", "rt_me", &cloud).await.unwrap();
        assert!(report.adopted.is_empty());
        assert!(local(&db, id).is_none());
    }
}
