//! WP-B1 unit tests: anchor validation, the retrieval funnel, candidate selection.

use super::edits::{apply_anchor, from_value, is_test_path, parse_blocks, plan, Edit, Planned};
use super::funnel;
use super::select::{decode, dedupe, encode, pick, Outcome};
use serde_json::json;
use std::collections::HashMap;

fn repo(files: &[(&str, &str)]) -> HashMap<String, String> {
    files.iter().map(|(p, c)| (p.to_string(), c.to_string())).collect()
}

#[test]
fn bugfix_parses_multi_file_search_replace_blocks() {
    let text = "Fix both:\n\nsrc/math.js\n```\n<<<<<<< SEARCH\nexports.add = (a, b) => a - b;\n=======\nexports.add = (a, b) => a + b;\n>>>>>>> REPLACE\n```\n\
                src/util.js\n<<<<<<< SEARCH\n=======\nexports.id = (x) => x;\n>>>>>>> REPLACE\n*** Delete File: old.js\n";
    let e = parse_blocks(text).unwrap();
    assert_eq!(e.len(), 3);
    assert_eq!(e[0], Edit::Replace { path: "src/math.js".into(), search: "exports.add = (a, b) => a - b;\n".into(), replace: "exports.add = (a, b) => a + b;\n".into() });
    assert_eq!(e[1].path(), "src/util.js");
    assert_eq!(e[2], Edit::Delete { path: "old.js".into() });
    assert!(parse_blocks("no blocks here").is_err());
    assert!(parse_blocks("a.js\n<<<<<<< SEARCH\nx\n").unwrap_err().contains("divider"));
}

#[test]
fn bugfix_anchor_must_be_unique_exact_first_then_whitespace_tolerant() {
    let old = "fn a() {\n    let x = 1;\n}\nfn b() {\n    let x = 1;\n}\n";
    // Two exact matches: refused with a "make it unique" error.
    assert!(apply_anchor(old, "    let x = 1;\n", "    let x = 2;\n").unwrap_err().contains("2 places"));
    // Unique with context.
    let new = apply_anchor(old, "fn b() {\n    let x = 1;\n", "fn b() {\n    let x = 2;\n").unwrap();
    assert!(new.ends_with("fn b() {\n    let x = 2;\n}\n") && new.starts_with("fn a() {\n    let x = 1;"));
    // Whitespace-tolerant fallback (indent and trailing spaces differ).
    let new = apply_anchor(old, "fn a() {\n  let x = 1;   \n", "fn a() {\n    let x = 3;\n").unwrap();
    assert!(new.starts_with("fn a() {\n    let x = 3;\n}\nfn b()"), "{new}");
    // Not found.
    assert!(apply_anchor(old, "let y = 9;\n", "").unwrap_err().contains("not found"));
}

#[test]
fn bugfix_plan_validates_every_edit_before_anything_applies() {
    let r = repo(&[("math.js", "exports.add = (a, b) => a - b;\nexports.sub = (a, b) => a - b;\n"), ("test.js", "x\n")]);
    let read = |p: &str| r.get(p).cloned();
    // Multi-file: anchored edit + create.
    let ok = plan(&[
        Edit::Replace { path: "math.js".into(), search: "exports.add = (a, b) => a - b;\n".into(), replace: "exports.add = (a, b) => a + b;\n".into() },
        Edit::Replace { path: "lib/new.js".into(), search: String::new(), replace: "module.exports = 1;\n".into() },
    ], read).unwrap();
    assert_eq!(ok.paths(), vec!["lib/new.js".to_string(), "math.js".to_string()]);
    assert_eq!(ok.created, vec!["lib/new.js".to_string()]);
    assert!(ok.files["math.js"].as_ref().unwrap().contains("a + b;\nexports.sub = (a, b) => a - b;"));
    // One bad edit rejects the whole patch.
    let bad = plan(&[
        Edit::Replace { path: "math.js".into(), search: "exports.add = (a, b) => a - b;\n".into(), replace: "x\n".into() },
        Edit::Replace { path: "math.js".into(), search: "nope\n".into(), replace: "y\n".into() },
    ], read);
    assert!(bad.unwrap_err().starts_with("edit 2 (math.js)"));
    // Safety: traversal, .git, gating tests.
    for p in ["../etc/passwd", "/abs.js", ".git/config", ".allternit/x.json", "test.js", "tests/a.rs", "src/a.test.ts", "test_math.py"] {
        assert!(plan(&[Edit::Replace { path: p.into(), search: String::new(), replace: "x\n".into() }], read).is_err(), "{p}");
    }
    // Whole-file rewrite refused for a large existing file.
    let big = "x\n".repeat(super::edits::WHOLE_FILE_MAX_LINES);
    let rb = repo(&[("big.js", &big)]);
    let e = plan(&[Edit::Replace { path: "big.js".into(), search: String::new(), replace: "y\n".into() }], |p| rb.get(p).cloned()).unwrap_err();
    assert!(e.contains("whole-file rewrite refused"), "{e}");
    // Legacy {path, content} proposals still validate (scripted executor).
    let legacy = from_value(&json!({ "path": "math.js", "content": "exports.add = (a, b) => a + b;\n" })).unwrap();
    assert!(plan(&legacy, read).is_ok());
    // Journal round-trip (WP-P1 replay) incl. errors and the legacy shape.
    let set = vec![Ok(vec![Edit::Delete { path: "a.js".into() }, Edit::Replace { path: "b.js".into(), search: "x\n".into(), replace: "y\n".into() }]), Err("bad".to_string())];
    assert_eq!(super::edits::candidates_from_journal(&super::edits::candidates_to_journal(&set)), set);
    assert_eq!(super::edits::candidates_from_journal(&json!({ "path": "m.js", "content": "c\n" })).len(), 1);
    assert!(is_test_path("pkg/__tests__/x.js") && is_test_path("foo_test.go") && !is_test_path("src/attest.rs") && !is_test_path("contest/main.py"));
}

