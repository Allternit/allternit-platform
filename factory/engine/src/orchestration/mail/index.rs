//! Mail full-text search index (E2-R1).
//!
//! SQLite FTS5 index over typed mail messages, following the
//! `index/search.rs` conventions: sqlx pool, `ensure_schema`,
//! `clear` + `rebuild_from_ledger`. The database lives at
//! `.allternit/mail/mail_index.db` and is a REBUILDABLE INDEX ONLY —
//! canonical state stays in the ledger and the message body files.

use std::path::{Path, PathBuf};

use anyhow::Result;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Pool, Row, Sqlite,
};

use crate::core::io::ensure_dir;
use crate::core::types::AllternitEvent;

#[derive(Debug, Clone)]
pub struct MailIndexOptions {
    pub root_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MailSearchHit {
    pub message_id: String,
    pub thread_id: String,
    pub from_agent: String,
    pub subject: Option<String>,
    pub ts: String,
    pub excerpt: String,
}

pub struct MailIndex {
    root_dir: PathBuf,
    pool: Pool<Sqlite>,
}

impl MailIndex {
    pub async fn new(opts: MailIndexOptions) -> Result<Self> {
        let root_dir = opts
            .root_dir
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        let base = root_dir.join(".allternit/mail");
        ensure_dir(&base)?;
        let db_path = base.join("mail_index.db");
        let connect_opts = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(connect_opts)
            .await?;
        let index = Self { root_dir, pool };
        index.ensure_schema().await?;
        Ok(index)
    }

    /// Index one typed message. Call on emit from the typed send path.
    pub async fn index_message(
        &self,
        message_id: &str,
        thread_id: &str,
        from_agent: &str,
        subject: Option<&str>,
        body: &str,
        ts: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO mail_messages_fts (message_id, thread_id, from_agent, subject, body, ts)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(message_id)
        .bind(thread_id)
        .bind(from_agent)
        .bind(subject.unwrap_or_default())
        .bind(body)
        .bind(ts)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Index an event if — and only if — it is a typed `MessageSent`. Legacy
    /// (`body_ref`) sends and non-`MessageSent` traffic (e.g. the
    /// `MailAssetShared` events on `wih:pipeline-*` threads) are skipped
    /// gracefully. The body text is read back from the event's `body_path`.
    pub async fn index_event(&self, event: &AllternitEvent) -> Result<bool> {
        if event.r#type != "MessageSent" {
            return Ok(false);
        }
        let payload = &event.payload;
        // Typed envelopes carry from_agent + body_path; anything else is the
        // legacy shape and stays out of the FTS index.
        let (Some(from_agent), Some(body_path)) = (
            payload.get("from_agent").and_then(|v| v.as_str()),
            payload.get("body_path").and_then(|v| v.as_str()),
        ) else {
            return Ok(false);
        };
        let Some(thread_id) = payload.get("thread_id").and_then(|v| v.as_str()) else {
            return Ok(false);
        };
        let subject = payload.get("subject").and_then(|v| v.as_str());
        let body = read_body(&self.root_dir, body_path);
        self.index_message(
            &event.event_id,
            thread_id,
            from_agent,
            subject,
            &body,
            &event.ts,
        )
        .await?;
        Ok(true)
    }

    /// bm25-ranked search. Excerpt is an FTS5 snippet over the body column.
    pub async fn search_messages(&self, query: &str, limit: i64) -> Result<Vec<MailSearchHit>> {
        let rows = sqlx::query(
            "SELECT message_id, thread_id, from_agent, subject, ts,
                    snippet(mail_messages_fts, 4, '', '', '…', 16) AS excerpt
             FROM mail_messages_fts
             WHERE mail_messages_fts MATCH ?
             ORDER BY bm25(mail_messages_fts)
             LIMIT ?",
        )
        .bind(fts_query(query))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        let mut hits = Vec::new();
        for row in rows {
            let subject: String = row.try_get("subject")?;
            hits.push(MailSearchHit {
                message_id: row.try_get("message_id")?,
                thread_id: row.try_get("thread_id")?,
                from_agent: row.try_get("from_agent")?,
                subject: if subject.is_empty() { None } else { Some(subject) },
                ts: row.try_get("ts")?,
                excerpt: row.try_get("excerpt")?,
            });
        }
        Ok(hits)
    }

