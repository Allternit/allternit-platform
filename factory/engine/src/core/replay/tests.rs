use super::*;
use crate::receipts::chain::{EffectContext, EffectOutcome, EffectRequest};
use crate::receipts::jcs::sha256_tagged;
use crate::receipts::sign::ReceiptSigner;
use crate::receipts::{ReceiptStore, ReceiptStoreOptions};
use std::cell::Cell;

const ABI: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../spec/Contracts/kernel/v1/conformance/examples");

fn store() -> (tempfile::TempDir, ChainStore) {
    let d = tempfile::tempdir().unwrap();
    let s = ChainStore::new(d.path(), ReceiptSigner::generate()).unwrap();
    (d, s)
}

fn cx(run: &str, node: &str) -> EffectContext {
    EffectContext { run_id: run.into(), session_id: "s1".into(), task_id: "t1".into(), node_id: Some(node.into()),
        trace_id: "tr1".into(), state_version: 1, producer_id: "allternit-factory".into(), policy_decision_id: "dec1".into() }
}

fn args(n: u32) -> Value { json!({"path": format!("f{n}.txt"), "content": n}) }

fn req(n: u32, key: &str) -> EffectRequest {
    EffectRequest { action_id: format!("act{n}"), tool_id: "tool.fs_write".into(),
        args_hash: hash_value(&args(n)).unwrap(), idempotency_key: key.into(),
        effect_class: "WORKSPACE_WRITE".into(), target: None }
}

fn step(n: u32) -> ReplayStep {
    ReplayStep::tool(&format!("node{n}"), "tool.fs_write", &args(n), "WORKSPACE_WRITE").unwrap()
}

/// A live run: policy decision, then three side-effecting tool calls.
fn live_run(s: &ChainStore, run: &str, calls: &Cell<u32>) {
    s.append(json!({"envelope": {"schema_id": "allternit.kernel.PolicyReceiptV1", "schema_version": "1.0.0",
        "run_id": run, "node_id": "gate"}, "decision": "ALLOW"})).unwrap();
    for n in 0..3 {
        let o = s.run_effect_once(&cx(run, &format!("node{n}")), &req(n, &format!("idem-key-{n}")), || {
            calls.set(calls.get() + 1);
            Ok((sha256_tagged(format!("out{n}").as_bytes()), Some(format!("ext{n}"))))
        }).unwrap();
        assert!(matches!(o, EffectOutcome::Executed(_)));
    }
}

fn policy_step() -> ReplayStep {
    ReplayStep { boundary: Boundary::Policy, node_id: "gate".into(),
        request_hash: boundary_request_hash(Boundary::Policy, "gate", "allternit.kernel.PolicyReceiptV1").unwrap(),
        branch: Some("ALLOW".into()), result_hash: None, idempotency_key: None }
}

fn rp(d: &tempfile::TempDir, run: &str, i: u64) -> PathBuf { d.path().join(format!("_chains/{run}/{i:010}.json")) }

#[test]
fn replay_abi_types_match_conformance_examples() {
    for (name, ok) in [("valid/CassetteV1", true), ("invalid/CassetteV1.unknown_field", false)] {
        let t = std::fs::read_to_string(format!("{ABI}/{name}.json")).unwrap();
        let r = serde_json::from_str::<CassetteV1>(&t);
        assert_eq!(r.is_ok(), ok, "{name}: {r:?}");
        if let Ok(c) = r {
            assert_eq!(serde_json::to_value(&c).unwrap(), serde_json::from_str::<Value>(&t).unwrap());
        }
    }
    for (name, ok) in [("valid/DivergenceReportV1", true), ("invalid/DivergenceReportV1.unknown_field", false)] {
        let t = std::fs::read_to_string(format!("{ABI}/{name}.json")).unwrap();
        assert_eq!(serde_json::from_str::<DivergenceReportV1>(&t).is_ok(), ok, "{name}");
    }
}

#[test]
fn replay_record_builds_abi_cassette_from_receipts() {
    let (_d, s) = store();
    let calls = Cell::new(0);
    live_run(&s, "run1", &calls);
    let c = record_cassette(&s, "run1", Some("graph1"), 3).unwrap();
    assert_eq!(c.entries.len(), 4, "INTENDED receipts fold into their terminal receipt");
    assert_eq!(c.entries[0].boundary, Boundary::Policy);
    assert_eq!(c.entries[0].branch_taken.as_deref(), Some("ALLOW"));
    let e = &c.entries[1];
    assert_eq!((e.boundary, e.effectful, e.node_id.as_str()), (Boundary::Tool, Some(true), "node0"));
    assert_eq!(e.primitive_id.as_deref(), Some("tool.fs_write"));
    assert_eq!(e.recorded_result_hash, sha256_tagged(b"out0"));
    assert_eq!(e.receipt_ids.as_ref().unwrap().len(), 2);
    assert_eq!(c.run_receipt_hash, s.read_run("run1").unwrap().last().unwrap()["chain"]["content_hash"].as_str().unwrap());
    // Deterministic id; ABI-closed round trip.
    assert_eq!(c.cassette_id, record_cassette(&s, "run1", Some("graph1"), 3).unwrap().cassette_id);
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(serde_json::from_value::<CassetteV1>(v).unwrap(), c);
    // Nothing to record / broken chain refuse.
    assert!(record_cassette(&s, "nope", None, 0).is_err());
}

