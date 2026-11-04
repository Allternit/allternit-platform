//! Vendors' official MCP servers, as a sign-in fallback for catalog connectors.
//!
//! When a catalog connector can't issue a sign-in (no Allternit OAuth app for a
//! curated connector, no OAuth app on the connector sidecar, or the sidecar doesn't
//! know the id) and the vendor runs an official MCP server that registers clients
//! itself (OAuth DCR or CIMD), Connect signs in to that server instead: it creates
//! (or reuses) the user's MCP connector for its URL and returns the MCP OAuth
//! start's `authorize_url`. An index row in `connector_connections`
//! (`auth_type = 'official_mcp'`, `metadata.official_mcp.mcp_connector_id`) makes
//! the catalog show it, connected once the MCP sign-in completes; disconnect
//! removes the MCP connector with it.
//!
//! The table is generated: `scripts/gen-official-mcp-servers.mjs` writes
//! `assets/official-mcp-servers.json` from allternit-ai's `official-servers.ts`.

use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::Value;
use std::sync::OnceLock;

/// `connector_connections.auth_type` (and `backend`) of a fallback sign-in.
pub const AUTH_TYPE: &str = "official_mcp";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct OfficialServer {
    pub id: String,
    pub name: String,
    pub url: String,
    /// `oauth-dcr`, `oauth-cimd`, `oauth` or `api-key`.
    pub auth: String,
}

#[derive(Deserialize)]
struct Table {
    servers: Vec<OfficialServer>,
}

fn table() -> &'static [OfficialServer] {
    static T: OnceLock<Vec<OfficialServer>> = OnceLock::new();
    T.get_or_init(|| {
        serde_json::from_str::<Table>(include_str!("../assets/official-mcp-servers.json")).map(|t| t.servers).unwrap_or_default()
    })
}

/// The official server for catalog `id` that Allternit can sign in to on its
/// own (it registers clients by DCR or reads our CIMD document).
pub fn signin_server(id: &str) -> Option<&'static OfficialServer> {
    table().iter().find(|s| s.id == id && matches!(s.auth.as_str(), "oauth-dcr" | "oauth-cimd"))
}

/// Did the catalog path fail only because it has no way to issue a sign-in?
pub fn catalog_cannot_sign_in(body: &Value) -> bool {
    let status = body.get("status").and_then(Value::as_str);
    let error = body.get("error").and_then(Value::as_str);
    status == Some("oauth_app_registration_required")
        || matches!(error, Some("oauth_app_registration_required" | "oauth_app_not_configured" | "connector_not_found" | "sidecar_unavailable"))
}

/// The fallback server for a failed catalog connect, when there is one.
pub fn fallback_for(id: &str, body: &Value) -> Option<&'static OfficialServer> {
    if catalog_cannot_sign_in(body) {
        signin_server(id)
    } else {
        None
    }
}

fn same_url(a: &str, b: &str) -> bool {
    a.trim().trim_end_matches('/').eq_ignore_ascii_case(b.trim().trim_end_matches('/'))
}

/// The user's MCP connector for `server`: an existing one with its URL, else a new one.
pub fn ensure_mcp_connector(conn: &Connection, user_id: &str, server: &OfficialServer) -> rusqlite::Result<String> {
    let mut stmt = conn.prepare("SELECT id, url FROM mcp_connectors WHERE user_id = ?1 ORDER BY created_at ASC")?;
    let existing = stmt
        .query_map(params![user_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .filter_map(Result::ok)
        .find(|(_, url)| same_url(url, &server.url));
    if let Some((id, _)) = existing {
        return Ok(id);
    }
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO mcp_connectors (id, user_id, name, name_id, url, type) VALUES (?1, ?2, ?3, ?4, ?5, 'streamable-http')",
        params![id, user_id, server.name, server.id, server.url],
    )?;
    Ok(id)
}

/// The MCP sign-in for `mcp_connector_id` completed: its catalog rows are connected.
pub fn mark_connected(conn: &Connection, mcp_connector_id: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE connector_connections SET status = 'connected', updated_at = CURRENT_TIMESTAMP
         WHERE auth_type = ?1 AND json_extract(metadata, '$.official_mcp.mcp_connector_id') = ?2",
        params![AUTH_TYPE, mcp_connector_id],
    )
}

