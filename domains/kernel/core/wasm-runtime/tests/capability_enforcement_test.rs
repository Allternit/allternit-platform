//! Capability enforcement tests for WASM runtime.

use allternit_history::HistoryLedger;
use allternit_wasm_runtime::host::ExecutionContext;
use allternit_wasm_runtime::{Capability, CapabilityGrant, ToolHostState};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

fn host_state(
    temp_dir: &TempDir,
    grant: CapabilityGrant,
) -> Result<ToolHostState, Box<dyn std::error::Error>> {
    let ledger_path = temp_dir.path().join("ledger.jsonl");
    let history_ledger = Arc::new(Mutex::new(HistoryLedger::new(&ledger_path)?));
    let context = ExecutionContext::new("test-tenant".to_string());
    Ok(ToolHostState::new(grant, context, history_ledger))
}

fn minimal_grant() -> CapabilityGrant {
    CapabilityGrant::minimal(
        "test-capsule".to_string(),
        "test-tenant".to_string(),
        "test-system".to_string(),
    )
}

#[test]
fn test_filesystem_read_capability_enforcement() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;

    let without_fs = host_state(&temp_dir, minimal_grant())?;
    assert!(!without_fs.check_capability("filesystem:read"));

    let with_fs = host_state(
        &temp_dir,
        minimal_grant().with_capability(Capability::FilesystemRead {
            paths: vec!["/tmp/test.txt".to_string()],
        }),
    )?;
    assert!(with_fs.check_capability("filesystem:read"));
    assert!(!with_fs.check_capability("filesystem:write"));

    Ok(())
}

#[test]
fn test_network_capability_enforcement() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;

    let without_network = host_state(&temp_dir, minimal_grant())?;
    assert!(!without_network.check_capability("network"));

    let with_network = host_state(
        &temp_dir,
        minimal_grant().with_capability(Capability::Network {
            allowed_hosts: vec!["example.com".to_string()],
            allowed_ports: vec![80, 443],
        }),
    )?;
    assert!(with_network.check_capability("network"));
    assert!(with_network.capability_set.can_access_host("example.com", 443));
    assert!(!with_network.capability_set.can_access_host("evil.com", 443));

    Ok(())
}

#[test]
fn test_http_client_capability_enforcement() -> Result<(), Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;

    let without_http = host_state(&temp_dir, minimal_grant())?;
    assert!(!without_http.check_capability("http_client"));

    let with_http = host_state(
        &temp_dir,
        minimal_grant().with_capability(Capability::HttpClient {
            allowed_hosts: vec!["httpbin.org".to_string()],
            max_requests_per_minute: 10,
        }),
    )?;
    assert!(with_http.check_capability("http_client"));
    assert!(!with_http.check_capability("network"));

    Ok(())
}
