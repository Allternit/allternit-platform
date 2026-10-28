#[cfg(test)]
mod tests {
    use crate::schema::{Node, Edge, NodeType, EdgeType};
    use crate::storage::Storage;
    use crate::query::QueryEngine;
    use crate::projection::ProjectionEngine;
    use crate::mutation::MutationEngine;
    use crate::error::Result;
    use uuid::Uuid;
    use chrono::Utc;

    #[tokio::test]
    async fn test_node_crud() -> Result<()> {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?;
        let storage = Storage::new(pool.clone());
        storage.init().await?;
        let query = QueryEngine::new(storage.clone());
        let mutation = MutationEngine::new(storage);

        let node = Node {
            node_id: Uuid::new_v4(),
            node_type: NodeType::Task,
            status: crate::schema::NodeStatus::Active,
            priority: 10,
            owner: "test".to_string(),
            source_refs: vec![],
            attributes: serde_json::json!({"test": "data"}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        mutation.commit_create_node(&node, "test", None).await?;

        let fetched = query.get_node(&node.node_id).await?;
        assert_eq!(fetched.node_id, node.node_id);
        assert_eq!(fetched.priority, 10);

        Ok(())
    }

    #[tokio::test]
    async fn test_subgraph_retrieval() -> Result<()> {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?;
        let storage = Storage::new(pool.clone());
        storage.init().await?;
        let query = QueryEngine::new(storage.clone());

        let node1 = Node {
            node_id: Uuid::new_v4(),
            node_type: NodeType::Task,
            status: crate::schema::NodeStatus::Active,
            priority: 10,
            owner: "test".to_string(),
            source_refs: vec![],
            attributes: serde_json::json!({}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let node2 = Node {
            node_id: Uuid::new_v4(),
            node_type: NodeType::Goal,
            status: crate::schema::NodeStatus::Active,
            priority: 20,
            owner: "test".to_string(),
            source_refs: vec![],
            attributes: serde_json::json!({}),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        storage.create_node(&node1).await?;
        storage.create_node(&node2).await?;

        let edge = Edge {
            edge_id: Uuid::new_v4(),
            from_node_id: node1.node_id,
            to_node_id: node2.node_id,
            edge_type: EdgeType::DependsOn,
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
        };

        storage.create_edge(&edge).await?;

        let subgraph = query.get_subgraph(&node1.node_id, Some(1)).await?;

        assert_eq!(subgraph.1.len(), 1);
        assert_eq!(subgraph.1[0].edge_id, edge.edge_id);

        Ok(())
    }
}