    pub async fn clear(&self) -> Result<()> {
        sqlx::query("DELETE FROM mail_messages_fts")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Full rescan from the ledger; idempotent (clears first). Used for
    /// recovery after the index db is wiped. Returns the number of typed
    /// messages indexed.
    pub async fn rebuild_from_ledger(&self, ledger: &crate::ledger::Ledger) -> Result<usize> {
        self.clear().await?;
        let events = ledger
            .query(crate::core::types::LedgerQuery::default())
            .await?;
        let mut indexed = 0;
        for event in &events {
            if self.index_event(event).await? {
                indexed += 1;
            }
        }
        Ok(indexed)
    }

    async fn ensure_schema(&self) -> Result<()> {
        sqlx::query(
            "CREATE VIRTUAL TABLE IF NOT EXISTS mail_messages_fts USING fts5(
                message_id UNINDEXED,
                thread_id UNINDEXED,
                from_agent,
                subject,
                body,
                ts UNINDEXED
            )",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// Read a message body file; a missing/unreadable body indexes as empty
/// rather than failing the indexing pass.
fn read_body(root_dir: &Path, body_path: &str) -> String {
    let path = root_dir.join(body_path);
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Escape user input into a plain-term FTS5 query: each whitespace-separated
/// token becomes a quoted phrase, so FTS operators in user input can't break
/// the MATCH.
fn fts_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::ActorType;
    use crate::ledger::{Ledger, LedgerOptions};
    use crate::mail::mail::{Mail, MailOptions};
    use crate::mail::types::{MailImportance, TypedMessage};
    use std::sync::Arc;
    use tempfile::TempDir;

    fn test_ledger(tmp: &TempDir) -> Arc<Ledger> {
        Arc::new(Ledger::new(LedgerOptions {
            root_dir: Some(tmp.path().to_path_buf()),
            ledger_dir: Some(PathBuf::from(".allternit/ledger")),
        }))
    }

    fn test_mail(tmp: &TempDir, ledger: Arc<Ledger>, index: Option<Arc<MailIndex>>) -> Mail {
        Mail::new(MailOptions {
            root_dir: Some(tmp.path().to_path_buf()),
            ledger,
            actor_id: Some("test".to_string()),
            actor_type: Some(ActorType::Agent),
            mail_index: index,
        })
    }

    fn typed(subject: &str, body: &str) -> TypedMessage {
        TypedMessage {
            from_agent: "alpha".to_string(),
            to_agents: vec!["beta".to_string()],
            subject: Some(subject.to_string()),
            importance: MailImportance::Normal,
            ack_required: false,
            body: body.to_string(),
        }
    }

    #[tokio::test]
    async fn search_finds_by_subject_and_body_word() {
        let tmp = TempDir::new().unwrap();
        let index = Arc::new(MailIndex::new(MailIndexOptions {
            root_dir: Some(tmp.path().to_path_buf()),
        })
        .await
        .unwrap());
        let mail = test_mail(&tmp, test_ledger(&tmp), Some(index.clone()));
        mail.ensure_thread("mail:general").await.unwrap();

        // Indexed on emit.
        mail.send_typed_message(
            "mail:general",
            typed("deploy window Friday", "Please review the rollout plan."),
        )
        .await
        .unwrap();
        mail.send_typed_message(
            "mail:general",
            typed("lunch menu", "Tacos again; nothing about releases."),
        )
        .await
        .unwrap();

        // By subject word.
        let hits = index.search_messages("deploy", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].subject.as_deref(), Some("deploy window Friday"));
        assert_eq!(hits[0].from_agent, "alpha");
        assert_eq!(hits[0].thread_id, "mail:general");
        assert!(!hits[0].ts.is_empty());
        assert!(!hits[0].excerpt.is_empty());

        // By body-only word.
        let hits = index.search_messages("rollout", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].subject.as_deref(), Some("deploy window Friday"));
        assert!(hits[0].excerpt.contains("rollout"));

        // FTS operator garbage in the query is treated as plain text.
        assert!(index.search_messages("deploy OR (", 10).await.is_ok());
    }

    #[tokio::test]
    async fn legacy_and_non_message_events_are_skipped() {
        let tmp = TempDir::new().unwrap();
        let ledger = test_ledger(&tmp);
        let mail = test_mail(&tmp, ledger.clone(), None);
        mail.ensure_thread("mail:general").await.unwrap();
        mail.ensure_thread("wih:pipeline-probe").await.unwrap();

        // Legacy untyped send + the pipeline's MailAssetShared traffic.
        mail.send_message("mail:general", "legacy body_ref text", vec![])
            .await
            .unwrap();
        mail.share_asset("wih:pipeline-probe", "outputs/x.png", None)
            .await
            .unwrap();

        let index = MailIndex::new(MailIndexOptions {
            root_dir: Some(tmp.path().to_path_buf()),
        })
        .await
        .unwrap();
        let indexed = index.rebuild_from_ledger(&ledger).await.unwrap();
        assert_eq!(indexed, 0);
        assert!(index
            .search_messages("legacy", 10)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn rebuild_after_wipe_restores_same_results() {
        let tmp = TempDir::new().unwrap();
        let ledger = test_ledger(&tmp);
        let mail = test_mail(&tmp, ledger.clone(), None);
        mail.ensure_thread("mail:general").await.unwrap();
        mail.send_typed_message(
            "mail:general",
            typed("deploy window Friday", "rollout plan"),
        )
        .await
        .unwrap();

        let index = MailIndex::new(MailIndexOptions {
            root_dir: Some(tmp.path().to_path_buf()),
        })
        .await
        .unwrap();
        assert_eq!(index.rebuild_from_ledger(&ledger).await.unwrap(), 1);
        let before = index.search_messages("deploy", 10).await.unwrap();
        assert_eq!(before.len(), 1);

        // Wipe the index db file entirely, then rebuild from the ledger.
        drop(index);
        std::fs::remove_file(tmp.path().join(".allternit/mail/mail_index.db")).unwrap();
        let index = MailIndex::new(MailIndexOptions {
            root_dir: Some(tmp.path().to_path_buf()),
        })
        .await
        .unwrap();
        assert_eq!(index.rebuild_from_ledger(&ledger).await.unwrap(), 1);
        let after = index.search_messages("deploy", 10).await.unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].message_id, before[0].message_id);
        assert_eq!(after[0].subject, before[0].subject);

        // Rebuild is idempotent (no duplicates on a second pass).
        assert_eq!(index.rebuild_from_ledger(&ledger).await.unwrap(), 1);
        assert_eq!(index.search_messages("deploy", 10).await.unwrap().len(), 1);
    }
}