#[test]
fn replay_recorded_run_has_zero_divergence_and_zero_live_effects() {
    let (_d, s) = store();
    let calls = Cell::new(0);
    live_run(&s, "run1", &calls);
    let chain_len = s.read_run("run1").unwrap().len();
    let c = record_cassette(&s, "run1", None, 0).unwrap();
    let mut r = Replayer::new(&s, c, "replay1", EffectsMode::RecordedOnly).unwrap();
    assert!(matches!(r.step(&policy_step()), StepOutcome::Recorded(_)));
    for n in 0..3 {
        let StepOutcome::Recorded(res) = r.step(&step(n).with_key(&format!("idem-key-{n}"))) else { panic!("refused") };
        assert_eq!(res.external_ref.as_deref(), Some(format!("ext{n}").as_str()));
        assert_eq!(res.result_hash, sha256_tagged(format!("out{n}").as_bytes()));
        assert_eq!(res.status.as_deref(), Some("COMMITTED"));
    }
    let rep = r.finish().unwrap();
    assert_eq!(rep.verdict, Verdict::Identical, "{rep:?}");
    assert!(rep.divergences.is_empty());
    assert_eq!(calls.get(), 3, "no effect ran during replay");
    assert_eq!(s.read_run("run1").unwrap().len(), chain_len, "replay appended nothing to the chain");
    // Report is ABI-shaped.
    let t = serde_json::to_string(&rep).unwrap();
    assert!(serde_json::from_str::<DivergenceReportV1>(&t).is_ok());
    // Self-replay helper agrees.
    let c = record_cassette(&s, "run1", None, 0).unwrap();
    assert_eq!(replay_report(&s, c, None, "self").unwrap().verdict, Verdict::Identical);
}

#[test]
fn replay_unrecorded_effect_is_refused_never_executed() {
    let (_d, s) = store();
    let calls = Cell::new(0);
    live_run(&s, "run1", &calls);
    let c = record_cassette(&s, "run1", None, 0).unwrap();
    let mut r = Replayer::new(&s, c, "replay1", EffectsMode::RecordedOnly).unwrap();
    r.step(&policy_step());
    // A call the recording never made (different args).
    let StepOutcome::Refused(d) = r.step(&step(99)) else { panic!("unrecorded effect must be refused") };
    assert_eq!(d.kind, DivergenceKind::ExtraEntry);
    assert_eq!(calls.get(), 3);
    let rep = r.finish().unwrap();
    assert_eq!(rep.verdict, Verdict::UnexpectedDivergence);
    // The three recorded calls were never reached.
    assert_eq!(rep.divergences.iter().filter(|d| d.kind == DivergenceKind::MissingEntry).count(), 3);
    // Calls past the end of the cassette are also refused.
    let c = record_cassette(&s, "run1", None, 0).unwrap();
    let mut steps = ReplayStep::from_cassette(&c);
    steps.push(steps[1].clone());
    let rep = replay_report(&s, c, Some(steps), "x").unwrap();
    assert_eq!(rep.divergences.len(), 1);
    assert_eq!(rep.divergences[0].kind, DivergenceKind::ExtraEntry);
}

