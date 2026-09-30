use super::external::*;
use serde_json::{json, Value};

pub(crate) fn tool_receipt(run: &str) -> Value {
    json!({
        "envelope": {"abi_version":"1.0.0", "schema_id":"allternit.kernel.ToolReceiptV1", "schema_version":"1.0.0",
            "run_id":run, "session_id":"session.test", "task_id":"task.test", "state_version":0, "trace_id":"trace.test",
            "created_at":"2026-09-30T12:00:00Z", "producer":{"component_id":"executor.test","component_version":"1","kind":"TOOL"}, "provenance":[{"source_type":"SYSTEM","source_id":"executor.test","trust_class":"INTERNAL"}]},
        "invocation_id":"inv.test", "tool_id":"read", "operation_id":"invoke", "started_at":"2026-09-30T12:00:00Z",
        "finished_at":"2026-09-30T12:00:01Z", "exit_class":"SUCCESS", "policy_receipt_id":"policy.test", "content_hashes":[]
    })
}

#[test]
fn wp10_external_schema_rejects_invalid_and_cross_run_receipts() {
    let good = tool_receipt("run.test");
    validate_tool_receipt("run.test", &good).unwrap();
    assert!(validate_tool_receipt("other.run", &good).is_err());
    for (pointer, value) in [
        ("/envelope/schema_id", json!("allternit.kernel.PolicyReceiptV1")),
        ("/envelope/abi_version", json!("1.1.0")),
        ("/envelope/run_id", json!("../escape")),
        ("/envelope/state_version", json!(-1)),
        ("/envelope/producer/kind", json!("UNKNOWN")),
        ("/started_at", json!("yesterday")),
        ("/finished_at", json!("2026-09-29T00:00:00Z")),
        ("/exit_class", json!("PASS")),
        ("/content_hashes", json!(["sha256:nope"])),
    ] {
        let mut bad = good.clone(); *bad.pointer_mut(pointer).unwrap() = value;
        assert!(validate_tool_receipt("run.test", &bad).is_err(), "accepted {pointer}");
    }
    for key in ["invocation_id", "policy_receipt_id", "content_hashes", "envelope"] {
        let mut bad = good.clone(); bad.as_object_mut().unwrap().remove(key);
        assert!(validate_tool_receipt("run.test", &bad).is_err());
    }
    let mut bad = good.clone(); bad["chain"] = json!({"signature":"forged"});
    assert!(validate_tool_receipt("run.test", &bad).is_err());
    bad = good; bad["extensions"] = json!({"not_namespaced": true});
    assert!(validate_tool_receipt("run.test", &bad).is_err());
}
