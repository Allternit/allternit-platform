//! Retrieval funnel for the patch step (memo C, upgrade 2): repo map →
//! candidate files → focused snippets, instead of inlining every tracked file.
//!
//! * Index: a symbol skeleton per source file (definition lines) plus a
//!   BM25 index over identifier terms, rebuilt per checkout (cheap, no deps).
//! * Candidates: paths named in the failing output (stack traces, failing
//!   test paths), BM25 top-k on the goal + failure terms, and import-graph
//!   neighbours of the strongest hits.
//! * Context: whole small files, else the definition / hit lines ±20 lines,
//!   under the same byte caps as before. The model sees a short repo map for
//!   everything else.

use regex::Regex;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::OnceLock;

pub const FILE_MAX: usize = 16 * 1024;
pub const TOTAL_MAX: usize = 48 * 1024;
const INDEX_FILE_MAX: usize = 256 * 1024;
const MAP_MAX: usize = 6 * 1024;
const TOP_K: usize = 6;
const WINDOW: usize = 20;
const SMALL_FILE_LINES: usize = 200;

const SOURCE_EXT: &[&str] = &["rs", "js", "mjs", "cjs", "ts", "tsx", "jsx", "py", "go", "java", "kt", "rb", "php", "c", "h", "cc", "cpp", "hpp", "cs", "swift", "scala", "sh", "json", "toml", "yaml", "yml"];
const STOP: &[&str] = &["the", "and", "for", "not", "with", "this", "that", "from", "error", "test", "tests", "fail", "failed", "failing", "expected", "true", "false", "null", "none", "return", "const", "function", "self", "line", "file", "node", "exports", "require", "module", "assert", "console", "process", "exit", "pass", "should", "fix", "bug"];

struct Doc {
    path: String,
    text: String,
    terms: HashMap<String, usize>,
    len: usize,
}

fn ident_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[A-Za-z_][A-Za-z0-9_]{2,}").unwrap())
}

fn def_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\s*(?:pub(?:\([a-z]+\))?\s+)?(?:export\s+)?(?:default\s+)?(?:async\s+)?(?:fn|def|class|function|func|struct|enum|trait|impl|interface|type|mod)\b|^\s*(?:exports|module\.exports)\.?[A-Za-z_]*\s*=|^\s*(?:export\s+)?(?:const|let|var)\s+[A-Za-z_$][\w$]*\s*=\s*(?:async\s*)?(?:\(|function)").unwrap())
}

fn import_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?:require\(\s*['"]([^'"]+)['"]|from\s+['"]([^'"]+)['"]|import\s+['"]([^'"]+)['"]|^\s*from\s+([\w.]+)\s+import|^\s*import\s+([\w.]+)|^\s*(?:pub\s+)?mod\s+(\w+)\s*;)"#).unwrap())
}

/// Identifier terms of a text (lower-cased, split on snake/camel case too).
pub fn terms(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for m in ident_re().find_iter(text) {
        let w = m.as_str();
        out.push(w.to_ascii_lowercase());
        let mut part = String::new();
        for c in w.chars() {
            if (c == '_' || c.is_ascii_uppercase()) && part.len() >= 3 {
                out.push(std::mem::take(&mut part).to_ascii_lowercase());
            } else if c == '_' {
                part.clear();
            }
            if c != '_' {
                part.push(c);
            }
        }
        if part.len() >= 3 && part.len() < w.len() {
            out.push(part.to_ascii_lowercase());
        }
    }
    out.retain(|t| !STOP.contains(&t.as_str()));
    out
}

