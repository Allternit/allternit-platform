#[cfg(test)]
mod v2_memory_system_tests {
    use allternit_memory::storage::{MemoryStorage, SqliteMemoryStorage};
    use allternit_memory::v2::conflict::{choose_strategy, ConflictStrategy};
    use allternit_memory::v2::truth_engine::TruthEngine;
    use allternit_memory::v2::types::{
        MemoryAuthority as V2MemoryAuthority, MemoryStatus as V2MemoryStatus, MemoryTimeline,
    };
    use allternit_memory::{
        ConsolidationState, ConsolidationTrigger, DeletionPolicy, MemoryDecayFunction, MemoryEntry,
        MemoryProvenance, MemoryRetentionPolicy, MemoryType,
    };
    use serde_json::json;
    use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_truth_preservation_supersedes_not_overwrites() {
        use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
        use std::time::Duration;

        // Create an in-memory SQLite database for testing
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        // Initialize the schema
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS memory_entries (
                memory_id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                session_id TEXT,
                agent_id TEXT,
                memory_type TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                last_accessed INTEGER NOT NULL,
                access_count INTEGER NOT NULL DEFAULT 0,
                sensitivity_tier INTEGER NOT NULL,
                tags TEXT NOT NULL,
                provenance TEXT NOT NULL,
                retention_policy TEXT NOT NULL,
                consolidation_state TEXT NOT NULL,
                created_at_index INTEGER NOT NULL,
                last_accessed_index INTEGER NOT NULL,
                status TEXT DEFAULT 'active',
                valid_from INTEGER,
                valid_to INTEGER,
                confidence REAL DEFAULT 0.75,
                authority TEXT DEFAULT 'agent',
                supersedes_memory_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_memory_status ON memory_entries(status)")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_memory_valid_from ON memory_entries(valid_from)",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_memory_valid_to ON memory_entries(valid_to)")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_memory_supersedes ON memory_entries(supersedes_memory_id)")
            .execute(&pool)
            .await
            .unwrap();

        // Create storage instance
        let storage = SqliteMemoryStorage::new(pool).await.unwrap();

        // Insert initial employment memory
        let initial_memory = MemoryEntry {
            id: "test_id".to_string(),
            memory_id: "emp_123".to_string(),
            tenant_id: "tenant_1".to_string(),
            session_id: Some("session_1".to_string()),
            agent_id: Some("agent_1".to_string()),
            memory_type: MemoryType::Declarative,
            content: json!({"job_title": "Software Engineer", "company": "Old Company"}),
            metadata: json!({}),
            embedding: None,
            created_at: 1000,
            last_accessed: 1000,
            access_count: 0,
            sensitivity_tier: 1,
            tags: vec!["employment".to_string()],
            provenance: MemoryProvenance {
                source_session: Some("session_1".to_string()),
                source_agent: "agent_1".to_string(),
                derivation_chain: vec![],
                integrity_hash: "hash1".to_string(),
                signature: None,
            },
            retention_policy: MemoryRetentionPolicy {
                time_to_live: None,
                max_accesses: None,
                decay_function: MemoryDecayFunction::Linear { rate: 0.1 },
                consolidation_trigger: ConsolidationTrigger::Manual,
                deletion_policy: DeletionPolicy::Immediate,
            },
            consolidation_state: ConsolidationState::Raw,
            status: "active".to_string(),
            valid_from: Some(1000),
            valid_to: None,
            confidence: 0.8,
            authority: "agent".to_string(),
            supersedes_memory_id: None,
        };

        // Store the initial memory using v2 semantics
        storage
            .store_memory_v2(
                &initial_memory.tenant_id,
                &initial_memory.memory_id,
                &format!("{:?}", initial_memory.memory_type),
                &initial_memory.content,
                &initial_memory.metadata,
                &initial_memory.tags,
                1000,
            )
            .await
            .unwrap();

        // Verify the initial memory exists
        let retrieved_initial = storage.retrieve_memory("emp_123").await.unwrap().unwrap();
        assert_eq!(retrieved_initial.status, "active");
        assert_eq!(
            retrieved_initial.content,
            json!({"job_title": "Software Engineer", "company": "Old Company"})
        );

        // Now store updated employment memory (should supersede, not overwrite)
        let updated_memory = MemoryEntry {
            id: "test_id_2".to_string(),
            memory_id: "emp_123".to_string(), // Same ID to trigger conflict resolution
            tenant_id: "tenant_1".to_string(),
            session_id: Some("session_1".to_string()),
            agent_id: Some("agent_1".to_string()),
            memory_type: MemoryType::Declarative,
            content: json!({"job_title": "Senior Software Engineer", "company": "New Company"}),
            metadata: json!({}),
            embedding: None,
            created_at: 2000,
            last_accessed: 2000,
            access_count: 0,
            sensitivity_tier: 1,
            tags: vec!["employment".to_string()],
            provenance: MemoryProvenance {
                source_session: Some("session_1".to_string()),
                source_agent: "agent_1".to_string(),
                derivation_chain: vec![],
                integrity_hash: "hash2".to_string(),
                signature: None,
            },
            retention_policy: MemoryRetentionPolicy {
                time_to_live: None,
                max_accesses: None,
                decay_function: MemoryDecayFunction::Linear { rate: 0.1 },
                consolidation_trigger: ConsolidationTrigger::Manual,
                deletion_policy: DeletionPolicy::Immediate,
            },
            consolidation_state: ConsolidationState::Raw,
            status: "active".to_string(),
            valid_from: Some(2000),
            valid_to: None,
            confidence: 0.85,
            authority: "agent".to_string(),
            supersedes_memory_id: None,
        };

        // Store the updated memory using the v2 method
        storage
            .store_memory_v2(
                &updated_memory.tenant_id,
                &updated_memory.memory_id,
                &format!("{:?}", updated_memory.memory_type),
                &updated_memory.content,
                &updated_memory.metadata,
                &updated_memory.tags,
                2000,
            )
            .await
            .unwrap();

        // Now we should have both the old (superseded) and new (active) versions.
        // query_memory returns only active rows unless a status is named, so
        // query each status the test expects. Both versions were written through
        // store_memory_v2, which takes no session or agent, so don't filter on them.
        let query_status = |status: &str| allternit_memory::storage::MemoryQuery {
            tenant_id: "tenant_1".to_string(),
            session_id: None,
            agent_id: None,
            memory_types: vec![MemoryType::Declarative],
            max_sensitivity_tier: Some(5),
            required_tags: vec!["employment".to_string()],
            time_range: None,
            content_search: None,
            limit: None,
            status_filter: Some(status.to_string()),
        };
        let mut all_memories = Vec::new();
        for status in ["active", "superseded"] {
            all_memories.extend(storage.query_memory(&query_status(status)).await.unwrap());
        }

        // Should have 2 memories: the original (superseded) and the new (active)
        assert_eq!(all_memories.len(), 2);

        // Find the active and superseded memories
        let active_memory = all_memories.iter().find(|m| m.status == "active").unwrap();
        let superseded_memory = all_memories
            .iter()
            .find(|m| m.status == "superseded")
            .unwrap();

        // Verify the active memory has the new content
        assert_eq!(
            active_memory.content,
            json!({"job_title": "Senior Software Engineer", "company": "New Company"})
        );
        assert_eq!(active_memory.valid_from, Some(2000));
        assert!(active_memory.supersedes_memory_id.is_some()); // Should reference the old one

        // Verify the superseded memory has the old content
        assert_eq!(
            superseded_memory.content,
            json!({"job_title": "Software Engineer", "company": "Old Company"})
        );
        assert_eq!(superseded_memory.valid_to, Some(2000)); // Should be marked as superseded at time 2000
    }