#[test]
fn replay_detects_tampered_and_missing_recordings() {
    // Tampered receipt body.
    let (d, s) = store();
    let calls = Cell::new(0);
    live_run(&s, "run1", &calls);
    let c = record_cassette(&s, "run1", None, 0).unwrap();
    let target = c.entries[2].clone();
    let idx = s.read_run("run1").unwrap().iter()
        .position(|r| r["chain"]["receipt_id"] == json!(target.result_ref)).unwrap() as u64;
    let p = rp(&d, "run1", idx);
    let mut v: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    v["result_hash"] = json!(sha256_tagged(b"forged"));
    std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
    let mut r = Replayer::new(&s, c.clone(), "r", EffectsMode::RecordedOnly).unwrap();
    r.step(&policy_step());
    // The chain no longer verifies: nothing is served, not even untouched entries (#12).
    assert!(matches!(r.step(&step(0)), StepOutcome::Refused(_)));
    let StepOutcome::Refused(dv) = r.step(&step(1)) else { panic!("tampered result must not be served") };
    assert_eq!((dv.kind, dv.seq), (DivergenceKind::ResultHash, target.seq));
    let rep = r.finish().unwrap();
    assert_eq!(rep.verdict, Verdict::UnexpectedDivergence);
    assert!(rep.divergences.iter().any(|d| d.node_id == "chain"), "chain break reported: {rep:?}");

    // Tampered cassette (recorded hash edited).
    let (_d2, s2) = store();
    live_run(&s2, "run2", &calls);
    let mut c2 = record_cassette(&s2, "run2", None, 0).unwrap();
    c2.entries[1].recorded_result_hash = sha256_tagged(b"lie");
    let rep = replay_report(&s2, c2, None, "r").unwrap();
    assert_eq!(rep.divergences.len(), 1);
    assert_eq!((rep.divergences[0].kind, rep.divergences[0].seq), (DivergenceKind::ResultHash, 1));

    // Missing recording (receipt deleted from the chain).
    let (d3, s3) = store();
    live_run(&s3, "run3", &calls);
    let c3 = record_cassette(&s3, "run3", None, 0).unwrap();
    let last = s3.read_run("run3").unwrap().len() as u64 - 1;
    std::fs::remove_file(rp(&d3, "run3", last)).unwrap();
    let rep = replay_report(&s3, c3, None, "r").unwrap();
    assert!(rep.divergences.iter().any(|d| d.kind == DivergenceKind::MissingEntry && d.seq == 3), "{rep:?}");
    // Branch change on a policy boundary.
    let (_d4, s4) = store();
    live_run(&s4, "run4", &calls);
    let c4 = record_cassette(&s4, "run4", None, 0).unwrap();
    let mut steps = ReplayStep::from_cassette(&c4);
    steps[0].branch = Some("DENY".into());
    steps.swap(2, 3);
    let rep = replay_report(&s4, c4.clone(), Some(steps.clone()), "r").unwrap();
    let kinds: Vec<_> = rep.divergences.iter().map(|d| d.kind).collect();
    assert!(kinds.contains(&DivergenceKind::PolicyOutcome) && kinds.contains(&DivergenceKind::NodeOrder), "{kinds:?}");
    // Declared-expected divergences give EXPECTED_DIVERGENCE.
    let mut r = Replayer::new(&s4, c4, "r", EffectsMode::RecordedOnly).unwrap()
        .expect(&[DivergenceKind::PolicyOutcome, DivergenceKind::NodeOrder]);
    for st in &steps { r.step(st); }
    assert_eq!(r.finish().unwrap().verdict, Verdict::ExpectedDivergence);
}

#[test]
fn replay_idempotency_interplay_with_receipt_chain() {
    let (_d, s) = store();
    let calls = Cell::new(0);
    live_run(&s, "run1", &calls);
    // A retry with the same key during the live run was deduped by WP3: still one entry per key.
    let o = s.run_effect_once(&cx("run1", "node0"), &req(0, "idem-key-0"), || { calls.set(calls.get() + 1); Ok((sha256_tagged(b"x"), None)) }).unwrap();
    assert!(matches!(o, EffectOutcome::Replayed(_)));
    // A FAILED attempt then a fresh successful attempt under a new key both record.
    s.run_effect_once(&cx("run1", "node5"), &req(5, "idem-key-5"), || anyhow::bail!("boom")).unwrap();
    s.run_effect_once(&cx("run1", "node5"), &req(5, "idem-key-5"), || Ok((sha256_tagged(b"ok5"), Some("ext5".into())))).unwrap();
    let c = record_cassette(&s, "run1", None, 0).unwrap();
    assert_eq!(c.entries.len(), 6, "{:#?}", c.entries);
    let mut r = Replayer::new(&s, c.clone(), "r", EffectsMode::RecordedOnly).unwrap();
    r.step(&policy_step());
    for n in 0..3 { assert!(matches!(r.step(&step(n).with_key(&format!("idem-key-{n}"))), StepOutcome::Recorded(_))); }
    let StepOutcome::Recorded(f) = r.step(&step(5)) else { panic!() };
    assert_eq!(f.status.as_deref(), Some("FAILED"), "recorded failure is replayed, not retried");
    let StepOutcome::Recorded(ok) = r.step(&step(5).with_key("idem-key-5")) else { panic!() };
    assert_eq!(ok.external_ref.as_deref(), Some("ext5"));
    assert_eq!(r.finish().unwrap().verdict, Verdict::Identical);
    // Key pointing at a different effect than the recording is a divergence, refused.
    let mut r = Replayer::new(&s, c, "r", EffectsMode::RecordedOnly).unwrap();
    r.step(&policy_step());
    assert!(matches!(r.step(&step(0).with_key("idem-key-1")), StepOutcome::Refused(_)));
    // Replay never touched the idempotency index: WP3 still dedupes to the original.
    assert_eq!(calls.get(), 3);
    let o = s.run_effect_once(&cx("run1", "node1"), &req(1, "idem-key-1"), || { calls.set(99); Ok((sha256_tagged(b"x"), None)) }).unwrap();
    let EffectOutcome::Replayed(v) = o else { panic!() };
    assert_eq!(v["external_ref"], json!("ext1"));
    // An unresolved INTENDED effect cannot be recorded.
    s.record_effect(&cx("run1", "node7"), &req(7, "idem-key-7"), "INTENDED", None, None, None).unwrap();
    assert!(record_cassette(&s, "run1", None, 0).unwrap_err().to_string().contains("unresolved"));
}