/// The MCP connectors behind this user's fallback rows for catalog `connector_id`.
pub fn linked_mcp_connectors(conn: &Connection, user_id: &str, connector_id: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT json_extract(metadata, '$.official_mcp.mcp_connector_id') FROM connector_connections
         WHERE user_id = ?1 AND connector_id = ?2 AND auth_type = ?3",
    )?;
    let ids = stmt
        .query_map(params![user_id, connector_id, AUTH_TYPE], |r| r.get::<_, Option<String>>(0))?
        .filter_map(|r| r.ok().flatten())
        .collect();
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn falls_back_only_when_the_catalog_cannot_sign_in_and_a_self_registering_server_exists() {
        // Notion (curated, no Allternit OAuth app) and Linear (sidecar, no OAuth app) both have DCR servers.
        let no_app = json!({ "status": "oauth_app_registration_required", "connector": "notion" });
        assert_eq!(fallback_for("notion", &no_app).map(|s| s.url.as_str()), Some("https://mcp.notion.com/mcp"));
        let sidecar = json!({ "error": "oauth_app_not_configured", "id": "linear" });
        assert!(fallback_for("linear", &sidecar).is_some());
        assert!(fallback_for("linear", &json!({ "error": "connector_not_found" })).is_some());

        // A sign-in the catalog could issue, or another failure, never switches path.
        assert!(fallback_for("linear", &json!({ "status": "authorization_required", "authorize_url": "x" })).is_none());
        assert!(fallback_for("linear", &json!({ "error": "api_key_required" })).is_none());
        // Slack is excluded from the official list; ids with only a vendor-registered OAuth app don't qualify.
        assert!(fallback_for("slack", &no_app).is_none());
        let oauth_only = table().iter().find(|s| s.auth == "oauth").expect("an oauth-only server");
        assert!(signin_server(&oauth_only.id).is_none());
    }

    #[test]
    fn connector_is_reused_by_url_and_rows_follow_the_mcp_sign_in() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE mcp_connectors (id TEXT PRIMARY KEY, user_id TEXT, name TEXT, name_id TEXT, url TEXT, type TEXT, created_at DATETIME DEFAULT CURRENT_TIMESTAMP);
             CREATE TABLE connector_connections (id TEXT, connector_id TEXT, user_id TEXT, auth_type TEXT, status TEXT, metadata TEXT, updated_at DATETIME);
             INSERT INTO mcp_connectors (id, user_id, name, name_id, url, type) VALUES ('mine', 'alice', 'Notion', 'x', 'https://mcp.notion.com/mcp/', 'http');",
        )
        .unwrap();
        let notion = signin_server("notion").unwrap();
        assert_eq!(ensure_mcp_connector(&c, "alice", notion).unwrap(), "mine");
        let bobs = ensure_mcp_connector(&c, "bob", notion).unwrap();
        assert_ne!(bobs, "mine");
        assert_eq!(ensure_mcp_connector(&c, "bob", notion).unwrap(), bobs);

        c.execute(
            "INSERT INTO connector_connections (id, connector_id, user_id, auth_type, status, metadata) VALUES ('r1', 'notion', 'bob', 'official_mcp', 'pending', ?1)",
            [json!({ "official_mcp": { "mcp_connector_id": bobs } }).to_string()],
        )
        .unwrap();
        assert_eq!(mark_connected(&c, "mine").unwrap(), 0);
        assert_eq!(mark_connected(&c, &bobs).unwrap(), 1);
        let status: String = c.query_row("SELECT status FROM connector_connections WHERE id = 'r1'", [], |r| r.get(0)).unwrap();
        assert_eq!(status, "connected");
        assert_eq!(linked_mcp_connectors(&c, "bob", "notion").unwrap(), vec![bobs]);
        assert!(linked_mcp_connectors(&c, "alice", "notion").unwrap().is_empty());
    }
}
