//! Tests for the intelligence layer: real engine, real temp folders, a real
//! SQLite index. The only double is the embedding model (an edge).

use std::sync::Arc;
use std::time::Duration;

use allternit_files_core::{PrincipalKind, PrincipalRef, RootKind};
use allternit_files_engine::RootRecord;

use super::budget::Budget;
use super::embed::{Embedder, NullEmbedder};
use super::{dedup, kind_may_read, match_expr, run_pass, scoped_roots};
use crate::files_v2::{Config, FilesV2, Inner, Req};
use crate::memory_index::Embedded;

/// A tiny "model": two meaning axes, money words and food words.
struct TopicEmbedder;

impl Embedder for TopicEmbedder {
    fn embed(&self, texts: &[String], _query: bool) -> Option<Embedded> {
        let money = ["bill", "invoice", "pay", "payment", "owed", "due"];
        let food = ["recipe", "cook", "bake", "flour", "dinner"];
        let vectors = texts
            .iter()
            .map(|t| {
                let t = t.to_lowercase();
                let count = |ws: &[&str]| ws.iter().filter(|w| t.contains(**w)).count() as f32;
                vec![count(&money), count(&food), 0.01]
            })
            .collect();
        Some(Embedded { model: "test-topics".into(), dim: 3, vectors })
    }
}

struct Env {
    _tmp: tempfile::TempDir,
    inner: Arc<Inner>,
}

async fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(base.join("home")).unwrap();
    let cfg = Config {
        enabled: true,
        data_dir: base.join("data"),
        home: Some(base.join("home")),
        computer_id: "rd_test".into(),
        memory_db: None,
        models_dir: None,
        code_dir: None,
        watch: false,
        clock: None,
    };
    let inner = FilesV2::new(cfg).inner().await.ok().expect("open");
    inner.intel.set_embedder(Arc::new(TopicEmbedder));
    Env { _tmp: tmp, inner }
}

impl Env {
    fn root(&self, kind: RootKind) -> RootRecord {
        self.inner.drive.roots().unwrap().into_iter().find(|r| r.kind == kind).expect("default root")
    }

    /// Writes a file into a root's folder, rescans, and returns its item id.
    fn put(&self, root: &RootRecord, name: &str, body: &str) -> String {
        std::fs::write(root.local_path.join(name), body).unwrap();
        self.inner.drive.scan_root(&root.root_id).unwrap();
        let top = self.inner.drive.list(&root.root_item_id).unwrap();
        top.into_iter().find(|i| i.name == name).expect("scanned").id
    }

    /// Runs passes until one finds nothing; returns the files indexed.
    fn index_all(&self) -> usize {
        let mut total = 0;
        for _ in 0..50 {
            let r = run_pass(&self.inner, &mut |_, _| {}).ok().expect("pass");
            total += r.indexed;
            if !r.more {
                break;
            }
        }
        total
    }

    fn owner(&self) -> Req {
        req(PrincipalKind::Owner)
    }
}

fn req(kind: PrincipalKind) -> Req {
    Req {
        principal: PrincipalRef { kind, id: "p1".into(), display_name: None },
        user_id: "u1".into(),
        relayed: false,
        unlocked: false,
        device_name: None,
    }
}

