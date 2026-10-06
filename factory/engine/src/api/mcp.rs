//! Model Context Protocol (MCP) server for the Rails CLI.
//!
//! Exposes Rails operations as MCP tools so agents can interact with the
//! ticket/DAG workflow through a standardized protocol.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::dependencies::load_graph;
use crate::killswitch::KillSwitch;
use crate::memory::MemoryStore;
use crate::rails_id::TicketId;
use crate::tickets::{self, Ticket, TicketKind, TicketPriority, TicketStatus, TicketStore};
use crate::wait_gates::WaitGateStore;

/// A JSON-RPC request from an MCP client.
#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

/// A JSON-RPC response.
#[derive(Clone, Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Clone, Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

/// MCP server state.
pub struct McpServer<'a> {
    root: &'a Path,
}

impl<'a> McpServer<'a> {
    pub fn new(root: &'a Path) -> Self {
        Self { root }
    }

    fn check_kill_switch(&self) -> Result<(), String> {
        KillSwitch::load(self.root)
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|e| e.to_string())
    }

    /// Run the server over stdin/stdout.
    pub fn run_stdio(&self) -> Result<()> {
        let stdin = io::stdin();
        let mut stdout = io::stdout().lock();
        let mut stderr = io::stderr().lock();

        for line in stdin.lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let response = match serde_json::from_str::<JsonRpcRequest>(&line) {
                Ok(req) => self.handle(req),
                Err(e) => JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: None,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32700,
                        message: format!("parse error: {e}"),
                        data: None,
                    }),
                },
            };

            let out = serde_json::to_string(&response)?;
            writeln!(stdout, "{out}").context("failed to write response")?;
            stdout.flush()?;
            writeln!(stderr, "mcp: handled request").ok();
        }

        Ok(())
    }

    fn handle(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        let result = match req.method.as_str() {
            "initialize" => Ok(serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "serverInfo": { "name": "rails-mcp", "version": "0.1.0" }
            })),
            "tools/list" => Ok(self.list_tools()),
            "tools/call" => self.call_tool(req.params),
            _ => Err(format!("method not found: {}", req.method)),
        };

        match result {
            Ok(result) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(result),
                error: None,
            },
            Err(message) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32601,
                    message,
                    data: None,
                }),
            },
        }
    }

    fn list_tools(&self) -> Value {
        serde_json::json!({
            "tools": [
                {
                    "name": "rails_ticket_new",
                    "description": "Create a new Rails ticket",
                    "inputSchema": {
                        "type": "object",
                        "required": ["title"],
                        "properties": {
                            "title": { "type": "string" },
                            "description": { "type": "string" },
                            "priority": { "type": "string", "enum": ["P0", "P1", "P2", "P3", "P4"] },
                            "kind": { "type": "string", "enum": ["task", "bug", "feature", "epic", "chore", "decision"] }
                        }
                    }
                },
                {
                    "name": "rails_ticket_list",
                    "description": "List open Rails tickets",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "all": { "type": "boolean" }
                        }
                    }
                },
                {
                    "name": "rails_ticket_close",
                    "description": "Close a Rails ticket",
                    "inputSchema": {
                        "type": "object",
                        "required": ["id"],
                        "properties": {
                            "id": { "type": "string" },
                            "reason": { "type": "string" }
                        }
                    }
                },
                {
                    "name": "rails_ready",
                    "description": "Get the list of ready tickets",
                    "inputSchema": { "type": "object" }
                },
                {
                    "name": "rails_memory_learn",
                    "description": "Store a persistent memory",
                    "inputSchema": {
                        "type": "object",
                        "required": ["content"],
                        "properties": {
                            "content": { "type": "string" },
                            "tags": { "type": "array", "items": { "type": "string" } }
                        }
                    }
                },
                {
                    "name": "rails_memory_brief",
                    "description": "Generate a brief from memories",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "tags": { "type": "array", "items": { "type": "string" } },
                            "limit": { "type": "integer" }
                        }
                    }
                }
            ]
        })
    }

    fn call_tool(&self, params: Option<Value>) -> Result<Value, String> {
        let params = params.ok_or("missing params")?;
        let name = params["name"].as_str().ok_or("missing tool name")?;
        let args = params["arguments"].as_object().cloned().unwrap_or_default();

        match name {
            "rails_ticket_new" => self.tool_ticket_new(args),
            "rails_ticket_list" => self.tool_ticket_list(args),
            "rails_ticket_close" => self.tool_ticket_close(args),
            "rails_ready" => self.tool_ready(),
            "rails_memory_learn" => self.tool_memory_learn(args),
            "rails_memory_brief" => self.tool_memory_brief(args),
            _ => Err(format!("unknown tool: {name}")),
        }
    }

    fn tool_ticket_new(&self, args: serde_json::Map<String, Value>) -> Result<Value, String> {
        self.check_kill_switch()?;
        let store = TicketStore::new(self.root).map_err(|e| e.to_string())?;
        let title = args["title"].as_str().ok_or("missing title")?.to_string();
        let description = args.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let priority = args.get("priority")
            .and_then(|v| v.as_str())
            .unwrap_or("P2")
            .parse::<TicketPriority>()
            .map_err(|e| e.to_string())?;
        let kind = args.get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("task")
            .parse::<TicketKind>()
            .map_err(|e| e.to_string())?;

        let id = TicketId::mint(format!("{title}:{}", Utc::now()).as_bytes());
        let ticket = Ticket {
            id,
            hierarchical_id: crate::rails_id::HierarchicalId::root(TicketId::mint(title.as_bytes())),
            title,
            description,
            design: None,
            acceptance: None,
            notes: Vec::new(),
            status: TicketStatus::Open,
            kind,
            priority,
            assignee: None,
            estimate_minutes: None,
            due_at: None,
            defer_until: None,
            labels: Vec::new(),
            external_ref: None,
            metadata: HashMap::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            closed_at: None,
            close_reason: None,
        };
        let ticket = store.create(ticket).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "ticket": ticket }))
    }

    fn tool_ticket_list(&self, args: serde_json::Map<String, Value>) -> Result<Value, String> {
        let store = TicketStore::new(self.root).map_err(|e| e.to_string())?;
        let all = args.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
        let tickets: Vec<_> = store.list().map_err(|e| e.to_string())?
            .into_iter()
            .filter(|t| all || t.status != TicketStatus::Closed)
            .collect();
        Ok(serde_json::json!({ "tickets": tickets }))
    }

    fn tool_ticket_close(&self, args: serde_json::Map<String, Value>) -> Result<Value, String> {
        self.check_kill_switch()?;
        let store = TicketStore::new(self.root).map_err(|e| e.to_string())?;
        let id = args["id"].as_str().ok_or("missing id")?;
        let id = id.parse::<TicketId>().map_err(|e| e.to_string())?;
        let reason = args.get("reason").and_then(|v| v.as_str()).map(|s| s.to_string());
        let ticket = store.set_status(&id, TicketStatus::Closed, "mcp", reason)
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "ticket": ticket }))
    }

    fn tool_ready(&self) -> Result<Value, String> {
        let store = TicketStore::new(self.root).map_err(|e| e.to_string())?;
        let graph = load_graph(self.root).map_err(|e| e.to_string())?;
        let gates = WaitGateStore::new(self.root).map_err(|e| e.to_string())?;
        let tickets_list = store.list().map_err(|e| e.to_string())?;
        let ready = tickets::ready(&tickets_list, &graph, &gates, Utc::now())
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "ready": ready }))
    }

    fn tool_memory_learn(&self, args: serde_json::Map<String, Value>) -> Result<Value, String> {
        self.check_kill_switch()?;
        let store = MemoryStore::new(self.root).map_err(|e| e.to_string())?;
        let content = args["content"].as_str().ok_or("missing content")?.to_string();
        let tags: Vec<String> = args.get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let memory = store.learn(content, tags).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "memory": memory }))
    }

    fn tool_memory_brief(&self, args: serde_json::Map<String, Value>) -> Result<Value, String> {
        let store = MemoryStore::new(self.root).map_err(|e| e.to_string())?;
        let tags: Vec<String> = args.get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
        let brief = store.brief(&tags, limit).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "brief": brief }))
    }
}

