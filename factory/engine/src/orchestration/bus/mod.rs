use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Pool, Row, Sqlite};

use crate::core::ids::create_event_id;
use crate::core::io::ensure_dir;
use crate::core::types::{AllternitEvent, Actor, ActorType};
use crate::ledger::Ledger;

pub struct BusOptions {
    pub root_dir: PathBuf,
    pub ledger: Arc<Ledger>,
    pub actor_id: Option<String>,
    pub actor_type: Option<ActorType>,
}

pub struct Bus {
    pool: Pool<Sqlite>,
    ledger: Arc<Ledger>,
    actor: Actor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusMessage {
    pub id: i64,
    pub correlation_id: String,
    pub to: String,
    pub from: String,
    pub kind: String,
    pub payload: Value,
    pub transport: String,
    pub status: String,
    pub created_at: String,
}

pub struct NewBusMessage {
    pub correlation_id: String,
    pub to: String,
    pub from: String,
    pub kind: String,
    pub payload: Value,
    pub transport: String,
}

impl Bus {
    pub async fn new(opts: BusOptions) -> Result<Self> {
        let bus_dir = opts.root_dir.join(".allternit").join("bus");
        ensure_dir(&bus_dir)?;
        let db_path = bus_dir.join("queue.db");
        let connect_opts = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(connect_opts)
            .await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                correlation_id TEXT NOT NULL,
                recipient TEXT NOT NULL,
                sender TEXT NOT NULL,
                kind TEXT NOT NULL,
                payload TEXT NOT NULL,
                transport TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                created_at TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS subscriptions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                actor_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                created_at TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await?;

        let actor = Actor {
            r#type: opts.actor_type.unwrap_or(ActorType::Gate),
            id: opts.actor_id.unwrap_or_else(|| "bus".to_string()),
        };

        Ok(Self {
            pool,
            ledger: opts.ledger,
            actor,
        })
    }

    pub fn ensure_event(&self, event_type: &str, payload: Value) -> AllternitEvent {
        AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor: self.actor.clone(),
            scope: None,
            r#type: event_type.to_string(),
            payload,
            provenance: None,
        }
    }

    pub async fn log_event(&self, event_type: &str, payload: Value) -> Result<String> {
        let event = self.ensure_event(event_type, payload);
        let id = self.ledger.append(event).await?;
        Ok(id)
    }

    pub async fn send_message(&self, msg: NewBusMessage) -> Result<i64> {
        let now = Utc::now().to_rfc3339();
        let payload_text = serde_json::to_string(&msg.payload)?;
        let result = sqlx::query(
            "INSERT INTO messages (correlation_id, recipient, sender, kind, payload, transport, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7)",
        )
        .bind(&msg.correlation_id)
        .bind(&msg.to)
        .bind(&msg.from)
        .bind(&msg.kind)
        .bind(&payload_text)
        .bind(&msg.transport)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        let id = result.last_insert_rowid();
        self.log_event(
            "BusMessageSent",
            json!({
                "message_id": id,
                "correlation_id": msg.correlation_id,
                "to": msg.to,
                "from": msg.from,
                "kind": msg.kind,
                "transport": msg.transport,
                "created_at": now
            }),
        )
        .await?;
        Ok(id)
    }

