//! Query language for the Rails CLI.
//!
//! Provides a small structured-query DSL over tickets, memories, echoes,
//! wait-gates, and merge locks.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::echoes::EchoStore;
use crate::memory::MemoryStore;
use crate::merge_locks::MergeLockStore;
use crate::tickets::{Ticket, TicketStore};
use crate::wait_gates::WaitGateStore;

/// Supported entities to query.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum QueryEntity {
    Tickets,
    Memories,
    Echoes,
    Gates,
    Locks,
}

impl std::fmt::Display for QueryEntity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryEntity::Tickets => write!(f, "tickets"),
            QueryEntity::Memories => write!(f, "memories"),
            QueryEntity::Echoes => write!(f, "echoes"),
            QueryEntity::Gates => write!(f, "gates"),
            QueryEntity::Locks => write!(f, "locks"),
        }
    }
}

/// A query condition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Condition {
    Eq(String, String),
    Ne(String, String),
    Contains(String, String),
    Gt(String, String),
    Lt(String, String),
    Gte(String, String),
    Lte(String, String),
}

impl Condition {
    /// Parse a single condition string like `status:open` or `priority>=p1`.
    pub fn parse(s: &str) -> Result<Self> {
        let ops = [("<=", "lte"), (">=", "gte"), (":", "eq"), ("!=", "ne"), (">", "gt"), ("<", "lt")];
        for (op, kind) in ops {
            if let Some(pos) = s.find(op) {
                let field = s[..pos].trim().to_string();
                let value = s[pos + op.len()..].trim().to_string();
                return match kind {
                    "eq" => Ok(Condition::Eq(field, value)),
                    "ne" => Ok(Condition::Ne(field, value)),
                    "contains" => Ok(Condition::Contains(field, value)),
                    "gt" => Ok(Condition::Gt(field, value)),
                    "lt" => Ok(Condition::Lt(field, value)),
                    "gte" => Ok(Condition::Gte(field, value)),
                    "lte" => Ok(Condition::Lte(field, value)),
                    _ => unreachable!(),
                };
            }
        }
        anyhow::bail!("invalid condition: {s}")
    }
}

/// Parsed query.
#[derive(Clone, Debug, Default)]
pub struct Query {
    pub conditions: Vec<Condition>,
}

impl Query {
    pub fn parse(input: &str) -> Result<Self> {
        let mut conditions = Vec::new();
        for token in input.split_whitespace() {
            if token.is_empty() {
                continue;
            }
            conditions.push(Condition::parse(token)?);
        }
        Ok(Self { conditions })
    }
}

/// Query result item.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum QueryResultItem {
    Ticket(Ticket),
    Memory(crate::memory::Memory),
    Echo(crate::echoes::Echo),
    Gate(crate::wait_gates::WaitGate),
    Lock(crate::merge_locks::MergeLock),
    Other(serde_json::Value),
}

/// Query executor.
pub struct QueryEngine<'a> {
    root: &'a Path,
}

impl<'a> QueryEngine<'a> {
    pub fn new(root: &'a Path) -> Self {
        Self { root }
    }

    /// Execute a query against the given entity.
    pub fn execute(&self, entity: QueryEntity, query: &Query) -> Result<Vec<QueryResultItem>> {
        match entity {
            QueryEntity::Tickets => self.query_tickets(query),
            QueryEntity::Memories => self.query_memories(query),
            QueryEntity::Echoes => self.query_echoes(query),
            QueryEntity::Gates => self.query_gates(query),
            QueryEntity::Locks => self.query_locks(query),
        }
    }

    fn query_tickets(&self, query: &Query) -> Result<Vec<QueryResultItem>> {
        let store = TicketStore::new(self.root)?;
        let mut results = Vec::new();
        for ticket in store.list()? {
            if matches_ticket(query, &ticket) {
                results.push(QueryResultItem::Ticket(ticket));
            }
        }
        Ok(results)
    }

    fn query_memories(&self, query: &Query) -> Result<Vec<QueryResultItem>> {
        let store = MemoryStore::new(self.root)?;
        let mut results = Vec::new();
        for memory in store.list(None)? {
            if matches_memory(query, &memory) {
                results.push(QueryResultItem::Memory(memory));
            }
        }
        Ok(results)
    }

    fn query_echoes(&self, query: &Query) -> Result<Vec<QueryResultItem>> {
        let store = EchoStore::new(self.root)?;
        let mut results = Vec::new();
        for echo in store.list(true)? {
            if matches_echo(query, &echo) {
                results.push(QueryResultItem::Echo(echo));
            }
        }
        Ok(results)
    }

    fn query_gates(&self, query: &Query) -> Result<Vec<QueryResultItem>> {
        let store = WaitGateStore::new(self.root)?;
        let mut results = Vec::new();
        // List gates across all tickets.
        let ticket_store = TicketStore::new(self.root)?;
        for ticket in ticket_store.list()? {
            for gate in store.for_ticket(&ticket.id, true)? {
                if matches_gate(query, &gate) {
                    results.push(QueryResultItem::Gate(gate));
                }
            }
        }
        Ok(results)
    }

    fn query_locks(&self, query: &Query) -> Result<Vec<QueryResultItem>> {
        let store = MergeLockStore::new(self.root)?;
        let mut results = Vec::new();
        for lock in store.list(true)? {
            if matches_lock(query, &lock) {
                results.push(QueryResultItem::Lock(lock));
            }
        }
        Ok(results)
    }
}

