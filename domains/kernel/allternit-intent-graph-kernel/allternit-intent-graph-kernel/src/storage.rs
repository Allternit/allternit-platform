use sqlx::{SqlitePool, Row};
use uuid::Uuid;
use chrono::{DateTime, Utc};
use crate::schema::{Node, Edge, Event, SourceRef, NodeId, EdgeId, EventId, NodeType, EdgeType, NodeStatus};
use crate::error::{IGKError, Result};

#[derive(Clone)]
pub struct Storage {
    pool: SqlitePool,
}

impl Storage {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn init(&self) -> Result<()> {
        sqlx::query("CREATE TABLE IF NOT EXISTS nodes (node_id TEXT PRIMARY KEY, node_type TEXT NOT NULL, status TEXT NOT NULL, priority INTEGER NOT NULL, owner TEXT NOT NULL, source_refs TEXT NOT NULL, attributes TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL)").execute(&self.pool).await.map_err(IGKError::Database)?;
        sqlx::query("CREATE TABLE IF NOT EXISTS edges (edge_id TEXT PRIMARY KEY, from_node_id TEXT NOT NULL, to_node_id TEXT NOT NULL, edge_type TEXT NOT NULL, metadata TEXT NOT NULL, created_at TEXT NOT NULL, FOREIGN KEY (from_node_id) REFERENCES nodes(node_id), FOREIGN KEY (to_node_id) REFERENCES nodes(node_id))").execute(&self.pool).await.map_err(IGKError::Database)?;
        sqlx::query("CREATE TABLE IF NOT EXISTS events (event_id TEXT PRIMARY KEY, timestamp TEXT NOT NULL, actor TEXT NOT NULL, action TEXT NOT NULL, target TEXT NOT NULL, before_state TEXT, after_state TEXT, policy_decision TEXT)").execute(&self.pool).await.map_err(IGKError::Database)?;

        Ok(())
    }