fn ids(roots: &[RootRecord]) -> Vec<String> {
    roots.iter().map(|r| r.root_id.clone()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn phrase_search_finds_a_document_by_its_content() {
    let e = env().await;
    let inbox = e.root(RootKind::Inbox);
    let budget = e.put(&inbox, "q3.txt", "The quarterly forecast shows revenue growth in the northern region.");
    e.put(&inbox, "lunch.txt", "Sandwiches on Friday, bring your own coffee.");
    assert_eq!(e.index_all(), 2);

    let roots = scoped_roots(&e.inner, &e.owner(), None).ok().expect("roots");
    let expr = match_expr("\"revenue growth\"").unwrap();
    let hits = e.inner.intel.search_full_text(&expr, &ids(&roots), None, 10, 0).unwrap();
    assert_eq!(hits.iter().map(|h| h.item_id.as_str()).collect::<Vec<_>>(), vec![budget.as_str()], "only the document that has the phrase");
    assert!(hits[0].snippet.as_deref().unwrap_or("").contains("revenue growth"));

    // The words in the wrong order are not the phrase.
    let swapped = match_expr("\"growth revenue\"").unwrap();
    assert!(e.inner.intel.search_full_text(&swapped, &ids(&roots), None, 10, 0).unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_file_is_found_by_its_new_content_only() {
    let e = env().await;
    let inbox = e.root(RootKind::Inbox);
    let id = e.put(&inbox, "note.txt", "alpha bravo charlie");
    e.index_all();
    e.put(&inbox, "note.txt", "delta echo foxtrot extra");
    e.index_all();
    let ids = ids(&scoped_roots(&e.inner, &e.owner(), None).ok().expect("roots"));
    let find = |q: &str| e.inner.intel.search_full_text(&match_expr(q).unwrap(), &ids, None, 10, 0).unwrap();
    assert!(find("bravo").is_empty());
    assert_eq!(find("foxtrot")[0].item_id, id);
}

#[tokio::test(flavor = "multi_thread")]
async fn meaning_search_ranks_by_topic_and_is_unsupported_without_a_model() {
    let e = env().await;
    let inbox = e.root(RootKind::Inbox);
    let bill = e.put(&inbox, "a.txt", "Please pay this invoice, payment is due Friday.");
    e.put(&inbox, "b.txt", "Bake the dinner with flour, a simple recipe to cook.");
    e.index_all();
    let roots = scoped_roots(&e.inner, &e.owner(), None).ok().expect("roots");
    let q = e.inner.intel.embedder().embed(&["money owed".to_string()], true).unwrap();
    let qv = super::embed::normalized(q.vectors[0].clone());
    let hits = e.inner.intel.search_semantic(&qv, &q.model, &ids(&roots), None, 10, 0).unwrap();
    assert_eq!(hits[0].item_id, bill);
    assert!(hits.len() == 1 || hits[1].score < hits[0].score);

    // No model: no vectors are made, and nothing is faked.
    e.inner.intel.set_embedder(Arc::new(NullEmbedder));
    let late = e.put(&inbox, "c.txt", "Another invoice to pay.");
    e.index_all();
    assert!(e.inner.intel.is_indexed(&late).unwrap());
    let models = e.inner.intel.vector_models().unwrap();
    assert_eq!(models, vec!["test-topics".to_string()]);
    let total_vectors: i64 = {
        let hits = e.inner.intel.search_semantic(&qv, "test-topics", &ids(&roots), None, 10, 0).unwrap();
        hits.iter().filter(|h| h.item_id == late).count() as i64
    };
    assert_eq!(total_vectors, 0, "a file indexed with no model has no vector");
}

/// SPEC D12: indexing runs only while the computer is already awake and never
/// resets idle. Reads the module sources on purpose: a behaviour test can't
/// see a call added for some other id.
#[test]
fn intel_never_wakes_or_touches_activity() {
    const SOURCES: &[(&str, &str)] = &[
        ("intel/mod.rs", include_str!("mod.rs")),
        ("intel/routes.rs", include_str!("routes.rs")),
        ("intel/budget.rs", include_str!("budget.rs")),
        ("intel/embed.rs", include_str!("embed.rs")),
    ];
    const FORBIDDEN: &[&str] = &[
        "touch_computer_activity",
        "touch_runtime_activity",
        "touch_activity",
        "touch_provisioned_activity",
        "last_activity_at",
        "mark_active",
        "wake_computer",
        "request_wake",
        "wake_ticket",
        "wake_runtime",
        "ensure_awake",
    ];
    let mut hits = Vec::new();
    for (file, src) in SOURCES {
        for (n, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            for f in FORBIDDEN {
                if code.contains(f) {
                    hits.push(format!("{file}:{}: {f}", n + 1));
                }
            }
        }
    }
    assert!(hits.is_empty(), "the indexer must never wake or touch activity: {hits:?}");
}

#[test]
fn the_cpu_budget_sets_the_rest_after_work() {
    let b = Budget { cpu_percent: 25, batch_files: 8 };
    assert_eq!(b.rest_after(Duration::from_millis(100)), Duration::from_millis(300));
    assert_eq!(Budget { cpu_percent: 50, batch_files: 8 }.rest_after(Duration::from_millis(80)), Duration::from_millis(80));
    assert_eq!(Budget { cpu_percent: 100, batch_files: 8 }.rest_after(Duration::from_secs(1)), Duration::ZERO);
    // 0 is not "no work": it is held at 1%.
    assert_eq!(Budget { cpu_percent: 0, batch_files: 8 }.rest_after(Duration::from_millis(10)), Duration::from_millis(990));
    let mut bo = super::budget::Backoff::new();
    let waits: Vec<u64> = (0..8).map(|_| bo.next_wait().as_secs()).collect();
    assert_eq!(waits, vec![30, 60, 120, 240, 300, 300, 300, 300]);
    bo.reset();
    assert_eq!(bo.next_wait().as_secs(), 30);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pass_holds_to_the_batch_and_rests_after_every_file() {
    let e = env().await;
    let inbox = e.root(RootKind::Inbox);
    for i in 0..12 {
        std::fs::write(inbox.local_path.join(format!("f{i}.txt")), format!("document number {i} about budgets")).unwrap();
    }
    e.inner.drive.scan_root(&inbox.root_id).unwrap();
    let budget = Budget { cpu_percent: 20, batch_files: 5 };
    e.inner.intel.set_budget(budget);

    let mut paced = Vec::new();
    let report = run_pass(&e.inner, &mut |busy, rest| paced.push((busy, rest))).ok().unwrap();
    assert_eq!(report.indexed, 5, "one pass is one batch");
    assert!(report.more);
    assert_eq!(paced.len(), 5, "a rest after every file");
    for (busy, rest) in &paced {
        assert_eq!(*rest, budget.rest_after(*busy));
        // The duty cycle never goes over the cap (small slack for rounding).
        let used = busy.as_secs_f64() / (busy.as_secs_f64() + rest.as_secs_f64()).max(1e-9);
        assert!(used <= 0.2 + 1e-6, "used {used}");
    }
    assert!(!e.inner.intel.caught_up());
    // The rest of the files take two more passes, then the index is caught up.
    e.index_all();
    assert_eq!(e.inner.intel.indexed_count().unwrap(), 12);
    run_pass(&e.inner, &mut |_, _| {}).ok().unwrap();
    assert!(e.inner.intel.caught_up());
}

#[tokio::test(flavor = "multi_thread")]
async fn protected_roots_are_not_indexed_unless_the_owner_opts_in() {
    let e = env().await;
    let shared = e.root(RootKind::Shared);
    let mut policy = shared.policy.clone();
    policy.protected = true;
    e.inner.drive.update_root_policy(&shared.root_id, policy).unwrap();
    let secret = e.put(&shared, "diary.txt", "my private diary entry");
    e.index_all();
    assert!(!e.inner.intel.is_indexed(&secret).unwrap(), "protected means not indexed by default");

    e.inner.intel.set_opt_in(&shared.root_id, true).unwrap();
    e.index_all();
    assert!(e.inner.intel.is_indexed(&secret).unwrap());

    // Opting out forgets what was indexed.
    e.inner.intel.set_opt_in(&shared.root_id, false).unwrap();
    e.index_all();
    assert!(!e.inner.intel.is_indexed(&secret).unwrap());
    let any = e.inner.intel.search_full_text(&match_expr("diary").unwrap(), &[shared.root_id.clone()], None, 10, 0).unwrap();
    assert!(any.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn search_scope_follows_the_caller() {
    let e = env().await;
    let all = scoped_roots(&e.inner, &e.owner(), None).ok().expect("roots");
    assert!(!all.is_empty());

    // An agent only gets roots that allow agent processing.
    let agent = scoped_roots(&e.inner, &req(PrincipalKind::Agent), None).ok().expect("roots");
    assert!(!agent.is_empty());
    assert!(agent.iter().all(|r| r.policy.agent_process));
    let denied: Vec<_> = all.iter().filter(|r| !r.policy.agent_process).collect();
    assert!(!denied.is_empty(), "the defaults include roots agents may not use");
    assert!(agent.iter().all(|r| denied.iter().all(|d| d.root_id != r.root_id)));

    // A guest gets nothing from these routes (shares have their own gateway).
    assert!(scoped_roots(&e.inner, &req(PrincipalKind::Guest), None).ok().expect("roots").is_empty());
    assert!(all.iter().all(|r| !kind_may_read(PrincipalKind::Guest, r)));

    // A root filter narrows, never widens.
    let inbox = e.root(RootKind::Inbox);
    let shared = e.root(RootKind::Shared);
    let pair = [shared.root_id.clone(), inbox.root_id.clone()];
    let only = scoped_roots(&e.inner, &req(PrincipalKind::Agent), Some(&pair[..])).ok().expect("roots");
    assert_eq!(only.iter().map(|r| r.root_id.clone()).collect::<Vec<_>>(), vec![inbox.root_id.clone()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicates_group_by_content_and_report_the_saving() {
    let e = env().await;
    let inbox = e.root(RootKind::Inbox);
    let a = e.put(&inbox, "one.txt", "exactly the same bytes");
    let b = e.put(&inbox, "two.txt", "exactly the same bytes");
    e.put(&inbox, "three.txt", "different bytes entirely");
    let groups = dedup(&e.inner.cfg.data_dir.join("engine"), &[inbox.root_id.clone()], 10).unwrap();
    assert_eq!(groups.len(), 1);
    let mut got = groups[0].item_ids.clone();
    got.sort();
    let mut want = vec![a, b];
    want.sort();
    assert_eq!(got, want);
    assert_eq!(groups[0].size, "exactly the same bytes".len() as u64);
}

#[tokio::test(flavor = "multi_thread")]
async fn tags_and_provenance_show_on_items() {
    let e = env().await;
    let inbox = e.root(RootKind::Inbox);
    let id = e.put(&inbox, "report.txt", "numbers");
    e.inner.intel.change_tags(&id, &["finance".into(), "q3".into()], &[]).ok().expect("tags");
    e.inner.intel.change_tags(&id, &[], &["q3".into()]).ok().expect("tags");
    e.inner
        .intel
        .set_provenance(
            &id,
            &allternit_files_core::Provenance {
                source: allternit_files_core::ProvenanceSource::Agent,
                ai_generated: true,
                agent_id: Some("ag1".into()),
                task_id: None,
                input_item_ids: None,
            },
        )
        .unwrap();
    let rec = e.inner.drive.stat(&id).unwrap();
    let item = e.inner.item(&inbox, &rec);
    assert_eq!(item.tags, Some(vec!["finance".to_string()]));
    assert!(item.provenance.expect("provenance").ai_generated);
}

#[test]
fn summaries_pick_sentences_from_the_file() {
    let text = "The launch moved to March because the supplier slipped. Budgets stay flat this year. \
                The supplier slipped twice, and the supplier contract is under review. Lunch is on Friday.";
    let s = super::summarize(text, 2);
    assert!(s.contains("supplier"), "{s}");
    assert!(!s.contains("Lunch"), "{s}");
}

/// A one-page PDF built byte by byte (no committed blob).
fn tiny_pdf(text: &str) -> Vec<u8> {
    let content = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
    for off in offsets {
        out.extend(format!("{off:010} 00000 n \n").bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).bytes());
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phrase_search_finds_a_pdf_by_its_text() {
    let e = env().await;
    let inbox = e.root(RootKind::Inbox);
    std::fs::write(inbox.local_path.join("contract.pdf"), tiny_pdf("The indemnity clause survives termination")).unwrap();
    std::fs::write(inbox.local_path.join("broken.pdf"), b"%PDF-1.4 not really a pdf").unwrap();
    e.inner.drive.scan_root(&inbox.root_id).unwrap();
    let id = e.inner.drive.list(&inbox.root_item_id).unwrap().into_iter().find(|i| i.name == "contract.pdf").unwrap().id;
    e.index_all(); // the broken file is skipped, not fatal
    let ids = ids(&scoped_roots(&e.inner, &e.owner(), None).ok().expect("roots"));
    let hits = e.inner.intel.search_full_text(&match_expr("\"indemnity clause\"").unwrap(), &ids, None, 10, 0).unwrap();
    assert_eq!(hits.iter().map(|h| h.item_id.clone()).collect::<Vec<_>>(), vec![id]);
}