fn matches_ticket(query: &Query, ticket: &Ticket) -> bool {
    for cond in &query.conditions {
        let ok = match cond {
            Condition::Eq(field, value) => match field.as_str() {
                "id" => ticket.id.to_string() == *value,
                "title" => ticket.title == *value,
                "status" => format!("{:?}", ticket.status).to_lowercase() == value.to_lowercase(),
                "kind" => format!("{:?}", ticket.kind).to_lowercase() == value.to_lowercase(),
                "priority" => ticket.priority.to_string() == value.to_uppercase(),
                "assignee" => ticket.assignee.as_deref().unwrap_or("") == value,
                _ => false,
            },
            Condition::Ne(field, value) => match field.as_str() {
                "status" => format!("{:?}", ticket.status).to_lowercase() != value.to_lowercase(),
                "kind" => format!("{:?}", ticket.kind).to_lowercase() != value.to_lowercase(),
                "priority" => ticket.priority.to_string() != value.to_uppercase(),
                _ => true,
            },
            Condition::Contains(field, value) => match field.as_str() {
                "title" => ticket.title.to_lowercase().contains(&value.to_lowercase()),
                "description" => ticket.description.to_lowercase().contains(&value.to_lowercase()),
                "label" => ticket.labels.iter().any(|l| l.to_lowercase() == value.to_lowercase()),
                _ => false,
            },
            Condition::Gt(field, value) | Condition::Gte(field, value) | Condition::Lt(field, value) | Condition::Lte(field, value) => {
                if field == "priority" {
                    let ticket_level = ticket.priority.level();
                    let query_level = parse_priority_level(value);
                    match cond {
                        Condition::Gt(_, _) => ticket_level > query_level,
                        Condition::Gte(_, _) => ticket_level >= query_level,
                        Condition::Lt(_, _) => ticket_level < query_level,
                        Condition::Lte(_, _) => ticket_level <= query_level,
                        _ => false,
                    }
                } else {
                    false
                }
            }
        };
        if !ok {
            return false;
        }
    }
    true
}

fn matches_memory(query: &Query, memory: &crate::memory::Memory) -> bool {
    for cond in &query.conditions {
        let ok = match cond {
            Condition::Eq(field, value) => match field.as_str() {
                "id" => memory.id == *value,
                "tag" => memory.tags.contains(value),
                _ => false,
            },
            Condition::Contains(field, value) => match field.as_str() {
                "content" => memory.content.to_lowercase().contains(&value.to_lowercase()),
                _ => false,
            },
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

fn matches_echo(query: &Query, echo: &crate::echoes::Echo) -> bool {
    for cond in &query.conditions {
        let ok = match cond {
            Condition::Eq(field, value) => match field.as_str() {
                "id" => echo.id.to_string() == *value,
                "kind" => format!("{:?}", echo.kind).to_lowercase() == value.to_lowercase(),
                _ => false,
            },
            Condition::Contains(field, value) => match field.as_str() {
                "content" => echo.content.to_lowercase().contains(&value.to_lowercase()),
                _ => false,
            },
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

fn matches_gate(query: &Query, gate: &crate::wait_gates::WaitGate) -> bool {
    for cond in &query.conditions {
        let ok = match cond {
            Condition::Eq(field, value) => match field.as_str() {
                "id" => gate.id == *value,
                "ticket" => gate.ticket_id.to_string() == *value,
                "kind" => format!("{:?}", gate.kind).to_lowercase() == value.to_lowercase(),
                "resolved" => gate.outcome.is_some().to_string() == *value,
                _ => false,
            },
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

fn parse_priority_level(value: &str) -> u8 {
    let normalized = value.trim().to_uppercase();
    if normalized.starts_with('P') {
        normalized[1..].parse::<u8>().unwrap_or(4)
    } else {
        normalized.parse::<u8>().unwrap_or(4)
    }
}

fn matches_lock(query: &Query, lock: &crate::merge_locks::MergeLock) -> bool {
    for cond in &query.conditions {
        let ok = match cond {
            Condition::Eq(field, value) => match field.as_str() {
                "id" => lock.id == *value,
                "domain" => lock.domain == *value,
                "owner" => lock.owner == *value,
                _ => false,
            },
            Condition::Contains(field, value) => match field.as_str() {
                "domain" => lock.domain.to_lowercase().contains(&value.to_lowercase()),
                _ => false,
            },
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rails_id::TicketId;
    use crate::tickets::{TicketKind, TicketPriority, TicketStatus};
    use std::collections::HashMap;
    use tempfile::TempDir;

    #[test]
    fn query_tickets_by_status() {
        let tmp = TempDir::new().unwrap();
        let store = TicketStore::new(tmp.path()).unwrap();
        store
            .create(Ticket {
                id: TicketId::mint("a"),
                hierarchical_id: crate::rails_id::HierarchicalId::root(TicketId::mint("a")),
                title: "Open ticket".to_string(),
                description: "".to_string(),
                design: None,
                acceptance: None,
                notes: Vec::new(),
                status: TicketStatus::Open,
                kind: TicketKind::Task,
                priority: TicketPriority::P1,
                assignee: None,
                estimate_minutes: None,
                due_at: None,
                defer_until: None,
                labels: Vec::new(),
                external_ref: None,
                metadata: HashMap::new(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                closed_at: None,
                close_reason: None,
            })
            .unwrap();

        let engine = QueryEngine::new(tmp.path());
        let query = Query::parse("status:open").unwrap();
        let results = engine.execute(QueryEntity::Tickets, &query).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn parse_conditions() {
        let query = Query::parse("status:open kind:bug priority>=p1").unwrap();
        assert_eq!(query.conditions.len(), 3);
    }
}
