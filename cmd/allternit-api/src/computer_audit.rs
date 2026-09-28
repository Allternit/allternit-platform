//! Audit logging for the computer real-time plane (Phase 3).
//!
//! Unlike `bot_desktop_audit` (a request middleware keyed by bot id from the
//! path), computer access events are logged explicitly at the action sites:
//! ws-token issues, proxy enable/disable, and every proxied request. Inserts
//! are fire-and-forget so the hot path never blocks on SQLite.

use rusqlite::params;
use tracing::warn;
use uuid::Uuid;

pub const KIND_WS_TOKEN: &str = "ws_token";
pub const KIND_EMBED_TOKEN: &str = "embed_token";
pub const KIND_PROXY_ENABLE: &str = "proxy_enable";
pub const KIND_PROXY_DISABLE: &str = "proxy_disable";
pub const KIND_PROXY: &str = "proxy";
/// Subscription Fabric: Sessions-computer binding and gateway forwards.
pub const KIND_SUBS_BINDING: &str = "subs_binding";
pub const KIND_SUBS_GATEWAY: &str = "subs_gateway";

/// Record one computer access event. Never panics and never blocks the caller;
/// failures are logged and dropped.
pub fn log_computer_access(
    db: &crate::db::DbHandle,
    computer_id: &str,
    user_id: &str,
    kind: &str,
    detail: &str,
) {
    let db = db.clone();
    let computer_id = computer_id.to_owned();
    let user_id = user_id.to_owned();
    let kind = kind.to_owned();
    let detail = detail.to_owned();
    let warn_ctx = format!("{computer_id}/{kind}");
    tokio::spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            let conn = db.connect()?;
            insert_computer_access(&conn, &computer_id, &user_id, &kind, &detail)
        })
        .await;
        if !matches!(result, Ok(Ok(_))) {
            warn!(?result, %warn_ctx, "failed to write computer access log");
        }
    });
}

/// Connection-only insert so tests can exercise it against in-memory SQLite.
pub fn insert_computer_access(
    conn: &rusqlite::Connection,
    computer_id: &str,
    user_id: &str,
    kind: &str,
    detail: &str,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO computer_access_logs (id, computer_id, user_id, kind, detail)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            format!("computer-access-{}", Uuid::new_v4().simple()),
            computer_id,
            user_id,
            kind,
            detail,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE computer_access_logs (
               id TEXT PRIMARY KEY,
               computer_id TEXT,
               user_id TEXT,
               kind TEXT,
               detail TEXT,
               created_at DATETIME DEFAULT CURRENT_TIMESTAMP
             );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn insert_writes_row_readable_back() {
        let conn = memory_conn();
        insert_computer_access(&conn, "computer-1", "user-1", KIND_PROXY, "GET /index → 200")
            .unwrap();
        let (computer_id, user_id, kind, detail): (String, String, String, String) = conn
            .query_row(
                "SELECT computer_id, user_id, kind, detail FROM computer_access_logs",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(computer_id, "computer-1");
        assert_eq!(user_id, "user-1");
        assert_eq!(kind, "proxy");
        assert_eq!(detail, "GET /index → 200");
    }
}
