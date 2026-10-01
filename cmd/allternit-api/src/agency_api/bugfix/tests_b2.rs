//! WP-B2 unit tests (memo C upgrades 4–8 + judge).

use super::edits::{is_test_path, path_is_safe, Planned};
use super::funnel::Funnel;
use super::select::{decode, encode, pick_with, tied, Outcome};
use super::{baseline, explore, judge, repro};

fn o(t: bool, repro_pass: usize, regressions: usize) -> Outcome {
    Outcome { tests_pass: t, lint_ok: true, output: String::new(), repro_pass, regressions }
}

fn planned(content: &str, diff_lines: usize) -> Planned {
    let mut p = Planned { diff_lines, ..Default::default() };
    p.files.insert("m.js".into(), Some(content.into()));
    p
}

#[test]
fn b2_selection_uses_repro_then_regressions_and_judge_only_on_ties() {
    let (a, b, c) = (planned("a + b", 2), planned("b + a", 4), planned("a === 10 ? 0 : a + b", 2));
    let all = [a.clone(), b.clone(), c.clone()];
    // Tests decide first: a failing candidate never wins on repro evidence.
    assert_eq!(pick_with(&[a.clone(), b.clone()], &[o(false, 3, 0), o(true, 0, 0)], &[1, 1], Some(0)), Some(1));
    // Repro evidence breaks a test tie; c passes the suite but not the repro.
    assert_eq!(pick_with(&all, &[o(true, 1, 0), o(true, 1, 0), o(true, 0, 0)], &[1, 1, 1], None), Some(0));
    // Fewer new failures beats more.
    assert_eq!(pick_with(&[a.clone(), b.clone()], &[o(false, 0, 2), o(false, 0, 1)], &[1, 1], None), Some(1));
    // Judge picks among tied (b, larger diff) over the smallest-diff default.
    let outs = [o(true, 1, 0), o(true, 1, 0), o(true, 0, 0)];
    assert_eq!(tied(&outs, &[1, 1, 1]), vec![0, 1]);
    assert_eq!(pick_with(&all, &outs, &[1, 1, 1], Some(1)), Some(1));
    // The judge cannot promote a candidate outside the tie.
    assert_eq!(pick_with(&all, &outs, &[1, 1, 1], Some(2)), Some(0));
    // Votes outrank the judge; no tie when nothing passes.
    assert_eq!(tied(&outs, &[2, 1, 1]), vec![0]);
    assert!(tied(&[o(false, 0, 0), o(false, 0, 0)], &[1, 1]).is_empty());
    // Evidence round-trips the new fields; B1-format refs still decode.
    let d = decode(&encode(1, &[0, 3], &[o(true, 2, 0), o(false, 0, 5)]));
    assert_eq!((d[0].1.repro_pass, d[1].1.regressions, d[1].0), (2, 5, 3));
    assert!(decode("candidates:1:0=PASS,1=LINT")[0].1.tests_pass);
}

#[test]
fn b2_judge_render_prompt_and_choice() {
    let p = planned("x = 2\n", 2);
    let r = judge::render(&p, |_| Some("x = 1\n".into()));
    assert!(r.contains("-x = 1") && r.contains("+x = 2"), "{r}");
    let pr = judge::prompt("Fix x", "boom", &[r.clone(), r]);
    assert!(pr.starts_with("Goal: Fix x") && pr.contains("Candidate 2:"));
    assert_eq!(judge::parse_choice("Candidate 2, because", 2), Some(1));
    assert_eq!(judge::parse_choice("7 then 1", 2), Some(0));
    assert_eq!(judge::parse_choice("none", 2), None);
}

