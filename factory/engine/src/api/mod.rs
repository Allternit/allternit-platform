//! Engine surfaces: the HTTP service, MCP, workspace-service client and the CLI
//! command implementations (SPEC §5 `surfaces/api`).

pub mod cli;
pub mod factory;
pub mod mcp;
pub mod peer;
pub mod service;
pub mod workspace;
