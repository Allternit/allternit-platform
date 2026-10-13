//! B2/B3 trained embedding head + C1 rules compile tests.

use super::embed_head::{self, EmbedHead};
use super::flywheel::{self, Head, HeadInput, Shadow, Vectors, SHADOW};
use super::rules_head::{self, CompileBar, Cond, RulesHead, Select};
use super::training::{self, TrainQuery};
use super::store;
use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::memory_index::{EmbedClient, DEFAULT_EMBED_MODEL, HASH_MODEL};
use rusqlite::params;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as usize
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.next() % xs.len()]
    }
}

/// (context, question, [(id, text)], label id)
type Ex = (String, Option<String>, Vec<(String, String)>, String);

const FILLER: &[&str] = &["hello", "please", "help", "today", "my", "account", "order", "thanks", "urgent", "again", "yesterday", "customer"];
const ROUTE_KEYWORDS: &[(&str, &[&str])] =
    &[("billing", &["refund", "invoice", "overcharged"]), ("tech", &["crash", "error", "login"]), ("fraud", &["stolen", "unauthorized", "phishing"])];

fn route_options() -> Vec<(String, String)> {
    [("billing", "Billing team: charges and payments"), ("tech", "Technical support: app faults"), ("fraud", "Fraud desk: account security")]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

/// Ticket routing: the label is a function of one keyword (rule-learnable).
fn route_dataset(n: usize, seed: u64) -> Vec<Ex> {
    let mut r = Lcg(seed);
    (0..n)
        .map(|i| {
            let (team, kws) = ROUTE_KEYWORDS[r.next() % 3];
            let ctx = format!("Ticket {i}: {} {} {} {}", r.pick(FILLER), r.pick(FILLER), r.pick(kws), r.pick(FILLER));
            (ctx, Some("Which team should handle this ticket?".into()), route_options(), team.into())
        })
        .collect()
}

fn topic(i: usize) -> String {
    const SYL: &[&str] = &["ka", "lo", "mi", "ne", "ru", "so", "ti", "va", "ze", "pu"];
    format!("{}{}{}", SYL[i % 10], SYL[(i / 10) % 10], SYL[(i * 7 + 3) % 10])
}

/// Pick the option about the state's topic: 80 topics, varying options and
/// positions. Semantic (shared meaning), not a short rule list.
fn pick_dataset(n: usize, seed: u64) -> Vec<Ex> {
    let mut r = Lcg(seed);
    (0..n)
        .map(|i| {
            let t = (i * 37 + 11) % 80;
            let mut topics = vec![t];
            while topics.len() < 4 {
                let d = r.next() % 80;
                if !topics.contains(&d) {
                    topics.push(d);
                }
            }
            let pos = r.next() % 4;
            topics.swap(0, pos);
            let opts = topics
                .iter()
                .enumerate()
                .map(|(j, &x)| (format!("o{j}"), format!("{} for {} work", r.pick(&["kit", "tool", "guide", "box"]), topic(x))))
                .collect();
            let ctx = format!("{} I need help with {} {}", r.pick(FILLER), topic(t), r.pick(FILLER));
            (ctx, None, opts, format!("o{pos}"))
        })
        .collect()
}

fn seed(db: &DbHandle, kind: &str, rows: &[Ex]) {
    let conn = db.connect().unwrap();
    conn.execute("INSERT OR REPLACE INTO decision_training_optin (scope, opted_in, actor, updated_at) VALUES ('org:t', 1, 'a', 't')", []).unwrap();
    for (i, (ctx, q, opts, label)) in rows.iter().enumerate() {
        let options = json!(opts.iter().map(|(id, t)| json!({ "id": id, "text": t })).collect::<Vec<_>>());
        conn.execute(
            "INSERT INTO decision_examples (decision_id, scope, kind, created_at, context, question, options_json,
                teacher_backend, teacher_choice, teacher_confidence, label, label_source, labelled_at)
             VALUES (?1, 'org:t', ?2, ?3, ?4, ?5, ?6, 'oracle', ?7, 1.0, ?7, 'teacher', ?3)",
            params![format!("{kind}-{i}"), kind, format!("2026-10-12T00:00:{:02}.{:03}Z", i / 1000, i % 1000), ctx, q, options.to_string(), label],
        )
        .unwrap();
    }
}

