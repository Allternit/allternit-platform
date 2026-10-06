use std::path::PathBuf;

use anyhow::Result;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Pool, Row, Sqlite,
};

use crate::core::io::ensure_dir;
use crate::core::types::AllternitEvent;

#[derive(Debug, Clone)]
pub struct IndexOptions {
    pub root_dir: Option<PathBuf>,
    pub index_dir: Option<PathBuf>,
}

pub struct Index {
    pool: Pool<Sqlite>,
}

impl Index {
    pub async fn new(opts: IndexOptions) -> Result<Self> {
        let root_dir = opts
            .root_dir
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        let index_dir = opts
            .index_dir
            .unwrap_or_else(|| PathBuf::from(".allternit/index"));
        let base = if index_dir.is_absolute() {
            index_dir
        } else {
            root_dir.join(index_dir)
        };
        ensure_dir(&base)?;
        let db_path = base.join("index.db");
        let connect_opts = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(connect_opts)
            .await?;
        let index = Self { pool };
        index.ensure_schema().await?;
        Ok(index)
    }

    pub async fn index_event(&self, event: &AllternitEvent) -> Result<()> {
        let payload_text = serde_json::to_string(&event.payload).unwrap_or_default();
        let scope_text = event
            .scope
            .as_ref()
            .map(|s| serde_json::to_string(s).unwrap_or_default())
            .unwrap_or_default();
        sqlx::query(
            "INSERT INTO events_fts (event_id, event_type, ts, scope, text) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&event.event_id)
        .bind(&event.r#type)
        .bind(&event.ts)
        .bind(scope_text)
        .bind(payload_text)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn search_events(&self, query: &str, limit: i64) -> Result<Vec<String>> {
        let rows = sqlx::query("SELECT event_id FROM events_fts WHERE events_fts MATCH ? LIMIT ?")
            .bind(query)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        let mut ids = Vec::new();
        for row in rows {
            let id: String = row.try_get("event_id")?;
            ids.push(id);
        }
        Ok(ids)
    }

    pub async fn clear(&self) -> Result<()> {
        sqlx::query("DELETE FROM events_fts")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn rebuild_from_ledger(&self, ledger: &crate::ledger::Ledger) -> Result<usize> {
        self.clear().await?;
        let events = ledger
            .query(crate::core::types::LedgerQuery::default())
            .await?;
        for event in &events {
            let _ = self.index_event(event).await;
        }
        Ok(events.len())
    }

    async fn ensure_schema(&self) -> Result<()> {
        sqlx::query(
            "CREATE VIRTUAL TABLE IF NOT EXISTS events_fts USING fts5(
                event_id UNINDEXED,
                event_type,
                ts UNINDEXED,
                scope,
                text
            )",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
