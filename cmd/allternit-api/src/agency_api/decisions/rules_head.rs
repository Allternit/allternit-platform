//! `head.rules`: S1 → S0 compile (Agency Descent C1).
//!
//! Some kinds are deterministic functions of plain features ("a ticket that
//! mentions a refund goes to billing"). For those the flywheel compiles the
//! labelled dataset into a readable **decision list**: ordered rules
//! `if <condition on the state> then <the option the selector picks>`. It
//! answers with zero model calls and no embeddings: tier S0 (`rules` in
//! `/v1/descent`).
//!
//! - **Features** (transparent): a word in the state (context + question;
//!   lowercase alphanumeric tokens of 2+ characters, the first
//!   [`MAX_STATE_TOKENS`] distinct ones), a word of an option's id or text
//!   that names exactly one of the decision's options, the option's id, the
//!   option position.
//! - **Learning**: greedy sequential covering. Each step takes the rule with
//!   no error on the examples not yet covered and the largest support (at
//!   least `min_support`); ties go to the earliest condition and selector in
//!   sorted order, so the result is deterministic. A rule fires only when its
//!   condition holds and its selector picks exactly one option; the first
//!   rule that fires answers.
//! - **Acceptance**: the list must be 100% accurate on its training split
//!   (every example covered, which is the stopping condition), have at most
//!   `max_rules` rules, and reach `val_accuracy` on the validation split over
//!   at least `min_samples` examples and `min_val` held-out ones. Otherwise
//!   nothing is stored.

use super::flywheel::{Head, HeadAnswer, HeadInput};
use super::training::{self, Item};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