    #[tokio::test]
    async fn test_decay_archives_inactive_memories_without_deleting() {
        use allternit_memory::storage::SqliteMemoryStorage;
        use allternit_memory::v2::decay::{decay_status, DecayPolicy};
        use allternit_memory::v2::types::MemoryStatus as V2MemoryStatus;
        use allternit_memory::{
            ConsolidationState, ConsolidationTrigger, DeletionPolicy, MemoryDecayFunction,
            MemoryEntry, MemoryProvenance, MemoryRetentionPolicy, MemoryType,
        };
        use serde_json::json;
        use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

        // Create an in-memory SQLite database for testing
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        // Initialize the schema
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS memory_entries (
                memory_id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                session_id TEXT,
                agent_id TEXT,
                memory_type TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                last_accessed INTEGER NOT NULL,
                access_count INTEGER NOT NULL DEFAULT 0,
                sensitivity_tier INTEGER NOT NULL,
                tags TEXT NOT NULL,
                provenance TEXT NOT NULL,
                retention_policy TEXT NOT NULL,
                consolidation_state TEXT NOT NULL,
                created_at_index INTEGER NOT NULL,
                last_accessed_index INTEGER NOT NULL,
                status TEXT DEFAULT 'active',
                valid_from INTEGER,
                valid_to INTEGER,
                confidence REAL DEFAULT 0.75,
                authority TEXT DEFAULT 'agent',
                supersedes_memory_id TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Create storage instance
        let storage = SqliteMemoryStorage::new(pool).await.unwrap();

        // Insert a memory with low access count and old last_accessed time
        let old_memory = MemoryEntry {
            id: "test_id".to_string(),
            memory_id: "old_mem_123".to_string(),
            tenant_id: "tenant_1".to_string(),
            session_id: Some("session_1".to_string()),
            agent_id: Some("agent_1".to_string()),
            memory_type: MemoryType::Episodic,
            content: json!({"event": "meeting", "date": "long ago"}),
            metadata: json!({}),
            embedding: None,
            created_at: 1000,
            last_accessed: 1000, // Very old access time
            access_count: 1,     // Low access count
            sensitivity_tier: 1,
            tags: vec!["event".to_string()],
            provenance: MemoryProvenance {
                source_session: Some("session_1".to_string()),
                source_agent: "agent_1".to_string(),
                derivation_chain: vec![],
                integrity_hash: "hash1".to_string(),
                signature: None,
            },
            retention_policy: MemoryRetentionPolicy {
                time_to_live: None,
                max_accesses: None,
                decay_function: MemoryDecayFunction::Linear { rate: 0.1 },
                consolidation_trigger: ConsolidationTrigger::Manual,
                deletion_policy: DeletionPolicy::Immediate,
            },
            consolidation_state: ConsolidationState::Raw,
            status: "active".to_string(),
            valid_from: Some(1000),
            valid_to: None,
            confidence: 0.7,
            authority: "agent".to_string(),
            supersedes_memory_id: None,
        };

        storage.store_memory(&old_memory).await.unwrap();

        // Simulate decay process: check if memory should be archived
        let now = 1000 + (91 * 86_400); // 91 days later (past the 90-day threshold)
        let policy = DecayPolicy::default(); // active_to_archived_days: 90

        // Convert the status to the v2 enum for testing
        let current_status = match old_memory.status.as_str() {
            "active" => V2MemoryStatus::Active,
            "superseded" => V2MemoryStatus::Superseded,
            "archived" => V2MemoryStatus::Archived,
            _ => V2MemoryStatus::Active,
        };

        let new_status = decay_status(
            now,
            &current_status,
            old_memory.last_accessed,
            old_memory.access_count,
            &policy,
        );

        // Since access_count is only 1 (less than min_access_keep_active: 5) and
        // it's been 91 days (> 90 days), it should be marked for archival
        assert!(new_status.is_some());
        assert_eq!(new_status.unwrap(), V2MemoryStatus::Archived);

        // Update the memory status to archived
        let mut archived_memory = old_memory.clone();
        archived_memory.status = "archived".to_string();

        storage.update_memory(&archived_memory).await.unwrap();

        // Verify the memory is now archived
        let retrieved = storage
            .retrieve_memory("old_mem_123")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.status, "archived");
    }