#[test]
fn replay_gate_recorded_tool_effects_replay_by_payload() {
    // Effects recorded through ReceiptStore::record_tool_effect (the gate path) replay
    // from the same payload the gate hashed.
    let d = tempfile::tempdir().unwrap();
    let rs = ReceiptStore::new(ReceiptStoreOptions { root_dir: Some(d.path().into()), receipts_dir: None, blobs_dir: None }).unwrap();
    let payload = json!({"cmd": "touch x", "idempotency_key": "gate-key-0001"});
    let id = rs.record_tool_effect("runG", "tool.shell_exec", &payload, || Ok("legacy-1".into())).unwrap();
    let cs = rs.chain_store().unwrap();
    let c = record_cassette(&cs, "runG", None, 0).unwrap();
    let st = ReplayStep::tool("runG", "tool.shell_exec", &payload, "EXECUTE").unwrap().with_key("gate-key-0001");
    let mut r = Replayer::new(&cs, c, "r", EffectsMode::RecordedOnly).unwrap();
    let StepOutcome::Recorded(res) = r.step(&st) else { panic!() };
    assert_eq!(res.external_ref.as_deref(), Some(id.as_str()));
    assert_eq!(r.finish().unwrap().verdict, Verdict::Identical);
}

#[test]
fn review12_replay_serves_nothing_after_signature_chain_fails() {
    let (d, s) = store();
    let calls = Cell::new(0);
    live_run(&s, "run1", &calls);
    let c = record_cassette(&s, "run1", None, 0).unwrap();
    // Corrupt only the signature of a COMMITTED effect receipt; body/hash/cassette unchanged.
    let target = c.entries[1].clone();
    let idx = s.read_run("run1").unwrap().iter()
        .position(|r| r["chain"]["receipt_id"] == json!(target.result_ref)).unwrap() as u64;
    let p = rp(&d, "run1", idx);
    let mut v: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
    let mut sig = v["chain"]["signature"]["value"].as_str().unwrap().to_string();
    let first = if sig.starts_with('A') { "B" } else { "A" };
    sig.replace_range(0..1, first);
    v["chain"]["signature"]["value"] = json!(sig);
    std::fs::write(&p, serde_json::to_vec(&v).unwrap()).unwrap();
    assert!(!s.verify_chain("run1").unwrap().ok);
    let mut r = Replayer::new(&s, c, "r", EffectsMode::RecordedOnly).unwrap();
    r.step(&policy_step());
    for n in 0..3 {
        assert!(matches!(r.step(&step(n)), StepOutcome::Refused(_)), "step {n} served from an unverified chain");
    }
    let rep = r.finish().unwrap();
    assert_eq!(rep.verdict, Verdict::UnexpectedDivergence);
    assert!(rep.divergences.iter().any(|d| d.node_id == "chain"), "{rep:?}");
}

#[test]
fn review13_relabelled_cassette_entry_is_refused() {
    let (_d, s) = store();
    let calls = Cell::new(0);
    live_run(&s, "run1", &calls);
    let mut c = record_cassette(&s, "run1", None, 0).unwrap();
    // Relabel recorded request A (node0's write) as a different request B.
    let b = ReplayStep::tool("node0", "tool.fs_write", &json!({"path": "evil.txt", "content": 9}), "WORKSPACE_WRITE").unwrap();
    c.entries[1].request_hash = b.request_hash.clone();
    let mut r = Replayer::new(&s, c, "r", EffectsMode::RecordedOnly).unwrap();
    r.step(&policy_step());
    let StepOutcome::Refused(d) = r.step(&b) else { panic!("relabelled entry served for a different request") };
    assert_eq!(d.kind, DivergenceKind::ResultHash);
    assert!(d.explanation.unwrap().contains("signed receipt"));
    assert_eq!(r.finish().unwrap().verdict, Verdict::UnexpectedDivergence);
}