    pub async fn create_node(&self, node: &Node) -> Result<Node> {
        sqlx::query(
            r#"
            INSERT INTO nodes (
                node_id, node_type, status, priority, owner,
                source_refs, attributes, created_at, updated_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(node.node_id.to_string())
        .bind(serde_json::to_string(&node.node_type).unwrap().replace("\"", ""))
        .bind(serde_json::to_string(&node.status).unwrap().replace("\"", ""))
        .bind(node.priority)
        .bind(&node.owner)
        .bind(serde_json::to_string(&node.source_refs).unwrap_or_else(|_| "[]".to_string()))
        .bind(serde_json::to_string(&node.attributes).unwrap_or_else(|_| "{}".to_string()))
        .bind(node.created_at.to_rfc3339())
        .bind(node.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(IGKError::Database)?;

        Ok(node.clone())
    }

    pub async fn get_node(&self, node_id: &NodeId) -> Result<Option<Node>> {
        let row = sqlx::query(r#"SELECT * FROM nodes WHERE node_id = ?"#)
            .bind(node_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(IGKError::Database)?;

        match row {
            Some(row) => Ok(Some(self.map_row_to_node(&row)?)),
            None => Ok(None),
        }
    }

    fn map_row_to_node(&self, row: &sqlx::sqlite::SqliteRow) -> Result<Node> {
        use sqlx::Row;
        use std::str::FromStr;

        let node_id_str: String = row.get("node_id");
        let node_type_str: String = row.get("node_type");
        let status_str: String = row.get("status");
        let created_at_str: String = row.get("created_at");
        let updated_at_str: String = row.get("updated_at");
        let source_refs_str: String = row.get("source_refs");
        let attributes_str: String = row.get("attributes");

        Ok(Node {
            node_id: Uuid::from_str(&node_id_str).map_err(|e| IGKError::Internal(e.to_string()))?,
            node_type: serde_json::from_str(&format!("\"{}\"", node_type_str)).map_err(IGKError::Serialization)?,
            status: serde_json::from_str(&format!("\"{}\"", status_str)).map_err(IGKError::Serialization)?,
            priority: row.get("priority"),
            owner: row.get("owner"),
            source_refs: serde_json::from_str(&source_refs_str).map_err(IGKError::Serialization)?,
            attributes: serde_json::from_str(&attributes_str).map_err(IGKError::Serialization)?,
            created_at: DateTime::parse_from_rfc3339(&created_at_str).map_err(|e| IGKError::Internal(e.to_string()))?.with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(&updated_at_str).map_err(|e| IGKError::Internal(e.to_string()))?.with_timezone(&Utc),
        })
    }

    pub async fn search_nodes(
        &self,
        node_type: Option<NodeType>,
        owner: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<Node>> {
        let mut sql = "SELECT * FROM nodes WHERE 1=1".to_string();
        if node_type.is_some() {
            sql.push_str(" AND node_type = ?");
        }
        if owner.is_some() {
            sql.push_str(" AND owner = ?");
        }
        if limit.is_some() {
            sql.push_str(" LIMIT ?");
        }

        let mut query = sqlx::query(&sql);

        if let Some(nt) = node_type {
            query = query.bind(serde_json::to_string(&nt).unwrap().replace("\"", ""));
        }

        if let Some(o) = owner {
            query = query.bind(o);
        }

        if let Some(l) = limit {
            query = query.bind(l as i64);
        }

        let rows = query
            .fetch_all(&self.pool)
            .await
            .map_err(IGKError::Database)?;

        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(self.map_row_to_node(&row)?);
        }

        Ok(nodes)
    }

    pub async fn create_edge(&self, edge: &Edge) -> Result<Edge> {
        sqlx::query(
            r#"
            INSERT INTO edges (
                edge_id, from_node_id, to_node_id, edge_type,
                metadata, created_at
            )
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(edge.edge_id.to_string())
        .bind(edge.from_node_id.to_string())
        .bind(edge.to_node_id.to_string())
        .bind(serde_json::to_string(&edge.edge_type).unwrap().replace("\"", ""))
        .bind(serde_json::to_string(&edge.metadata).unwrap_or_else(|_| "{}".to_string()))
        .bind(edge.created_at.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(IGKError::Database)?;

        Ok(edge.clone())
    }

    pub async fn get_subgraph(
        &self,
        root_node_id: &NodeId,
        depth: Option<usize>,
    ) -> Result<Vec<(Node, Edge)>> {
        // Breadth-first walk over outgoing edges from the root. Each result pairs
        // an edge with the node it points to. `depth` bounds the number of hops
        // (None = unbounded); the visited set keeps cycles from looping forever.
        use std::collections::{HashSet, VecDeque};

        let mut results = Vec::new();
        let mut visited: HashSet<NodeId> = HashSet::new();
        let mut queue: VecDeque<(NodeId, usize)> = VecDeque::new();
        visited.insert(*root_node_id);
        queue.push_back((*root_node_id, 0));

        while let Some((node_id, level)) = queue.pop_front() {
            if depth.is_some_and(|max| level >= max) {
                continue;
            }

            let rows = sqlx::query(
                r#"SELECT * FROM edges WHERE from_node_id = ? ORDER BY created_at, edge_id"#,
            )
            .bind(node_id.to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(IGKError::Database)?;

            for row in rows {
                let edge = self.map_row_to_edge(&row)?;
                let target = self
                    .get_node(&edge.to_node_id)
                    .await?
                    .ok_or_else(|| IGKError::NodeNotFound(edge.to_node_id.to_string()))?;
                if visited.insert(edge.to_node_id) {
                    queue.push_back((edge.to_node_id, level + 1));
                }
                results.push((target, edge));
            }
        }

        Ok(results)
    }

    fn map_row_to_edge(&self, row: &sqlx::sqlite::SqliteRow) -> Result<Edge> {
        use std::str::FromStr;

        let edge_id_str: String = row.get("edge_id");
        let from_str: String = row.get("from_node_id");
        let to_str: String = row.get("to_node_id");
        let edge_type_str: String = row.get("edge_type");
        let metadata_str: String = row.get("metadata");
        let created_at_str: String = row.get("created_at");
        let parse_uuid = |s: &str| Uuid::from_str(s).map_err(|e| IGKError::Internal(e.to_string()));

        Ok(Edge {
            edge_id: parse_uuid(&edge_id_str)?,
            from_node_id: parse_uuid(&from_str)?,
            to_node_id: parse_uuid(&to_str)?,
            edge_type: serde_json::from_str(&format!("\"{}\"", edge_type_str)).map_err(IGKError::Serialization)?,
            metadata: serde_json::from_str(&metadata_str).map_err(IGKError::Serialization)?,
            created_at: DateTime::parse_from_rfc3339(&created_at_str).map_err(|e| IGKError::Internal(e.to_string()))?.with_timezone(&Utc),
        })
    }

    pub async fn create_event(&self, event: &Event) -> Result<Event> {
        sqlx::query(
            r#"
            INSERT INTO events (
                event_id, timestamp, actor, action, target,
                before_state, after_state, policy_decision
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(event.event_id.to_string())
        .bind(event.timestamp.to_rfc3339())
        .bind(&event.actor)
        .bind(&event.action)
        .bind(&event.target)
        .bind(event.before_state.as_ref().map(|v| serde_json::to_string(v).unwrap()))
        .bind(event.after_state.as_ref().map(|v| serde_json::to_string(v).unwrap()))
        .bind(event.policy_decision.as_deref())
        .execute(&self.pool)
        .await
        .map_err(IGKError::Database)?;

        Ok(event.clone())
    }
}