#[test]
fn bugfix_funnel_ranks_trace_paths_and_neighbours_and_stays_under_caps() {
    let mut files: Vec<(String, String)> = vec![
        ("src/math.js".into(), "exports.add = (a, b) => a - b;\nexports.mul = (a, b) => a * b;\n".into()),
        ("src/index.js".into(), "const m = require('./math');\nmodule.exports = m;\n".into()),
        ("test/math.test.js".into(), "const { add } = require('../src/math');\nif (add(2, 3) !== 5) throw new Error('add broken');\n".into()),
        ("README.md".into(), "docs\n".into()),
    ];
    for i in 0..40 {
        files.push((format!("src/noise{i}.js"), format!("exports.noise{i} = () => {i};\n").repeat(400)));
    }
    let map: HashMap<String, String> = files.iter().cloned().collect();
    let names: Vec<String> = files.iter().map(|f| f.0.clone()).collect();
    let failure = "Error: add broken\n    at Object.<anonymous> (test/math.test.js:2:22)\n";
    let f = funnel::build(&names, "Fix add so it adds", failure, |p| map.get(p).cloned());
    let top: Vec<&str> = f.ranked.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(top[0], "test/math.test.js", "{top:?}");
    assert!(top[..3].contains(&"src/math.js"), "import neighbour of the trace file: {top:?}");
    assert!(!top.iter().any(|p| p.starts_with("src/noise")), "{top:?}");
    assert!(f.context.contains("--- src/math.js ---\nexports.add = (a, b) => a - b;"), "{}", f.context);
    assert!(f.context.contains("Repository map"));
    assert!(f.context.len() <= funnel::TOTAL_MAX + 8 * 1024, "{}", f.context.len());
    let m = funnel::mentioned_paths(failure, &names);
    assert_eq!(m["test/math.test.js"].iter().copied().collect::<Vec<_>>(), vec![2]);
}

fn planned(path: &str, content: &str, diff_lines: usize) -> Planned {
    let mut p = Planned { diff_lines, ..Default::default() };
    p.files.insert(path.into(), Some(content.into()));
    p
}

#[test]
fn bugfix_selection_prefers_tests_then_lint_then_votes_then_smallest_diff() {
    let o = |t: bool, l: bool| Outcome { tests_pass: t, lint_ok: l, output: String::new(), repro_pass: 0, regressions: 0 };
    let a = planned("m.js", "a + b", 10);
    let b = planned("m.js", "a  +  b", 4); // same change, different whitespace
    let c = planned("m.js", "b + a", 2);
    // Passing beats a smaller failing diff.
    assert_eq!(pick(&[a.clone(), c.clone()], &[o(true, true), o(false, true)]), Some(0));
    // All pass: the normalized majority (a == b) beats the single smaller c.
    assert_eq!(pick(&[a.clone(), b.clone(), c.clone()], &[o(true, true), o(true, true), o(true, true)]), Some(1));
    // Equal votes: smallest diff.
    assert_eq!(pick(&[a.clone(), c.clone()], &[o(true, true), o(true, true)]), Some(1));
    // Nothing passes: lint-clean beats a syntax failure.
    assert_eq!(pick(&[c.clone(), a.clone()], &[o(false, false), o(false, true)]), Some(1));
    // Dedupe keeps the first of each normalized outcome with its vote count.
    assert_eq!(dedupe(&[a.clone(), b, c]), vec![(0, 2), (2, 1)]);
    // Evidence ref round-trips (re-driven runs recover outcomes).
    let ev = encode(2, &[0, 2], &[o(true, true), o(false, false)]);
    assert_eq!(ev, "candidates:2:0=PASS/r0/g0,2=LINT/r0/g0");
    let d = decode(&ev);
    assert_eq!(d.len(), 2);
    assert!(d[0].1.tests_pass && !d[1].1.lint_ok && d[1].0 == 2);
}
