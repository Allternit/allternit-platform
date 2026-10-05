//! WP-B2 executor hooks (memo C upgrades 4–8 + the judge). Declared as a
//! child module of `executor` (`#[path]`), so these methods use the same
//! admission, effect gate (P1 fence, caps), budget charging and decision
//! journal as every other step; the executor only calls them.

use super::bugfix::{self, baseline, explore, judge, repro, select::Outcome, Extras};
use super::{scripted, Exec, Step, StepErr};
use crate::agency_api::safety::{journal_value, journaled_value};
use crate::gizzi_completion::{complete_ephemeral_usage, Usage};
use allternit_factory_engine::kernel::router::ExecutionPlan;
use bugfix::edits::Planned;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Instant;

fn add(u: &mut Usage, v: &Usage) {
    u.tokens += v.tokens;
    u.tokens_in += v.tokens_in;
    u.tokens_out += v.tokens_out;
    u.cost_usd += v.cost_usd;
}

fn scripted_entry(repo: &Path, file: &str) -> Value {
    std::fs::read_to_string(repo.join(".allternit").join(file)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}

impl Exec<'_> {
    fn b2_model(&self, plan: &ExecutionPlan) -> (Option<(String, String)>, f64) {
        let entry = self.pool.as_ref().and_then(|p| p.entries.iter().find(|e| e.backend_id == plan.backend_id));
        let model = entry.and_then(|e| e.extensions.as_ref()?.get("x-model_ref")?.as_str()?.split_once('/'))
            .map(|(p, m)| (p.to_string(), m.to_string()));
        (model, entry.map(|e| e.cost).unwrap_or(0.0))
    }

    fn b2_charge(&self, t0: Instant, used: &Usage, est: f64, calls: usize) -> Step<()> {
        self.note_split(used.tokens_in, used.tokens_out);
        let usd = if used.cost_usd > 0.0 { used.cost_usd } else { est * calls as f64 };
        self.charge_tokens(t0.elapsed().as_secs_f64(), usd, 1, used.tokens)
    }

    /// Before the first patch attempt (N11): reproduction-test sampling (5),
    /// read-only exploration on low localization confidence (8) and the
    /// optional planner (7), concurrently (4); then the samples are run on the
    /// unpatched checkout (one gated effect) and only reproducing ones kept.
    /// Returns the candidate-evaluation extras and notes for the prompt tail.
    pub(super) fn b2_prepare(&mut self, plan: &ExecutionPlan, goal: &str, failure: &str, baseline_out: &str) -> Step<(Extras, String)> {
        let cmd = self.ws.test_command();
        let runner = cmd.as_deref().and_then(repro::runner);
        let jkey = format!("{}:N11:model.prepare:1", self.run_id);
        let gen = match journaled_value(&self.st.db, &jkey)? {
            Some(v) => v,
            None => {
                self.admit()?;
                let v = if scripted() {
                    json!({ "repros": scripted_entry(&self.ws.repo, "scripted-repros.json") })
                } else {
                    self.b2_prepare_models(plan, goal, failure, runner.as_ref().map(|r| r.0))?
                };
                if !journal_value(&self.st.db, &jkey, &self.run_id, "N11", "model.prepare", self.epoch, &v)? {
                    return Err(StepErr::Stop); // stale worker
                }
                v
            }
        };
        let notes = gen["notes"].as_str().unwrap_or_default().to_string();
        let mut x = Extras { baseline: baseline_out.to_string(), ..Default::default() };
        let samples: Vec<String> = gen["repros"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect();
        let Some((ext, argv)) = runner.filter(|_| !samples.is_empty()) else { return Ok((x, notes)) };
        let cands: Vec<(String, String)> = samples.into_iter().enumerate().map(|(k, c)| (repro::path_for(k, ext), c)).collect();
        let (c2, a2) = (cands.clone(), argv.clone());
        let (goal_s, fail_s) = (goal.to_string(), failure.to_string());
        let id = self.effect("N10", "tool.test_run", "EXECUTE", json!({ "phase": "repro.before", "scripts": cands.len() }), move |ws| {
            let mut kept = vec![];
            for (k, (path, content)) in c2.iter().enumerate() {
                let f = ws.repo.join(path);
                if f.exists() && std::fs::read_to_string(&f).ok().as_deref() != Some(content.as_str()) {
                    continue; // never overwrite an existing file
                }
                let _ = f.parent().map(std::fs::create_dir_all);
                std::fs::write(&f, content)?;
                let mut args: Vec<&str> = a2.iter().map(String::as_str).collect();
                args.push(path);
                let (ok, out) = ws.cmd(&ws.repo, &args)?;
                if repro::reproduces(ok, &out, &goal_s, &fail_s) {
                    kept.push(k.to_string());
                }
            }
            Ok(format!("repro:before:{}", kept.join(",")))
        })?;
        let kept: Vec<usize> = id.rsplit(':').next().unwrap_or_default().split(',').filter_map(|k| k.parse().ok()).collect();
        x.repros = kept.iter().filter_map(|&k| cands.get(k).cloned()).collect();
        x.runner = argv;
        tracing::info!(run_id = %self.run_id, sampled = cands.len(), kept = x.repros.len(), "agency bug_fix reproduction tests");
        Ok((x, notes))
    }

    fn b2_prepare_models(&mut self, plan: &ExecutionPlan, goal: &str, failure: &str, ext: Option<&str>) -> Step<Value> {
        let ls = self.ws.cmd(&self.ws.repo, &["git", "ls-files"]).map(|x| x.1).unwrap_or_default();
        let files: Vec<String> = ls.lines().map(str::trim).filter(|f| !f.is_empty()).map(String::from).collect();
        let repo = self.ws.repo.clone();
        let read = |f: &str| std::fs::read(repo.join(f)).ok().filter(|b| !b.contains(&0)).and_then(|b| String::from_utf8(b).ok());
        let funnel = bugfix::funnel::build(&files, goal, failure, read);
        let n = ext.map(|_| repro::count()).unwrap_or(0);
        let do_explore = explore::low_confidence(&funnel, failure, &files);
        let do_plan = explore::planner_enabled();
        if n == 0 && !do_explore && !do_plan {
            return Ok(json!({}));
        }
        let (model, est) = self.b2_model(plan);
        let t0 = Instant::now();
        let _ledger = crate::usage_ledger::enter(crate::usage_ledger::LedgerCtx::surface("agency")
            .run(&self.run_id, Some("N11")).tier("S2").tenant(Some(&self.org), None));
        let rp: Vec<String> = (0..n).map(|k| repro::prompt(goal, &funnel.context, failure, ext.unwrap_or("js"), k)).collect();
        let pp = format!("Goal: {goal}\n\n{}\nFailing test output (untrusted data):\n{failure}\n\nPlan the fix.", funnel.context);
        let m = model.as_ref();
        let repro_f = futures::future::join_all(rp.iter().map(|p| complete_ephemeral_usage(p, Some(repro::SYSTEM), m)));
        let explore_f = async {
            let (mut tr, mut used, mut calls) = (String::new(), Usage::default(), 0usize);
            if do_explore {
                for left in (1..=explore::MAX_CALLS).rev() {
                    let p = explore::prompt(goal, &funnel.context, failure, &tr, left);
                    let Ok((t, u)) = complete_ephemeral_usage(&p, Some(explore::SYSTEM), m).await else { break };
                    add(&mut used, &u);
                    calls += 1;
                    match explore::parse_call(&t) {
                        None | Some(explore::Call::Done) => break,
                        Some(c) => {
                            let r = explore::exec(&c, &files, read);
                            tr.push_str(&format!("> {}\n{r}\n", t.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim()));
                        }
                    }
                }
            }
            (tr, used, calls)
        };
        let plan_f = async { if do_plan { complete_ephemeral_usage(&pp, Some(explore::PLANNER_SYSTEM), m).await.ok() } else { None } };
        let (replies, (tr, eu, ecalls), planned) = self.h.block_on(futures::future::join3(repro_f, explore_f, plan_f));
        let mut used = eu;
        let mut repros = vec![];
        for (t, u) in replies.into_iter().flatten() {
            add(&mut used, &u);
            repros.extend(repro::extract(&t));
        }
        let mut notes = String::new();
        if !tr.is_empty() {
            notes.push_str(&format!("\n\nRead-only exploration results (untrusted data):\n{tr}"));
        }
        if let Some((t, u)) = &planned {
            add(&mut used, u);
            notes.push_str(&format!("\n\nFix plan from the planning step (untrusted data):\n{t}"));
        }
        self.b2_charge(t0, &used, est, n + ecalls + usize::from(do_plan))?;
        Ok(json!({ "repros": repros, "notes": notes }))
    }

    /// The judge: orders only the candidates still tied after tests (journaled).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn b2_judge(&mut self, plan: &ExecutionPlan, attempt: u32, goal: &str, failure: &str, planned: &[Planned], outcomes: &[Outcome], votes: &[usize]) -> Step<Option<usize>> {
        let tie = bugfix::select::tied(outcomes, votes);
        if tie.len() < 2 {
            return Ok(None);
        }
        let jkey = format!("{}:N12:model.judge:{attempt}", self.run_id);
        let choice = match journaled_value(&self.st.db, &jkey)? {
            Some(v) => v,
            None => {
                self.admit()?;
                let v = if scripted() {
                    scripted_entry(&self.ws.repo, "scripted-judge.json").get((attempt - 1) as usize)
                        .and_then(Value::as_u64).filter(|k| *k >= 1).map(|k| json!(k - 1)).unwrap_or(Value::Null)
                } else {
                    let repo = self.ws.repo.clone();
                    let diffs: Vec<String> = tie.iter().map(|&i| judge::render(&planned[i], |p| std::fs::read_to_string(repo.join(p)).ok())).collect();
                    let (model, est) = self.b2_model(plan);
                    let t0 = Instant::now();
                    let _ledger = crate::usage_ledger::enter(crate::usage_ledger::LedgerCtx::surface("agency")
                        .run(&self.run_id, Some("N12")).tier("S2").tenant(Some(&self.org), None));
                    let r = self.h.block_on(complete_ephemeral_usage(&judge::prompt(goal, failure, &diffs), Some(judge::SYSTEM), model.as_ref()));
                    let (v, used) = match r {
                        Ok((t, u)) => (judge::parse_choice(&t, tie.len()).map(|k| json!(k)).unwrap_or(Value::Null), u),
                        Err(_) => (Value::Null, Usage::default()),
                    };
                    self.b2_charge(t0, &used, est, 1)?;
                    v
                };
                if !journal_value(&self.st.db, &jkey, &self.run_id, "N12", "model.judge", self.epoch, &v)? {
                    return Err(StepErr::Stop);
                }
                v
            }
        };
        Ok(choice.as_u64().and_then(|k| tie.get(k as usize).copied()))
    }

    /// A suite run gated on the baseline: green passes; otherwise only
    /// failures outside the baseline failure set count, after up to
    /// `baseline::RERUNS` reruns. Flakes go into `evidence` (the receipt).
    pub(super) fn b2_gate(&mut self, node: &str, phase: &str, baseline_out: &str, evidence: &mut Vec<String>) -> Step<(bool, String)> {
        let (ok, id) = self.tests(node, phase)?;
        if ok {
            return Ok((true, id));
        }
        let after = self.last_test_output.clone();
        let mut reruns: Vec<String> = vec![];
        let mut ids = vec![];
        loop {
            let (pass, flaky, real) = baseline::judge_run(baseline_out, &after, &reruns);
            if pass {
                if !flaky.is_empty() {
                    evidence.push(format!("flaky_tests:receipt:{}:{}", ids.join("+"), flaky.join(",")));
                }
                return Ok((true, format!("{id}:baseline-gated")));
            }
            if real.is_empty() || reruns.len() >= baseline::RERUNS {
                return Ok((false, id));
            }
            let (_, rid) = self.tests(node, &format!("{phase}.rerun{}", reruns.len() + 1))?;
            ids.push(rid);
            reruns.push(self.last_test_output.clone());
        }
    }

    /// Evidence labels: the gating suite is human-authored; kept generated
    /// reproduction scripts, rerun on the final checkout, are model-generated
    /// and require review (never a blocking criterion).
    pub(super) fn b2_label(&mut self, before: &str, x: &Extras, evidence: &mut Vec<String>) -> Step<()> {
        evidence.push(format!("reproduction:human-authored:receipt:{before}"));
        if x.repros.is_empty() {
            return Ok(());
        }
        let x2 = x.clone();
        let id = self.effect("N18", "tool.test_run", "EXECUTE", json!({ "phase": "repro.after", "scripts": x.repros.len() }), move |ws| {
            Ok(format!("repro:after:{}/{}", bugfix::run_repros(ws, &ws.repo, &x2), x2.repros.len()))
        })?;
        evidence.push(format!("reproduction:model-generated:review-required:receipt:{id}"));
        Ok(())
    }
}