    #[test]
    fn test_conflict_resolution_strategies() {
        // Test that different tags and memory types get appropriate conflict strategies
        let employment_tags = vec!["employment".to_string()];
        let belief_tags = vec!["belief".to_string()];
        let opinion_tags = vec!["opinion".to_string()];
        let location_tags = vec!["location".to_string()];

        // Employment should use TemporalShift strategy
        let emp_strategy = choose_strategy("Declarative", &employment_tags);
        assert_eq!(emp_strategy, ConflictStrategy::TemporalShift);

        // Beliefs should use ParallelTruths strategy
        let belief_strategy = choose_strategy("Semantic", &belief_tags);
        assert_eq!(belief_strategy, ConflictStrategy::ParallelTruths);

        // Opinions should use ParallelTruths strategy
        let opinion_strategy = choose_strategy("Semantic", &opinion_tags);
        assert_eq!(opinion_strategy, ConflictStrategy::ParallelTruths);

        // Location should use TemporalShift strategy
        let location_strategy = choose_strategy("Declarative", &location_tags);
        assert_eq!(location_strategy, ConflictStrategy::TemporalShift);

        // Default for Semantic/Declarative should be OverwriteWithArchive
        let default_semantic = choose_strategy("Semantic", &vec![]);
        assert_eq!(default_semantic, ConflictStrategy::OverwriteWithArchive);

        // Default for Episodic/Working should be TemporalShift
        let default_episodic = choose_strategy("Episodic", &vec![]);
        assert_eq!(default_episodic, ConflictStrategy::TemporalShift);
    }

    #[test]
    fn test_context_routing_logic() {
        use allternit_memory::v2::context_tree::{decide, ContextRoute};

        // Test explicit recall request
        let recall_decision = decide("What did I tell you about the project?");
        assert_eq!(recall_decision.route, ContextRoute::SummariesThenItems);

        // Test relationship query
        let relationship_decision = decide("What is the relationship between A and B?");
        assert_eq!(relationship_decision.route, ContextRoute::GraphAndItems);

        let relationship_decision2 = decide("How are these concepts connected?");
        assert_eq!(relationship_decision2.route, ContextRoute::GraphAndItems);

        // Test default behavior
        let default_decision = decide("Calculate the sum of these numbers");
        assert_eq!(default_decision.route, ContextRoute::SummariesOnly);
    }

    #[test]
    fn test_memory_timeline_creation() {
        use allternit_memory::v2::types::{MemoryAuthority, MemoryStatus, MemoryTimeline};

        let now = 1234567890;
        let timeline = MemoryTimeline::new_active(now, MemoryAuthority::User, 0.9);

        assert_eq!(timeline.status, MemoryStatus::Active);
        assert_eq!(timeline.valid_from, now);
        assert_eq!(timeline.valid_to, None);
        assert_eq!(timeline.confidence, 0.9);
        assert_eq!(timeline.authority, MemoryAuthority::User);
        assert_eq!(timeline.supersedes_memory_id, None);
    }
}