/// Repo-relative paths (and line numbers) the failure output names.
pub fn mentioned_paths(failure: &str, files: &[String]) -> BTreeMap<String, BTreeSet<usize>> {
    let mut out: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    let line_re = Regex::new(r":(\d+)(?::\d+)?").unwrap();
    for f in files {
        let base = f.rsplit('/').next().unwrap_or(f);
        let by_path = failure.match_indices(f.as_str()).map(|(i, _)| i + f.len()).collect::<Vec<_>>();
        let hits = if !by_path.is_empty() {
            by_path
        } else if base.contains('.') && base.len() > 3 {
            // Basename only, as a whole token (not a suffix of another name).
            failure.match_indices(base).filter(|(i, _)| *i == 0 || !failure.as_bytes()[i - 1].is_ascii_alphanumeric()).map(|(i, _)| i + base.len()).collect()
        } else {
            vec![]
        };
        for end in hits {
            let e = out.entry(f.clone()).or_default();
            if let Some(c) = line_re.captures(&failure[end..failure.len().min(end + 16)]).filter(|c| c.get(0).is_some_and(|m| m.start() == 0)) {
                if let Ok(n) = c[1].parse() {
                    e.insert(n);
                }
            }
        }
    }
    out
}

fn resolve_import(from: &str, spec: &str, files: &BTreeSet<&str>) -> Option<String> {
    let dir = Path::new(from).parent().unwrap_or(Path::new(""));
    let raw = if spec.starts_with('.') { dir.join(spec) } else { Path::new(&spec.replace('.', "/")).to_path_buf() };
    let mut norm: Vec<String> = vec![];
    for c in raw.components() {
        match c {
            std::path::Component::ParentDir => { norm.pop(); }
            std::path::Component::Normal(s) => norm.push(s.to_string_lossy().into()),
            _ => {}
        }
    }
    let base = norm.join("/");
    let local_mod = dir.join(spec).to_string_lossy().to_string();
    for cand in [base.clone(), format!("{base}.js"), format!("{base}.ts"), format!("{base}.tsx"), format!("{base}.py"), format!("{base}/index.js"),
                 format!("{base}/index.ts"), format!("{base}/__init__.py"), format!("{local_mod}.rs"), format!("{local_mod}/mod.rs")] {
        if files.contains(cand.as_str()) {
            return Some(cand);
        }
    }
    None
}

/// What the funnel chose: files in rank order with why, and the compiled context.
#[derive(Debug, Clone, Default)]
pub struct Funnel {
    pub ranked: Vec<(String, f64)>,
    pub context: String,
}