pub const NAME: &str = "head.rules";
pub const FORMAT: &str = "decision_list/v1";
pub const MAX_STATE_TOKENS: usize = 2048;
/// Selector id spaces, in order of preference on ties: option id, option
/// word, position.
const WORD_SPACE: u32 = 1 << 24;
const POS_SPACE: u32 = 2 << 24;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cond {
    Always,
    StateHas(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Select {
    /// The option with this id.
    Option(String),
    /// The one option whose id or text has this word.
    OptionHas(String),
    Position(usize),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    #[serde(rename = "if")]
    pub cond: Cond,
    pub then: Select,
    /// Training examples this rule answered (all of them right).
    pub support: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuleList {
    pub format: String,
    pub max_state_tokens: usize,
    pub rules: Vec<Rule>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CompileBar {
    pub min_samples: usize,
    pub min_val: usize,
    pub val_accuracy: f64,
    pub min_support: usize,
    pub max_rules: usize,
}

impl Default for CompileBar {
    fn default() -> Self {
        Self { min_samples: 50, min_val: 10, val_accuracy: 0.98, min_support: 3, max_rules: 32 }
    }
}

/// Lowercase alphanumeric words of 2+ characters, distinct, in order.
pub fn tokens(text: &str, cap: usize) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for t in text.split(|c: char| !c.is_alphanumeric()).filter(|t| t.chars().count() >= 2) {
        let t = t.to_lowercase();
        if seen.insert(t.clone()) {
            out.push(t);
            if out.len() >= cap {
                break;
            }
        }
    }
    out
}

fn option_tokens(id: &str, text: &str) -> BTreeSet<String> {
    tokens(&format!("{id} {text}"), usize::MAX).into_iter().collect()
}

impl RuleList {
    /// The first rule that fires: (option index, its rule's support).
    pub fn predict(&self, state: &str, options: &[(&str, &str)]) -> Option<(usize, usize)> {
        let st: BTreeSet<String> = tokens(state, self.max_state_tokens).into_iter().collect();
        let opt: Vec<BTreeSet<String>> = options.iter().map(|(id, text)| option_tokens(id, text)).collect();
        for r in &self.rules {
            let holds = match &r.cond {
                Cond::Always => true,
                Cond::StateHas(t) => st.contains(t),
            };
            if !holds {
                continue;
            }
            let picked = match &r.then {
                Select::Position(p) => (*p < options.len()).then_some(*p),
                Select::Option(id) => options.iter().position(|(o, _)| o == id),
                Select::OptionHas(t) => {
                    let hits: Vec<usize> = (0..opt.len()).filter(|&j| opt[j].contains(t)).collect();
                    (hits.len() == 1).then(|| hits[0])
                }
            };
            if let Some(j) = picked {
                return Some((j, r.support));
            }
        }
        None
    }
}

/// Smoothed by the rule's support: 3 → 0.8, 100 → 0.99.
pub fn rule_probs(n: usize, i: usize, support: usize) -> Vec<f64> {
    super::flywheel::lookup_probs(n, i, support as i64, 0)
}

/// Learn a decision list that answers every training example right, or
/// say why there is none.
pub fn learn(train: &[&Item], bar: &CompileBar) -> Result<RuleList, String> {
    // Interned, sorted vocabularies: ids order like the strings, so tie-breaks
    // on ids are lexicographic.
    let st_tok: Vec<Vec<String>> = train.iter().map(|e| tokens(&e.state, MAX_STATE_TOKENS)).collect();
    let op_tok: Vec<Vec<BTreeSet<String>>> =
        train.iter().map(|e| e.options.iter().map(|(id, t)| option_tokens(id, t)).collect()).collect();
    let svocab: BTreeSet<&str> = st_tok.iter().flatten().map(String::as_str).collect();
    let svocab: Vec<&str> = svocab.into_iter().collect();
    let sid: HashMap<&str, u32> = svocab.iter().enumerate().map(|(i, t)| (*t, i as u32 + 1)).collect();
    let ovocab: BTreeSet<&str> = op_tok.iter().flatten().flatten().map(String::as_str).collect();
    let ovocab: Vec<&str> = ovocab.into_iter().collect();
    let oid: HashMap<&str, u32> = ovocab.iter().enumerate().map(|(i, t)| (*t, WORD_SPACE + i as u32)).collect();
    let ivocab: BTreeSet<&str> = train.iter().flat_map(|e| e.options.iter().map(|(id, _)| id.as_str())).collect();
    let ivocab: Vec<&str> = ivocab.into_iter().collect();
    let iid: HashMap<&str, u32> = ivocab.iter().enumerate().map(|(i, t)| (*t, i as u32)).collect();

    // Per example: condition ids (0 = always) and the selectors that pick a
    // unique option: option ids, option words, then positions.
    let conds: Vec<Vec<u32>> = st_tok.iter().map(|ts| std::iter::once(0).chain(ts.iter().map(|t| sid[t.as_str()])).collect()).collect();
    let sels: Vec<Vec<(u32, usize)>> = op_tok
        .iter()
        .zip(train)
        .map(|(opts, item)| {
            let mut count: HashMap<&str, (usize, usize)> = HashMap::new();
            for (j, ts) in opts.iter().enumerate() {
                for t in ts {
                    count.entry(t.as_str()).and_modify(|c| c.0 += 1).or_insert((1, j));
                }
            }
            let mut v: Vec<(u32, usize)> = count.into_iter().filter(|(_, (n, _))| *n == 1).map(|(t, (_, j))| (oid[t], j)).collect();
            v.extend((0..opts.len()).map(|j| (POS_SPACE + j as u32, j)));
            // Ids are unique within a decision (validated on the way in).
            v.extend(item.options.iter().enumerate().map(|(j, (id, _))| (iid[id.as_str()], j)));
            v.sort_unstable();
            v
        })
        .collect();

    let mut remaining: Vec<usize> = (0..train.len()).collect();
    let mut rules = Vec::new();
    while !remaining.is_empty() {
        if rules.len() >= bar.max_rules {
            return Err(format!("needs more than {} rules ({} examples left uncovered)", bar.max_rules, remaining.len()));
        }
        let mut counts: HashMap<(u32, u32), (usize, usize)> = HashMap::new();
        for &e in &remaining {
            for &c in &conds[e] {
                for &(s, j) in &sels[e] {
                    let x = counts.entry((c, s)).or_default();
                    if j == train[e].label {
                        x.0 += 1;
                    } else {
                        x.1 += 1;
                    }
                }
            }
        }
        let best = counts
            .into_iter()
            .filter(|(_, (right, wrong))| *wrong == 0 && *right >= bar.min_support)
            .min_by(|(ka, (ra, _)), (kb, (rb, _))| rb.cmp(ra).then(ka.cmp(kb)));
        let Some(((c, s), (support, _))) = best else {
            return Err(format!("no error-free rule with support ≥ {} for {} remaining examples", bar.min_support, remaining.len()));
        };
        remaining.retain(|&e| !(conds[e].contains(&c) && sels[e].iter().any(|&(x, _)| x == s)));
        rules.push(Rule {
            cond: if c == 0 { Cond::Always } else { Cond::StateHas(svocab[c as usize - 1].to_string()) },
            then: match s {
                s if s >= POS_SPACE => Select::Position((s - POS_SPACE) as usize),
                s if s >= WORD_SPACE => Select::OptionHas(ovocab[(s - WORD_SPACE) as usize].to_string()),
                s => Select::Option(ivocab[s as usize].to_string()),
            },
            support,
        });
    }
    Ok(RuleList { format: FORMAT.into(), max_state_tokens: MAX_STATE_TOKENS, rules })
}

fn accuracy(rules: &RuleList, set: &[&Item]) -> (f64, f64) {
    if set.is_empty() {
        return (0.0, 0.0);
    }
    let (mut right, mut answered) = (0usize, 0usize);
    for e in set {
        let opts: Vec<(&str, &str)> = e.options.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        if let Some((j, _)) = rules.predict(&e.state, &opts) {
            answered += 1;
            right += (j == e.label) as usize;
        }
    }
    (right as f64 / set.len() as f64, answered as f64 / set.len() as f64)
}

/// A compile attempt: the rules when accepted, and the metrics either way.
pub struct Compiled {
    pub rules: Option<RuleList>,
    pub metrics: Value,
}

pub fn compile(items: &[Item], bar: &CompileBar) -> Compiled {
    let (train, val) = training::split(items);
    let mut metrics = json!({
        "examples": items.len(), "train_examples": train.len(), "val_examples": val.len(), "bar": bar,
        "baselines": training::baselines(&train, &val),
    });
    let reject = |mut m: Value, why: String| {
        m["accepted"] = json!(false);
        m["reason"] = json!(why);
        Compiled { rules: None, metrics: m }
    };
    if items.len() < bar.min_samples || val.len() < bar.min_val {
        return reject(metrics, format!("needs {} examples with {} held out (has {} / {})", bar.min_samples, bar.min_val, items.len(), val.len()));
    }
    let rules = match learn(&train, bar) {
        Ok(r) => r,
        Err(why) => return reject(metrics, format!("not 100% on its training set: {why}")),
    };
    let (train_acc, _) = accuracy(&rules, &train);
    let (val_acc, val_cov) = accuracy(&rules, &val);
    metrics["rules"] = json!(rules.rules.len());
    metrics["train_accuracy"] = json!(train_acc);
    metrics["val_accuracy"] = json!(val_acc);
    metrics["val_coverage"] = json!(val_cov);
    if train_acc < 1.0 {
        return reject(metrics, format!("train accuracy {train_acc:.4} < 1"));
    }
    if val_acc < bar.val_accuracy {
        return reject(metrics, format!("validation accuracy {val_acc:.4} < {}", bar.val_accuracy));
    }
    metrics["accepted"] = json!(true);
    Compiled { rules: Some(rules), metrics }
}

// ── serving ─────────────────────────────────────────────────────────────────

pub fn latest_rules(conn: &Connection, kind: &str) -> rusqlite::Result<Option<Arc<RuleList>>> {
    static CACHE: Mutex<Option<HashMap<String, (String, Arc<RuleList>)>>> = Mutex::new(None);
    let Some((id, _)) = training::latest(conn, NAME, kind)? else { return Ok(None) };
    if let Some((_, r)) = CACHE.lock().ok().and_then(|c| c.as_ref().and_then(|c| c.get(kind).cloned())).filter(|(cid, _)| *cid == id) {
        return Ok(Some(r));
    }
    let a = training::load(conn, &id)?;
    let Some(r) = a.rules_json.as_deref().and_then(|s| serde_json::from_str::<RuleList>(s).ok()) else {
        tracing::warn!(kind, id, "decision head.rules: malformed artifact");
        return Ok(None);
    };
    let r = Arc::new(r);
    if let Ok(mut c) = CACHE.lock() {
        c.get_or_insert_with(HashMap::new).insert(kind.to_string(), (id, r.clone()));
    }
    Ok(Some(r))
}

pub struct RulesHead;

impl Head for RulesHead {
    fn name(&self) -> &'static str {
        NAME
    }
    fn applies(&self, _kind: &str) -> bool {
        true
    }
    fn ready(&self, conn: &Connection, kind: &str) -> bool {
        training::latest(conn, NAME, kind).ok().flatten().is_some()
    }
    fn predict(&self, conn: &Connection, input: &HeadInput<'_>) -> rusqlite::Result<Option<HeadAnswer>> {
        let Some(rules) = latest_rules(conn, input.kind)? else { return Ok(None) };
        let state = super::embed_head::state_text(input.context, input.question);
        let opts: Vec<(&str, &str)> = input.ids.iter().zip(input.texts).map(|(a, b)| (*a, *b)).collect();
        if opts.len() != input.ids.len() {
            return Ok(None);
        }
        Ok(rules.predict(&state, &opts).map(|(j, support)| HeadAnswer { choice: j, probs: rule_probs(opts.len(), j, support) }))
    }
}