    pub async fn poll_pending(&self, limit: usize) -> Result<Vec<BusMessage>> {
        let rows = sqlx::query(
            "SELECT id, correlation_id, recipient, sender, kind, payload, transport, status, created_at
             FROM messages
             WHERE status = 'pending'
             ORDER BY id ASC
             LIMIT ?1",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        let mut result = Vec::new();
        for row in rows {
            let payload_text: String = row.try_get("payload")?;
            let payload = serde_json::from_str(&payload_text).unwrap_or(json!(null));
            result.push(BusMessage {
                id: row.try_get("id")?,
                correlation_id: row.try_get("correlation_id")?,
                to: row.try_get("recipient")?,
                from: row.try_get("sender")?,
                kind: row.try_get("kind")?,
                payload,
                transport: row.try_get("transport")?,
                status: row.try_get("status")?,
                created_at: row.try_get("created_at")?,
            });
        }
        Ok(result)
    }

    pub async fn poll_pending_for(
        &self,
        recipient: &str,
        transport: Option<&str>,
        limit: usize,
    ) -> Result<Vec<BusMessage>> {
        let mut query = String::from(
            "SELECT id, correlation_id, recipient, sender, kind, payload, transport, status, created_at
             FROM messages
             WHERE status = 'pending' AND recipient = ?1",
        );
        if transport.is_some() {
            query.push_str(" AND transport = ?2");
        }
        query.push_str(" ORDER BY id ASC LIMIT ?3");

        let mut q = sqlx::query(&query);
        q = q.bind(recipient);
        if let Some(trans) = transport {
            q = q.bind(trans);
            q = q.bind(limit as i64);
        } else {
            q = q.bind(limit as i64);
        }

        let rows = q.fetch_all(&self.pool).await?;
        let mut result = Vec::new();
        for row in rows {
            let payload_text: String = row.try_get("payload")?;
            let payload = serde_json::from_str(&payload_text).unwrap_or(json!(null));
            result.push(BusMessage {
                id: row.try_get("id")?,
                correlation_id: row.try_get("correlation_id")?,
                to: row.try_get("recipient")?,
                from: row.try_get("sender")?,
                kind: row.try_get("kind")?,
                payload,
                transport: row.try_get("transport")?,
                status: row.try_get("status")?,
                created_at: row.try_get("created_at")?,
            });
        }
        Ok(result)
    }

    pub async fn mark_delivered(&self, id: i64) -> Result<()> {
        self.update_status(id, "delivered", "BusMessageDelivered")
            .await
    }

    pub async fn mark_failed(&self, id: i64, reason: &str) -> Result<()> {
        self.update_status(id, "failed", "BusMessageFailed").await?;
        self.log_event(
            "BusMessageFailed",
            json!({ "message_id": id, "reason": reason }),
        )
        .await?;
        Ok(())
    }

    pub async fn update_status(&self, id: i64, status: &str, event_type: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE messages SET status = ?1 WHERE id = ?2")
            .bind(status)
            .bind(id)
            .execute(&self.pool)
            .await?;
        self.log_event(
            event_type,
            json!({
                "message_id": id,
                "status": status,
                "updated_at": now
            }),
        )
        .await?;
        Ok(())
    }

    // --- Signal Subscriptions (Ported from mcp-agent) ---

    pub async fn subscribe(&self, actor_id: &str, kind: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO subscriptions (actor_id, kind, created_at) VALUES (?1, ?2, ?3)")
            .bind(actor_id)
            .bind(kind)
            .bind(&now)
            .execute(&self.pool)
            .await?;
        self.log_event(
            "BusSubscribed",
            json!({ "actor_id": actor_id, "kind": kind, "created_at": now }),
        )
        .await?;
        Ok(())
    }

    pub async fn unsubscribe(&self, actor_id: &str, kind: &str) -> Result<()> {
        sqlx::query("DELETE FROM subscriptions WHERE actor_id = ?1 AND kind = ?2")
            .bind(actor_id)
            .bind(kind)
            .execute(&self.pool)
            .await?;
        self.log_event(
            "BusUnsubscribed",
            json!({ "actor_id": actor_id, "kind": kind }),
        )
        .await?;
        Ok(())
    }

    pub async fn broadcast_signal(&self, sender: &str, kind: &str, payload: Value) -> Result<usize> {
        let correlation_id = format!("sig_{}", create_event_id());
        let subscribers = sqlx::query("SELECT actor_id FROM subscriptions WHERE kind = ?1")
            .bind(kind)
            .fetch_all(&self.pool)
            .await?;

        let mut count = 0;
        for row in subscribers {
            let recipient: String = row.try_get("actor_id")?;
            self.send_message(NewBusMessage {
                correlation_id: correlation_id.clone(),
                to: recipient,
                from: sender.to_string(),
                kind: kind.to_string(),
                payload: payload.clone(),
                transport: "internal".to_string(),
            })
            .await?;
            count += 1;
        }

        self.log_event(
            "BusSignalBroadcasted",
            json!({
                "correlation_id": correlation_id,
                "kind": kind,
                "recipient_count": count
            }),
        )
        .await?;

        Ok(count)
    }
}