#[test]
fn b2_repro_scripts_are_fenced_and_kept_only_when_they_reproduce() {
    assert_eq!(repro::runner(&["npm", "test", "--silent"]).unwrap().0, "js");
    assert_eq!(repro::runner(&["python3", "-m", "pytest", "-q"]).unwrap().0, "py");
    assert!(repro::runner(&["cargo", "test"]).is_none());
    let p = repro::path_for(0, "js");
    // The patch step can never touch generated scripts, and they never
    // collide with the gating tests' paths.
    assert!(!path_is_safe(&p) && !is_test_path("math.js"));
    assert_eq!(repro::extract("here:\n```js\nthrow 1\n```\nbye").as_deref(), Some("throw 1\n"));
    assert_eq!(repro::extract("  \n"), None);
    assert!(repro::reproduces(false, "Error: add(2,3) returned -1", "Fix add", "FAIL"));
    assert!(!repro::reproduces(true, "add ok", "Fix add", ""), "passing on the bug = useless");
    assert!(!repro::reproduces(false, "SyntaxError: Unexpected token add", "Fix add", ""), "broken script");
    assert!(!repro::reproduces(false, "unrelated widget crash", "Fix add", "FAIL math"), "wrong reason");
    std::env::set_var("ALLTERNIT_AGENCY_REPROS", "99");
    assert_eq!(repro::count(), 10);
    std::env::remove_var("ALLTERNIT_AGENCY_REPROS");
}

#[test]
fn b2_baseline_gates_only_new_failures_and_records_flakes() {
    let before = "ok 1 - adds\nnot ok 2 - legacy broken\nok 3 - io\n";
    let r = baseline::parse("test a::b ... ok\ntest a::c ... FAILED\nFAILED t.py::x - boom\nPASSED t.py::y\n  ✓ jest ok (2 ms)\n  ✕ jest bad\n");
    assert_eq!((r["a::b"], r["a::c"], r["t.py::x"], r["t.py::y"], r["jest ok"], r["jest bad"]), (true, false, false, true, true, false));
    // Pre-existing failure only: passes the baseline gate.
    assert_eq!(baseline::judge_run(before, "ok 1 - adds\nnot ok 2 - legacy broken\nok 3 - io\n", &[]), (true, vec![], vec![]));
    // New failure, no reruns yet: a real regression.
    let after = "ok 1 - adds\nnot ok 2 - legacy broken\nnot ok 3 - io\n";
    assert_eq!(baseline::judge_run(before, after, &[]).2, vec!["io".to_string()]);
    // Passes on a rerun: a flake, recorded, not a regression.
    assert_eq!(baseline::judge_run(before, after, &[after.into(), before.into()]), (true, vec!["io".to_string()], vec![]));
    // Nothing parsed: fail closed.
    assert_eq!(baseline::judge_run(before, "FAIL", &[]), (false, vec![], vec![]));
}

#[test]
fn b2_exploration_is_bounded_read_only_and_only_on_low_confidence() {
    let files = vec!["src/a.js".to_string(), ".allternit/x.json".to_string()];
    let read = |f: &str| Some(match f { "src/a.js" => "function add(a, b) {\n  return a - b;\n}\n", _ => "secret add" }.to_string());
    let flat = Funnel { ranked: vec![("src/a.js".into(), 0.9), ("b.js".into(), 0.85)], context: String::new() };
    let sharp = Funnel { ranked: vec![("src/a.js".into(), 1.0), ("b.js".into(), 0.2)], context: String::new() };
    assert!(explore::low_confidence(&flat, "boom", &files) && !explore::low_confidence(&sharp, "boom", &files));
    assert!(!explore::low_confidence(&flat, "at src/a.js:2:3", &files), "stack-trace paths = confident");
    assert_eq!(explore::parse_call("view src/a.js 2 3\nmore"), Some(explore::Call::View("src/a.js".into(), 2, 3)));
    assert_eq!(explore::parse_call("`done`"), Some(explore::Call::Done));
    assert_eq!(explore::parse_call("rm -rf /"), None);
    let g = explore::exec(&explore::Call::Grep("add".into()), &files, read);
    assert!(g.contains("src/a.js:1:") && !g.contains(".allternit"), "{g}");
    assert!(explore::exec(&explore::Call::View("src/a.js".into(), 2, 2), &files, read).contains("2:   return a - b;"));
    assert!(explore::exec(&explore::Call::View("/etc/passwd".into(), 1, 2), &files, read).contains("not a tracked file"));
    assert!(explore::exec(&explore::Call::Symbol("add".into()), &files, read).contains("function add"));
    assert_eq!(explore::MAX_CALLS, 10);
}