/// Build the patch-step context for `goal` + `failure` over the checkout.
/// `read` returns a file's text (None for binary / unreadable).
pub fn build(files: &[String], goal: &str, failure: &str, read: impl Fn(&str) -> Option<String>) -> Funnel {
    let docs: Vec<Doc> = files.iter().filter(|f| {
        Path::new(f).extension().and_then(|e| e.to_str()).is_some_and(|e| SOURCE_EXT.contains(&e)) || !f.contains('.')
    }).filter_map(|f| {
        let text = read(f).filter(|t| t.len() <= INDEX_FILE_MAX)?;
        let mut tf = HashMap::new();
        let ts = terms(&text);
        for t in &ts {
            *tf.entry(t.clone()).or_insert(0) += 1;
        }
        Some(Doc { path: f.clone(), len: ts.len().max(1), terms: tf, text })
    }).collect();
    let n = docs.len().max(1) as f64;
    let avg = docs.iter().map(|d| d.len).sum::<usize>() as f64 / n;
    let mut q: Vec<String> = terms(goal);
    q.extend(terms(failure));
    q.sort();
    q.dedup();
    let df = |t: &str| docs.iter().filter(|d| d.terms.contains_key(t)).count() as f64;
    let mut score: HashMap<String, f64> = HashMap::new();
    for d in &docs {
        let mut s = 0.0;
        for t in &q {
            let Some(&f) = d.terms.get(t) else { continue };
            let idf = ((n - df(t) + 0.5) / (df(t) + 0.5) + 1.0).ln();
            let f = f as f64;
            s += idf * f * 2.2 / (f + 1.2 * (0.25 + 0.75 * d.len as f64 / avg.max(1.0)));
        }
        score.insert(d.path.clone(), s);
    }
    let max_bm = score.values().cloned().fold(0.0, f64::max).max(1e-9);
    for v in score.values_mut() {
        *v /= max_bm; // 0..1
    }
    // Stack-trace / failing-test paths are the strongest signal.
    let mentioned = mentioned_paths(failure, files);
    for p in mentioned.keys() {
        *score.entry(p.clone()).or_insert(0.0) += 2.0;
    }
    // Import-graph neighbours of the strongest hits (both directions).
    let set: BTreeSet<&str> = files.iter().map(String::as_str).collect();
    let mut edges: Vec<(String, String)> = vec![];
    for d in &docs {
        for c in import_re().captures_iter(&d.text) {
            if let Some(spec) = c.iter().skip(1).flatten().next() {
                if let Some(to) = resolve_import(&d.path, spec.as_str(), &set) {
                    edges.push((d.path.clone(), to));
                }
            }
        }
    }
    let mut seeds: Vec<(String, f64)> = score.iter().map(|(k, v)| (k.clone(), *v)).filter(|(_, v)| *v >= 0.5).collect();
    seeds.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    for (seed, _) in seeds.iter().take(TOP_K) {
        for (a, b) in &edges {
            let other = if a == seed { b } else if b == seed { a } else { continue };
            *score.entry(other.clone()).or_insert(0.0) += 0.4;
        }
    }
    let mut ranked: Vec<(String, f64)> = score.into_iter().filter(|(_, v)| *v > 0.0).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked.truncate(TOP_K);

    // Compile: repo map (skeleton) then focused snippets of the top files.
    let mut ctx = String::from("Repository map (definitions per file):\n");
    let mut map_used = 0;
    let mut unlisted = 0;
    for f in files {
        let line = match docs.iter().find(|d| &d.path == f) {
            Some(d) => {
                let defs: Vec<&str> = d.text.lines().filter(|l| def_re().is_match(l)).map(str::trim).take(8).collect();
                if defs.is_empty() { format!("{f}\n") } else { format!("{f}: {}\n", defs.join(" | ").chars().take(240).collect::<String>()) }
            }
            None => format!("{f}\n"),
        };
        if map_used + line.len() > MAP_MAX {
            unlisted += 1;
            continue;
        }
        map_used += line.len();
        ctx.push_str(&line);
    }
    if unlisted > 0 {
        ctx.push_str(&format!("(+{unlisted} more files)\n"));
    }
    ctx.push_str("\nRelevant files (ranked; test files are read-only):\n");
    let mut used = 0;
    for (f, _) in &ranked {
        let Some(d) = docs.iter().find(|d| &d.path == f) else { continue };
        let lines: Vec<&str> = d.text.lines().collect();
        let body = if lines.len() <= SMALL_FILE_LINES && d.text.len() <= FILE_MAX {
            format!("--- {f} ---\n{}{}", d.text, if d.text.ends_with('\n') { "" } else { "\n" })
        } else {
            // Windows around failure lines, definition lines and query hits.
            let mut keep = vec![false; lines.len()];
            let mut mark = |i: usize| {
                for k in i.saturating_sub(WINDOW)..(i + WINDOW + 1).min(lines.len()) {
                    keep[k] = true;
                }
            };
            for &ln in mentioned.get(f).into_iter().flatten() {
                if ln >= 1 && ln <= lines.len() { mark(ln - 1); }
            }
            for (i, l) in lines.iter().enumerate() {
                let lt = l.to_ascii_lowercase();
                if q.iter().any(|t| t.len() >= 4 && lt.contains(t.as_str())) && (def_re().is_match(l) || mentioned.contains_key(f) || q.iter().filter(|t| lt.contains(t.as_str())).count() >= 2) {
                    mark(i);
                }
            }
            if !keep.iter().any(|k| *k) {
                for i in 0..lines.len().min(WINDOW * 2) { keep[i] = true; }
            }
            let mut s = format!("--- {f} (excerpts; copy SEARCH lines exactly) ---\n");
            let mut prev = true;
            for (i, l) in lines.iter().enumerate() {
                if keep[i] {
                    if !prev { s.push_str("...\n"); }
                    s.push_str(l);
                    s.push('\n');
                }
                prev = keep[i];
            }
            if s.len() > FILE_MAX { s.truncate(s.char_indices().take_while(|(i, _)| *i < FILE_MAX).last().map(|(i, _)| i).unwrap_or(0)); s.push_str("\n...\n"); }
            s
        };
        if used + body.len() > TOTAL_MAX {
            continue;
        }
        used += body.len();
        ctx.push_str(&body);
    }
    Funnel { ranked, context: ctx }
}
