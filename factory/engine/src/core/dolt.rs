//! Optional Dolt storage backend for Rails.
//!
//! Dolt is a MySQL-compatible database with Git-style versioning. This module
//! lets a Rails workspace replicate its ticket state to a Dolt server so the
//! data can be queried with SQL, branched, and collaborated on like a database.
//!
//! The backend is compile-time optional via the `dolt` feature. When the
//! feature is disabled, no Dolt code is compiled into the library or CLI.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::mysql::MySqlPool;
use sqlx::Row;

use crate::tickets::{Ticket, TicketStore};

/// Connection configuration for a Dolt server.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DoltConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
}

impl DoltConfig {
    /// Load configuration from environment variables.
    ///
    /// Variables: `RAILS_DOLT_HOST`, `RAILS_DOLT_PORT`, `RAILS_DOLT_USER`,
    /// `RAILS_DOLT_PASSWORD`, `RAILS_DOLT_DATABASE`.
    pub fn from_env() -> Self {
        Self {
            host: std::env::var("RAILS_DOLT_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
            port: std::env::var("RAILS_DOLT_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3306),
            user: std::env::var("RAILS_DOLT_USER").unwrap_or_else(|_| "root".to_string()),
            password: std::env::var("RAILS_DOLT_PASSWORD").unwrap_or_default(),
            database: std::env::var("RAILS_DOLT_DATABASE").unwrap_or_else(|_| "rails".to_string()),
        }
    }

    /// Build a MySQL connection string from the configuration.
    pub fn connection_string(&self) -> String {
        format!(
            "mysql://{}:{}@{}:{}/{}",
            urlencoding::encode(&self.user),
            urlencoding::encode(&self.password),
            self.host,
            self.port,
            urlencoding::encode(&self.database)
        )
    }
}

/// Dolt-backed storage for Rails tickets.
pub struct DoltStorage {
    pool: MySqlPool,
}

impl DoltStorage {
    /// Connect to a Dolt server using the provided configuration.
    pub async fn connect(config: &DoltConfig) -> Result<Self> {
        let url = config.connection_string();
        let pool = MySqlPool::connect(&url)
            .await
            .with_context(|| format!("failed to connect to Dolt at {}:{}", config.host, config.port))?;
        Ok(Self { pool })
    }

    /// Create the Rails schema if it does not already exist.
    pub async fn init_schema(&self) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS rails_tickets (
                id VARCHAR(64) PRIMARY KEY,
                status VARCHAR(32) NOT NULL,
                kind VARCHAR(32) NOT NULL,
                priority VARCHAR(8) NOT NULL,
                updated_at DATETIME(6) NOT NULL,
                data JSON NOT NULL
            ) ENGINE=InnoDB",
        )
        .execute(&self.pool)
        .await
        .context("failed to create rails_tickets table")?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS rails_events (
                sequence BIGINT PRIMARY KEY,
                event_hash VARCHAR(64) NOT NULL,
                previous_hash VARCHAR(64),
                data JSON NOT NULL
            ) ENGINE=InnoDB",
        )
        .execute(&self.pool)
        .await
        .context("failed to create rails_events table")?;

        Ok(())
    }

    /// Replace or insert a batch of tickets.
    pub async fn import_tickets(&self, tickets: &[Ticket]) -> Result<usize> {
        let mut tx = self.pool.begin().await.context("failed to begin transaction")?;
        for ticket in tickets {
            let data = sqlx::types::Json(ticket.clone());
            sqlx::query(
                "INSERT INTO rails_tickets (id, status, kind, priority, updated_at, data)
                 VALUES (?, ?, ?, ?, ?, ?)
                 ON DUPLICATE KEY UPDATE
                   status = VALUES(status),
                   kind = VALUES(kind),
                   priority = VALUES(priority),
                   updated_at = VALUES(updated_at),
                   data = VALUES(data)",
            )
            .bind(ticket.id.to_string())
            .bind(ticket.status.to_string())
            .bind(ticket.kind.to_string())
            .bind(ticket.priority.to_string())
            .bind(ticket.updated_at)
            .bind(data)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("failed to import ticket {}", ticket.id))?;
        }
        tx.commit().await.context("failed to commit ticket import")?;
        Ok(tickets.len())
    }

    /// Read all tickets from Dolt.
    pub async fn export_tickets(&self) -> Result<Vec<Ticket>> {
        let rows = sqlx::query("SELECT data FROM rails_tickets")
            .fetch_all(&self.pool)
            .await
            .context("failed to fetch tickets from Dolt")?;

        let mut tickets = Vec::with_capacity(rows.len());
        for row in rows {
            let data: sqlx::types::Json<Ticket> = row.try_get("data")?;
            tickets.push(data.0);
        }
        Ok(tickets)
    }

    /// Push all tickets from the local file store into Dolt.
    pub async fn sync_from_file_store(&self, root: &Path) -> Result<usize> {
        let store = TicketStore::new(root).context("failed to open local ticket store")?;
        let tickets = store.list().context("failed to list local tickets")?;
        self.import_tickets(&tickets).await
    }

    /// Pull all tickets from Dolt into the local file store.
    pub async fn sync_to_file_store(&self, root: &Path) -> Result<usize> {
        let tickets = self.export_tickets().await?;
        let store = TicketStore::new(root).context("failed to open local ticket store")?;
        let mut count = 0;
        for ticket in tickets {
            if store.get(&ticket.id)?.is_some() {
                store.update(&ticket.id, ticket.clone().into())?;
            } else {
                store.create(ticket)?;
            }
            count += 1;
        }
        Ok(count)
    }

    /// Return counts for both sides of the sync.
    pub async fn status(&self, root: &Path) -> Result<DoltStatus> {
        let file_count = TicketStore::new(root)
            .and_then(|s| s.list())
            .map(|v| v.len())
            .unwrap_or(0);
        let db_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rails_tickets")
            .fetch_one(&self.pool)
            .await
            .context("failed to count Dolt tickets")?;
        Ok(DoltStatus {
            file_tickets: file_count,
            dolt_tickets: db_count as usize,
        })
    }
}

/// Status comparison between the local file store and Dolt.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DoltStatus {
    pub file_tickets: usize,
    pub dolt_tickets: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_connection_string_uses_url_encoding() {
        let config = DoltConfig {
            user: "user".to_string(),
            password: "p@ss/w#rd".to_string(),
            host: "127.0.0.1".to_string(),
            port: 3306,
            database: "rails".to_string(),
        };
        let url = config.connection_string();
        assert!(url.starts_with("mysql://"));
        assert!(!url.contains("p@ss/w#rd"));
        assert!(url.contains("p%40ss%2Fw%23rd"));
    }
}