/// An OpenAI-shaped embeddings stub serving a signed bag-of-words embedding
/// (model `test/bow-256`): shared words mean similar vectors.
async fn bow_server() -> String {
    use axum::{routing::post, Json, Router};
    fn bow(t: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; 256];
        for w in t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
            let h = Sha256::digest(w.to_lowercase().as_bytes());
            v[h[0] as usize] += if h[1] & 1 == 0 { 1.0 } else { -1.0 };
        }
        v
    }
    let app = Router::new().route(
        "/v1/embeddings",
        post(|Json(body): Json<Value>| async move {
            let data: Vec<Value> = body["input"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, t)| json!({ "index": i, "embedding": bow(t.as_str().unwrap()) }))
                .collect();
            Json(json!({ "model": "test/bow-256", "data": data }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

/// Poll a job until it ends (or a minute passes).
async fn wait_job(db: &DbHandle, id: &str) -> Value {
    for _ in 0..1200 {
        let j = training::job(&db.connect().unwrap(), id).unwrap().unwrap();
        if j["status"] == training::SUCCEEDED || j["status"] == training::FAILED {
            return j;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("job {id} did not finish");
}

#[tokio::test(flavor = "multi_thread")]
async fn decisions_training_runs_as_a_background_job() {
    let db = DbHandle::new_memory().unwrap();
    seed(&db, "pick", &pick_dataset(120, 9));
    let client: &'static EmbedClient = Box::leak(Box::new(EmbedClient::new(Some(bow_server().await), DEFAULT_EMBED_MODEL)));
    let conn = db.connect().unwrap();
    // One queued or running job per (head, kind).
    let id = training::enqueue(&conn, embed_head::NAME, "pick", "adm").unwrap();
    assert_eq!(training::enqueue(&conn, embed_head::NAME, "pick", "adm").unwrap_err().0, 409);
    // Another head or kind is independent.
    let other = training::enqueue(&conn, rules_head::NAME, "pick", "adm").unwrap();
    training::spawn(db.clone(), client, other.clone(), rules_head::NAME.into(), "pick".into(), "adm".into(), 10_000);
    training::spawn(db.clone(), client, id.clone(), embed_head::NAME.into(), "pick".into(), "adm".into(), 10_000);
    let j = wait_job(&db, &id).await;
    assert_eq!(j["status"], "succeeded", "{j}");
    assert!(j["progress"]["total"].as_u64().unwrap() > 120);
    assert_eq!(j["progress"]["done"], j["progress"]["total"]);
    assert_eq!(j["result"]["version"], 1);
    assert_eq!(flywheel::current_state(&conn, embed_head::NAME, "pick").as_deref(), Some(SHADOW));
    // Same weights as an in-request run over the same data: still deterministic.
    let direct = training::train_embed(&db, client, "pick", "adm", 10_000).await.unwrap();
    assert_eq!(j["result"]["metrics"]["weights_sha256"], direct["metrics"]["weights_sha256"]);
    // The semantic kind does not compile: that job fails with the reason.
    let r = wait_job(&db, &other).await;
    assert_eq!(r["status"], "failed");
    assert!(r["error"].as_str().unwrap().contains("does not compile"), "{r}");
    // Done: a new job for the same (head, kind) may start.
    assert!(training::enqueue(&conn, embed_head::NAME, "pick", "adm").is_ok());

    // A job left running by a process that ended does not block forever.
    conn.execute(
        "INSERT INTO decision_head_jobs (id, head, kind, status, actor, created_at, updated_at) VALUES ('dhj_old', 'head.rules', 'route', 'running', 'a', 't', 't')",
        [],
    )
    .unwrap();
    assert!(training::enqueue(&conn, rules_head::NAME, "route", "adm").is_ok());
    let old = training::job(&conn, "dhj_old").unwrap().unwrap();
    assert_eq!((old["status"].as_str(), old["error"].as_str()), (Some("failed"), Some("interrupted: the server restarted")));
}

fn f(v: &Value, path: &[&str]) -> f64 {
    path.iter().fold(v, |v, k| &v[*k]).as_f64().unwrap_or_else(|| panic!("{path:?} in {v}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn decisions_embed_head_trains_deterministically_and_beats_baselines() {
    let db = DbHandle::new_memory().unwrap();
    seed(&db, "pick", &pick_dataset(200, 7));
    let client = EmbedClient::new(Some(bow_server().await), DEFAULT_EMBED_MODEL);
    let a = training::train_embed(&db, &client, "pick", "adm", 10_000).await.unwrap();
    let b = training::train_embed(&db, &client, "pick", "adm", 10_000).await.unwrap();
    let m = &a["metrics"];
    eprintln!("pick (bow-256): {m:#}");
    // Same data, same embeddings: bit-identical weights and dataset hash.
    assert_eq!(m["weights_sha256"], b["metrics"]["weights_sha256"]);
    assert_eq!(a["dataset_hash"], b["dataset_hash"]);
    assert_eq!((a["version"].as_i64(), b["version"].as_i64()), (Some(1), Some(2)));
    assert_eq!(m["embed_model"], "test/bow-256");
    assert!(m["val_examples"].as_u64().unwrap() >= 25, "{m}");
    let val = f(m, &["val_accuracy"]);
    assert!(val >= 0.9, "val accuracy {val}");
    assert!(val > f(m, &["baselines", "lookup", "accuracy"]) + 0.5, "{m}");
    assert!(val > f(m, &["baselines", "prior", "accuracy"]) + 0.5, "{m}");
    assert_eq!(m["val_accuracy_teacher_labels"], m["val_accuracy"]);

    // Registered in shadow, audited once per version; listed with its version.
    let conn = db.connect().unwrap();
    assert_eq!(flywheel::current_state(&conn, embed_head::NAME, "pick").as_deref(), Some(SHADOW));
    let audits: i64 = conn.query_row("SELECT COUNT(*) FROM decision_head_audit WHERE head = 'head.embed'", [], |r| r.get(0)).unwrap();
    assert_eq!(audits, 2);
    let listed = flywheel::list(&conn).unwrap();
    let row = listed["data"].as_array().unwrap().iter().find(|r| r["head"] == "head.embed").unwrap();
    assert_eq!(row["model"]["version"], 2);
    let versions = training::versions(&conn, embed_head::NAME, "pick").unwrap();
    assert_eq!(versions.len(), 2);
    assert!(versions[0].get("weights").is_none());

    // The semantic kind does not compile to rules: nothing stored.
    let err = training::compile_rules(&conn, "pick", "adm", 10_000, &CompileBar::default()).unwrap_err();
    assert_eq!(err.0, 422, "{}", err.1);
    assert!(training::latest(&conn, rules_head::NAME, "pick").unwrap().is_none());
}

#[test]
fn decisions_fit_is_deterministic_on_fixed_data() {
    // Pure trainer: two runs over the same vectors, identical weights.
    let vecs: Vec<Vec<f32>> = (0..12).map(|i| (0..8).map(|j| (((i * 31 + j * 17) % 13) as f32 - 6.0) / 6.0).collect()).collect();
    let ex: Vec<embed_head::Example> = (0..6).map(|i| embed_head::Example { state: i, options: vec![6 + i, 6 + (i + 1) % 6], label: 0 }).collect();
    let cfg = embed_head::TrainConfig::default();
    let w1 = embed_head::fit(&vecs, &ex, 8, &cfg);
    let w2 = embed_head::fit(&vecs, &ex, 8, &cfg);
    assert_eq!(embed_head::weights_sha256(&w1), embed_head::weights_sha256(&w2));
    assert_eq!(w1.len(), embed_head::n_features(8));
    assert_eq!(embed_head::evaluate(&w1, &vecs, &ex).0, 1.0);
}

#[tokio::test(flavor = "multi_thread")]
async fn decisions_embed_head_refuses_another_embedding_model() {
    let db = DbHandle::new_memory().unwrap();
    seed(&db, "pick", &pick_dataset(60, 3));
    // The embedding server is "down": trained on the hash fallback.
    let hash = EmbedClient::new(None, DEFAULT_EMBED_MODEL);
    let out = training::train_embed(&db, &hash, "pick", "adm", 10_000).await.unwrap();
    assert_eq!(out["metrics"]["embed_model"], HASH_MODEL);
    let conn = db.connect().unwrap();
    let (ctx, texts) = ("tools for kalomi work", ["kit for kalomi work", "box for nerupu work"]);
    let ids = ["o0", "o1"];
    let input = |v: Option<&Vectors>| {
        EmbedHead.predict(&conn, &HeadInput { owner: "u", kind: "pick", key: "k", ids: &ids, context: ctx, question: None, texts: &texts, vectors: v, decision_id: None, pin: None }).unwrap()
    };
    let same = embed_head::embed_decision(&hash, ctx, None, &texts).await.unwrap();
    let a = input(Some(&same)).expect("hash vectors on a hash-trained head");
    assert!((a.probs.iter().sum::<f64>() - 1.0).abs() < 1e-9);
    // Vectors from EmbeddingGemma (or anything else): no answer, never mixed.
    let gemma = Vectors { model: DEFAULT_EMBED_MODEL.into(), ..same.clone() };
    assert!(input(Some(&gemma)).is_none());
    let short = Vectors { state: same.state[..128].to_vec(), ..same.clone() };
    assert!(input(Some(&short)).is_none());
    assert!(input(None).is_none());
}

#[test]
fn decisions_rules_compile_only_when_clean() {
    let db = DbHandle::new_memory().unwrap();
    seed(&db, "route", &route_dataset(200, 11));
    let conn = db.connect().unwrap();
    let out = training::compile_rules(&conn, "route", "adm", 10_000, &CompileBar::default()).unwrap();
    let m = &out["metrics"];
    eprintln!("route rules: {:#}", out["rules"]);
    assert_eq!((m["accepted"].as_bool(), m["train_accuracy"].as_f64(), m["val_accuracy"].as_f64()), (Some(true), Some(1.0), Some(1.0)));
    // Readable: one keyword rule per keyword, naming its team.
    let rules: rules_head::RuleList = serde_json::from_value(out["rules"].clone()).unwrap();
    assert!(rules.rules.len() <= 9, "{:?}", rules.rules);
    assert!(rules.rules.iter().any(|r| r.cond == Cond::StateHas("refund".into()) && r.then == Select::Option("billing".into())), "{:?}", rules.rules);
    assert_eq!(flywheel::current_state(&conn, rules_head::NAME, "route").as_deref(), Some(SHADOW));

    // Deterministic: the same data compiles to the same list.
    let again = training::compile_rules(&conn, "route", "adm", 10_000, &CompileBar::default()).unwrap();
    assert_eq!(again["rules"], out["rules"]);

    // Served as an S0 head: no vectors needed.
    let ids = ["billing", "tech", "fraud"];
    let route = route_options();
    let texts: Vec<&str> = route.iter().map(|o| o.1.as_str()).collect();
    let a = RulesHead
        .predict(&conn, &HeadInput { owner: "u", kind: "route", key: "k", ids: &ids, context: "Somebody made an unauthorized purchase", question: None, texts: &texts, vectors: None, decision_id: None, pin: None })
        .unwrap()
        .unwrap();
    assert_eq!(ids[a.choice], "fraud");
    assert!(a.probs[a.choice] > 0.9 && a.probs[a.choice] < 1.0);

    // Noisy labels (10% contradict their keyword): not 100% on training, rejected.
    let noisy = DbHandle::new_memory().unwrap();
    let mut rows = route_dataset(200, 11);
    let teams = ["billing", "tech", "fraud"];
    for r in rows.iter_mut().step_by(10) {
        let k = teams.iter().position(|t| *t == r.3).unwrap();
        r.3 = teams[(k + 1) % 3].into();
    }
    seed(&noisy, "route", &rows);
    let err = training::compile_rules(&noisy.connect().unwrap(), "route", "adm", 10_000, &CompileBar::default()).unwrap_err();
    assert_eq!(err.0, 422);
    assert!(training::latest(&noisy.connect().unwrap(), rules_head::NAME, "route").unwrap().is_none());

    // Too few examples: rejected however clean.
    let small = DbHandle::new_memory().unwrap();
    seed(&small, "route", &route_dataset(30, 5));
    let err = training::compile_rules(&small.connect().unwrap(), "route", "adm", 10_000, &CompileBar::default()).unwrap_err();
    assert!(err.1.contains("needs 50 examples"), "{}", err.1);
}

#[tokio::test(flavor = "multi_thread")]
async fn decisions_trained_heads_run_in_shadow_only_where_trained() {
    let db = DbHandle::new_memory().unwrap();
    seed(&db, "pick", &pick_dataset(120, 9));
    seed(&db, "route", &route_dataset(120, 4));
    let client = EmbedClient::new(Some(bow_server().await), DEFAULT_EMBED_MODEL);
    training::train_embed(&db, &client, "pick", "adm", 10_000).await.unwrap();
    training::compile_rules(&db.connect().unwrap(), "route", "adm", 10_000, &CompileBar::default()).unwrap();

    let shadow = |id: &str, kind: &str, ctx: &str, opts: &[(&str, &str)], vectors: Option<Vectors>| {
        let options = json!(opts.iter().map(|(i, t)| json!({ "id": i, "text": t })).collect::<Vec<_>>());
        store::insert(&db, &store::Row {
            id, owner: "u", created_at: &super::super::store::now(), kind, session_id: None, task: None, context_hash: "h",
            has_image: false, options: &options, choice: None, probs: &json!({}), confidence: 0.0, abstained: true,
            backend: "local", latency_ms: 1.0, attempts: &json!([]),
        })
        .unwrap();
        let s = Shadow {
            decision_id: id.into(), owner: "u".into(), kind: kind.into(), key: id.into(),
            ids: opts.iter().map(|o| o.0.to_string()).collect(), has_image: false, context: ctx.into(), question: None,
            texts: opts.iter().map(|o| o.1.to_string()).collect(), vectors,
        };
        flywheel::record_key(&db, &s).unwrap();
        flywheel::record_shadow(&db, &s).unwrap();
    };
    let opts = [("o0", "box for kalomi work"), ("o1", "kit for nerupu work"), ("o2", "tool for sotiva work")];
    let texts: Vec<&str> = opts.iter().map(|o| o.1).collect();
    let v = embed_head::embed_decision(&client, "please I need help with nerupu", None, &texts).await;
    shadow("d1", "pick", "please I need help with nerupu", &opts, v);
    let route: Vec<(String, String)> = route_options();
    let ropts: Vec<(&str, &str)> = route.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    shadow("d2", "route", "my login shows an error", &ropts, None);

    let conn = db.connect().unwrap();
    let rows: Vec<(String, String, Option<String>)> = conn
        .prepare("SELECT decision_id, head, choice FROM decision_shadow ORDER BY decision_id, head")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            ("d1".into(), "head.embed".into(), Some("o1".into())),
            ("d1".into(), "head.lookup".into(), None),
            ("d2".into(), "head.lookup".into(), None),
            ("d2".into(), "head.rules".into(), Some("tech".into())),
        ]
    );
    // Untrained heads are never registered for a kind.
    let embed_route: i64 = conn.query_row("SELECT COUNT(*) FROM decision_heads WHERE head = 'head.embed' AND kind = 'route'", [], |r| r.get(0)).unwrap();
    assert_eq!(embed_route, 0);
}

#[tokio::test]
async fn decisions_train_and_compile_endpoints_are_admin_only() {
    use axum::extract::{Path, Query, State};
    use axum::Extension;
    let dir = std::env::temp_dir().join(format!("allternit-heads-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let st = crate::test_helpers::app_state(&dir).await;
    let user = |id: &str| AuthUser {
        user_id: id.into(), email: None, name: None, avatar_url: None, tenant_id: None,
        organization_id: Some("org-1".into()), organization_role: Some("org:admin".into()), organization_slug: None,
    };
    let rid = super::super::RequestId("req_t".into());
    // SAFETY of env mutation: only the decisions admin checks read it.
    std::env::set_var("ALLTERNIT_DECISIONS_ADMINS", "dec-admin");
    let status = |r: Result<axum::response::Response, super::super::ApiError>| match r {
        Ok(r) => r.status().as_u16(),
        Err(e) => e.status.as_u16(),
    };
    // An org admin is not a decisions admin: heads are shared across orgs.
    let r = training::train_head(State(st.clone()), Extension(user("org-admin")), Extension(rid.clone()), Path("route".into()), Query(TrainQuery::default())).await;
    assert_eq!(status(r), 403);
    let r = training::compile_head(State(st.clone()), Extension(user("org-admin")), Extension(rid.clone()), Path("route".into()), Query(TrainQuery::default())).await;
    assert_eq!(status(r), 403);
    // A decisions admin gets a queued job (202); with no data it fails.
    let r = training::train_head(State(st.clone()), Extension(user("dec-admin")), Extension(rid.clone()), Path("route".into()), Query(TrainQuery::default())).await;
    let body = |r: axum::response::Response| async move {
        assert_eq!(r.status().as_u16(), 202);
        let b = http_body_util::BodyExt::collect(r.into_body()).await.unwrap().to_bytes();
        serde_json::from_slice::<Value>(&b).unwrap()
    };
    let queued = body(r.unwrap_or_else(|_| panic!("train not accepted"))).await;
    assert_eq!(queued["status"], "queued");
    let id = queued["id"].as_str().unwrap().to_string();
    let done = wait_job(&st.db, &id).await;
    assert_eq!(done["status"], "failed");
    assert!(done["error"].as_str().unwrap().contains("training needs 20"), "{done}");
    let r = training::compile_head(State(st.clone()), Extension(user("dec-admin")), Extension(rid.clone()), Path("route".into()), Query(TrainQuery::default())).await;
    let cid = body(r.unwrap_or_else(|_| panic!("compile not accepted"))).await["id"].as_str().unwrap().to_string();
    assert_eq!(wait_job(&st.db, &cid).await["status"], "failed");
    // Jobs are readable by decisions admins only.
    let r = training::get_job(State(st.clone()), Extension(user("org-admin")), Extension(rid.clone()), Path(id.clone())).await;
    assert_eq!(status(r), 403);
    let r = training::get_job(State(st.clone()), Extension(user("dec-admin")), Extension(rid.clone()), Path(id)).await;
    assert_eq!(status(r), 200);
    let r = training::get_job(State(st.clone()), Extension(user("dec-admin")), Extension(rid.clone()), Path("dhj_nope".into())).await;
    assert_eq!(status(r), 404);
    let r = training::train_head(State(st.clone()), Extension(user("dec-admin")), Extension(rid.clone()), Path("bad kind!".into()), Query(TrainQuery::default())).await;
    assert_eq!(status(r), 400);
    // Versions are readable by anyone signed in (metrics only).
    let r = training::list_versions(State(st.clone()), Extension(user("org-admin")), Extension(rid.clone()), Path(("head.embed".into(), "route".into()))).await;
    assert_eq!(status(r), 200);
    std::env::remove_var("ALLTERNIT_DECISIONS_ADMINS");
}

/// Ticket routing in natural language, keywords never repeated verbatim
/// between classes' phrasings: needs meaning, not a word list.
fn live_route_dataset() -> Vec<Ex> {
    let classes: &[(&str, &[&str])] = &[
        ("billing", &[
            "I was charged twice for the same order", "my card got billed after I cancelled", "please send me a copy of last month's receipt",
            "the price on my statement is higher than advertised", "I want my money back for the annual plan", "why did my subscription renew without asking",
            "the discount code was not applied at checkout", "can I switch from monthly to yearly payments", "there is a fee I do not recognise on my bill",
            "I need a VAT invoice for my company", "my payment failed but the money left my account", "how do I update the credit card on file",
        ]),
        ("tech", &[
            "the app closes as soon as I open it", "I cannot sign in, the password reset email never arrives", "pages load forever and then time out",
            "the export button does nothing when I click it", "after the update my settings are gone", "the screen goes blank on my phone",
            "sync between my laptop and tablet stopped working", "I get a 500 error when uploading a file", "notifications stopped arriving",
            "the search returns nothing even for exact titles", "two factor codes are rejected as invalid", "the desktop client freezes during startup",
        ]),
        ("fraud", &[
            "someone bought things with my account that I never ordered", "I got an email pretending to be you asking for my password",
            "my card was used in another country while I was at home", "a stranger changed my account email address", "there are logins from devices I do not own",
            "a seller asked me to pay outside the platform", "my gift card balance was drained overnight", "someone is impersonating your support team on social media",
            "I think my account was hacked", "unknown people were added to my team workspace", "a refund was sent to a bank account that is not mine",
            "I received a text with a link to verify my account that looks fake",
        ]),
    ];
    let leads = ["Hi,", "Hello team,", "Urgent:", "Good morning.", "Please help:", "Quick question -"];
    let tails = ["Thanks.", "What should I do?", "This is the second time.", "Regards, Sam", "", "Please advise."];
    let opts: Vec<(String, String)> = vec![
        ("billing".into(), "Billing: charges, refunds, invoices and subscriptions".into()),
        ("tech".into(), "Technical support: bugs, crashes, login and performance problems".into()),
        ("fraud".into(), "Trust and safety: fraud, scams, account takeover and suspicious activity".into()),
    ];
    let mut out = Vec::new();
    for (team, phrases) in classes {
        for (i, p) in phrases.iter().enumerate() {
            for k in 0..6 {
                let ctx = format!("{} {p}. {}", leads[(i + k) % leads.len()], tails[(i * 5 + k) % tails.len()]);
                out.push((ctx, Some("Which team should handle this ticket?".to_string()), opts.clone(), team.to_string()));
            }
        }
    }
    out
}

/// Against the real EmbeddingGemma 2 server on ALLTERNIT_EMBED_URL (default
/// :7719). `cargo test -p allternit-api --lib live_embed_head -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn decisions_live_embed_head() {
    let client = EmbedClient::from_env();
    client.embed_remote(&["ping".into()], crate::memory_index::InputType::Query).await.expect("embedding server not reachable");
    for (kind, rows) in [("route", live_route_dataset()), ("pick", pick_dataset(240, 21))] {
        let db = DbHandle::new_memory().unwrap();
        seed(&db, kind, &rows);
        let t = std::time::Instant::now();
        let out = training::train_embed(&db, &client, kind, "adm", 10_000).await.unwrap();
        let m = &out["metrics"];
        eprintln!(
            "LIVE {kind}: model {} dim {} | {} examples ({} train / {} val) | val acc {:.4} (teacher labels {:.4}) train acc {:.4} | cosine {:.4} lookup {:.4} prior {:.4} | overconf {:+.4} | fit {} ms, total {:.1} s",
            m["embed_model"], m["embed_dim"], m["examples"], m["train_examples"], m["val_examples"],
            f(m, &["val_accuracy"]), f(m, &["val_accuracy_teacher_labels"]), f(m, &["train_accuracy"]),
            f(m, &["baselines", "cosine", "accuracy"]), f(m, &["baselines", "lookup", "accuracy"]), f(m, &["baselines", "prior", "accuracy"]),
            f(m, &["val_overconfidence"]), m["train_ms"], t.elapsed().as_secs_f64(),
        );
        assert_eq!(m["embed_model"], DEFAULT_EMBED_MODEL);
        assert!(f(m, &["val_accuracy"]) > f(m, &["baselines", "prior", "accuracy"]) + 0.3, "{m}");
        let rules = training::compile_rules(&db.connect().unwrap(), kind, "adm", 10_000, &CompileBar::default());
        eprintln!("LIVE {kind}: rules compile → {}", match &rules { Ok(v) => format!("accepted, {} rules", v["metrics"]["rules"]), Err(e) => e.1.clone() });
        // Serving latency: one embedded decision through the stored head.
        let (ctx, q, opts, label) = &rows[0];
        let texts: Vec<&str> = opts.iter().map(|o| o.1.as_str()).collect();
        let ids: Vec<&str> = opts.iter().map(|o| o.0.as_str()).collect();
        let t = std::time::Instant::now();
        let v = embed_head::embed_decision(&client, ctx, q.as_deref(), &texts).await.unwrap();
        let embed_ms = t.elapsed().as_secs_f64() * 1000.0;
        let conn = db.connect().unwrap();
        let t = std::time::Instant::now();
        let a = EmbedHead
            .predict(&conn, &HeadInput { owner: "u", kind, key: "k", ids: &ids, context: ctx, question: q.as_deref(), texts: &texts, vectors: Some(&v), decision_id: None, pin: None })
            .unwrap()
            .unwrap();
        eprintln!("LIVE {kind}: embed {embed_ms:.1} ms, predict {:.3} ms, choice {} (label {label})", t.elapsed().as_secs_f64() * 1000.0, ids[a.choice]);
    }
}

// ── replays with trained-head overrides ─────────────────────────────────────

/// A stored decision of run `run` (as `finish` stores it) with its harvested
/// example (the text a trained head re-decides from), served `served`,
/// with `right` as its outcome label.
fn run_decision(db: &DbHandle, id: &str, run: &str, kind: &str, ctx: &str, opts: &[(String, String)], served: &str, right: &str) {
    let options = json!(opts.iter().map(|(i, t)| json!({ "id": i, "text": t })).collect::<Vec<_>>());
    store::insert(db, &store::Row {
        id, owner: "u", created_at: &super::super::store::now(), kind, session_id: Some(run), task: None, context_hash: "h",
        has_image: false, options: &options, choice: Some(served), probs: &json!({}), confidence: 0.9, abstained: false,
        backend: "oracle", latency_ms: 900.0, attempts: &json!([{ "backend": "oracle", "detail": { "cost_usd": 0.002 } }]),
    })
    .unwrap();
    let s = Shadow {
        decision_id: id.into(), owner: "u".into(), kind: kind.into(), key: format!("key-{id}"),
        ids: opts.iter().map(|o| o.0.clone()).collect(), has_image: false, context: ctx.into(), question: None,
        texts: opts.iter().map(|o| o.1.clone()).collect(), vectors: None,
    };
    flywheel::record_key(db, &s).unwrap();
    db.connect()
        .unwrap()
        .execute(
            "INSERT OR REPLACE INTO decision_examples (decision_id, scope, kind, created_at, context, question, options_json, teacher_backend,
                teacher_choice, teacher_confidence) VALUES (?1, 'org:run', ?2, 't', ?3, NULL, ?4, 'oracle', ?5, 0.9)",
            params![id, kind, ctx, options.to_string(), served],
        )
        .unwrap();
    store::set_outcome(db, id, "u", "failure", Some(right), None, &super::super::store::now()).unwrap().unwrap();
}

#[test]
fn decisions_replay_with_a_rules_override_flips_a_decision() {
    use crate::agency_api::replays::{decide, parse_overrides, resolve_versions};
    let db = DbHandle::new_memory().unwrap();
    seed(&db, "route", &route_dataset(200, 11));
    let conn = db.connect().unwrap();
    training::compile_rules(&conn, "route", "adm", 10_000, &CompileBar::default()).unwrap();
    // The run served billing for a fraud ticket; the next decision of the run
    // is one the rules were compiled from (leave-one-out: no answer).
    run_decision(&db, "run_q_d1", "run_q", "route", "Someone made an unauthorized purchase on my card", &route_options(), "billing", "fraud");
    let artifact = training::load(&conn, &training::latest(&conn, rules_head::NAME, "route").unwrap().unwrap().0).unwrap();
    let i = (0..200).find(|i| artifact.trained_on(&format!("route-{i}"))).unwrap();
    assert!(!artifact.trained_on("run_q_d1"));
    let (ctx, _, opts, label) = route_dataset(200, 11).swap_remove(i);
    run_decision(&db, &format!("route-{i}"), "run_q", "route", &ctx, &opts, &label, &label);

    // Overrides: a pinned version that exists, the newest by default, a missing one refused.
    let mut ov = parse_overrides(Some(&json!({ "decision_heads": { "route": { "head": "head.rules", "version": 1 } } }))).unwrap();
    resolve_versions(&conn, &mut ov).unwrap();
    assert_eq!((ov.heads[0].pin, ov.heads[0].version.as_str()), (Some(1), "1"));
    assert_eq!(ov.echo()["decision_heads"]["route"], json!({ "head": "head.rules", "version": "1" }));
    let mut latest = parse_overrides(Some(&json!({ "decision_heads": { "route": "head.rules" } }))).unwrap();
    resolve_versions(&conn, &mut latest).unwrap();
    assert_eq!(latest.heads[0].pin, Some(1));
    let mut missing = parse_overrides(Some(&json!({ "decision_heads": { "route": { "head": "head.rules", "version": "7" } } }))).unwrap();
    assert_eq!(resolve_versions(&conn, &mut missing).unwrap_err().status, 400);
    assert!(parse_overrides(Some(&json!({ "decision_heads": { "route": { "head": "head.rules", "version": "x" } } }))).is_err());

    let h = &ov.heads[0];
    let mut recorded = decide::load(&conn, "u", "run_q").unwrap();
    assert_eq!(recorded.len(), 2);
    recorded.sort_by_key(|d| d.id != "run_q_d1");
    assert!(recorded[0].context.is_some() && recorded[0].texts.len() == 3);
    let r = decide::redecide_with(&conn, "u", &recorded[0], h.head, h.pin, 3).unwrap();
    assert_eq!(r.samples.len(), 1, "deterministic: one sample");
    let (v, row) = decide::compare_with(&recorded[0], h.head, &h.version, &r);
    assert_eq!((v["candidate"]["choice"].as_str(), v["effective_choice"].as_str()), (Some("fraud"), Some("fraud")), "{v}");
    assert_eq!((v["correct"].as_bool(), v["served_correct"].as_bool(), v["candidate"]["version"].as_str()), (Some(true), Some(false), Some("1")));
    assert!(row.confident && row.cost_usd == 0.0);
    // A decision the version was compiled from: left out, the served choice stands.
    let r = decide::redecide_with(&conn, "u", &recorded[1], h.head, h.pin, 1).unwrap();
    assert_eq!(r.samples, vec![None]);
    // Without its harvested text, a trained head has no answer.
    let mut bare = recorded[0].clone();
    bare.context = None;
    assert_eq!(decide::redecide_with(&conn, "u", &bare, h.head, h.pin, 1).unwrap().samples, vec![None]);
    // Nothing written by the replay.
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM decision_shadow", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn decisions_replay_with_an_embed_override_embeds_before_redeciding() {
    use crate::agency_api::replays::{decide, parse_overrides, resolve_versions};
    let db = DbHandle::new_memory().unwrap();
    seed(&db, "pick", &pick_dataset(120, 9));
    let client = EmbedClient::new(Some(bow_server().await), DEFAULT_EMBED_MODEL);
    training::train_embed(&db, &client, "pick", "adm", 10_000).await.unwrap();
    training::train_embed(&db, &client, "pick", "adm", 10_000).await.unwrap();
    let opts: Vec<(String, String)> = [("o0", "box for kalomi work"), ("o1", "kit for nerupu work"), ("o2", "tool for sotiva work")]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    run_decision(&db, "run_e_d1", "run_e", "pick", "please I need help with sotiva today", &opts, "o0", "o2");
    let conn = db.connect().unwrap();
    let mut ov = parse_overrides(Some(&json!({ "decision_heads": { "pick": { "head": "head.embed", "version": "1" } } }))).unwrap();
    resolve_versions(&conn, &mut ov).unwrap();
    let h = ov.heads[0].clone();
    assert_eq!(h.version, "1", "pinned below the newest (2)");
    let mut recorded = decide::load(&conn, "u", "run_e").unwrap();
    // Before the async embedding step: no vectors, no answer.
    assert_eq!(decide::redecide_with(&conn, "u", &recorded[0], h.head, h.pin, 1).unwrap().samples, vec![None]);
    decide::prepare(&mut recorded, &|kind| ov.head_for(kind), &client).await;
    assert_eq!(recorded[0].vectors.as_ref().map(|v| v.model.as_str()), Some("test/bow-256"));
    let r = decide::redecide_with(&conn, "u", &recorded[0], h.head, h.pin, 1).unwrap();
    let (v, _) = decide::compare_with(&recorded[0], h.head, &h.version, &r);
    assert_eq!(v["candidate"]["choice"], "o2", "{v}");
    assert_eq!(v["candidate"]["version"], "1");
    // Hash vectors (embedding server down) never meet a bow-trained version.
    let mut hashed = recorded.clone();
    hashed[0].vectors = None;
    decide::prepare(&mut hashed, &|kind| ov.head_for(kind), &EmbedClient::new(None, DEFAULT_EMBED_MODEL)).await;
    assert_eq!(decide::redecide_with(&conn, "u", &hashed[0], h.head, h.pin, 1).unwrap().samples, vec![None]);
}
