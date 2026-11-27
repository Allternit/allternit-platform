//! The validity check for a harness patch.
//!
//! A candidate is valid only when all of these hold:
//! - (a) `filter_patch_by_paths` with the protected list leaves the patch
//!   unchanged, so nothing outside the view is in it;
//! - (b) every file it touches is on the allowlist, and it only edits files
//!   that already exist (no new, deleted, renamed or binary files);
//! - (c) the two `.ts` files change only in number literals, only on the
//!   tunable lines, and only within bounds;
//! - the `.txt` files add no vendor names and stay under their byte caps
//!   (a fast local copy of what the guard tests check);
//! - (d) the guard tests pass in the candidate checkout.
//!
//! The static checks (a–c) run first and need only the patch and the
//! baseline repo, so a bad patch fails before any child process starts.

use crate::view::{self, GUARDRAIL_TS, LIMIT_TS, PROMPT_DIR};
use allternit_self_improve::eval::{self, EvalError, ValidityCheck};
use allternit_self_improve::working_copy::filter_patch_by_paths;
use std::collections::HashSet;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// Where the guard tests run, relative to the candidate checkout.
pub const GUARD_TEST_DIR: &str = view::GIZZI_DIR;

/// The guard tests: byte caps, vendor ban, injection wording, context
/// breakdown and turn guardrails. Change the list here.
pub const GUARD_TEST_CMD: &[&str] = &[
    "bun",
    "test",
    "test/session/system-prompt-selection.test.ts",
    "test/session/system-prompt-guard.test.ts",
    "test/session/context-breakdown.test.ts",
    "test/session/guardrails.test.ts",
    "test/session/guardrails-loop.test.ts",
];

/// Names that must never appear on an added prompt or tool-text line.
/// Matched case-insensitively as substrings.
pub const VENDOR_NAMES: &[&str] = &["claude", "anthropic", "openai", "gpt", "gemini", "codex", "kimi", "grok", "opencode"];

/// Byte caps for system prompt parts, by file name. A fast copy of the table
/// in `test/session/system-prompt-selection.test.ts`; that test stays the
/// source of truth and runs in step (d).
pub const PROMPT_BYTE_CAPS: &[(&str, usize)] = &[
    ("base-chat.txt", 5_600),
    ("capabilities.txt", 7_400),
    ("bot.txt", 3_200),
    ("duet.txt", 1_800),
    ("delegation.txt", 1_000),
    ("project-thread.txt", 1_300),
    ("computer-use.txt", 3_400),
    ("care.txt", 1_500),
    ("safety.txt", 2_900),
    ("base-code.txt", 11_200),
    ("addendum-gpt.txt", 700),
    ("addendum-gemini.txt", 700),
    ("addendum-open.txt", 700),
    ("voice.txt", 2_000),
    ("phone.txt", 700),
    ("unattended.txt", 700),
    ("memory-use.txt", 1_300),
    ("mode-auto.txt", 700),
    ("mode-ask.txt", 400),
    ("background-code.txt", 700),
    ("addendum-ondevice.txt", 400),
    ("voice-tags.txt", 300),
];

/// `limit.ts` DEFAULT_WARN_AT and DEFAULT_LAND_AT.
pub const WARN_LAND_RANGE: RangeInclusive<f64> = 0.5..=0.99;
/// `limit.ts` DEFAULT_WRAP_UP_STEPS (whole numbers).
pub const WRAP_UP_STEPS_RANGE: RangeInclusive<f64> = 1.0..=10.0;
/// `guardrail-defaults.ts` TURN_LIMIT_DEFAULTS: new value / current value.
pub const TURN_LIMIT_RATIO: RangeInclusive<f64> = 0.5..=2.0;

const LIMIT_TUNABLES: &[&str] = &["DEFAULT_WARN_AT", "DEFAULT_LAND_AT", "DEFAULT_WRAP_UP_STEPS"];
const TURN_LIMIT_ROWS: &[&str] = &["primary", "subagent", "internal"];
const TURN_LIMIT_FIELDS: &[&str] = &["steps", "toolCalls", "turnWallClockMs"];

/// A patch that passed the static checks, with each touched file's new text.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedPatch {
    /// `(repo-relative path, content after the patch)`, in patch order.
    pub files: Vec<(String, String)>,
}

/// Static checks (a), (b), (c) and the local vendor / byte-cap checks.
/// `baseline` is the repo (or view) the patch was made against. Returns every
/// reason the patch is rejected, not just the first.
pub fn check_patch(patch: &str, baseline: &Path) -> Result<CheckedPatch, Vec<String>> {
    // One trailing newline, so the protected-path filter (which rejoins
    // lines with `\n`) gives back the same text for a clean patch.
    let mut patch = patch.trim_end_matches('\n').to_string();
    patch.push('\n');
    if patch.trim().is_empty() {
        return Err(vec!["the patch is empty".into()]);
    }

    // (a) Nothing outside the view: the protected-path filter must be a no-op.
    match filter_patch_by_paths(&patch, &view::protected_paths()) {
        Ok(filtered) if filtered == patch => {}
        Ok(_) => return Err(vec!["the patch touches a protected path (suite, graders or tests)".into()]),
        Err(err) => return Err(vec![format!("the patch names a protected path: {err}")]),
    }

    let files = parse_patch(&patch)?;
    let mut reasons = Vec::new();
    let mut seen = HashSet::new();
    let mut checked = Vec::new();

    for file in &files {
        let rel = file.path.as_str();
        if !seen.insert(rel.to_string()) {
            reasons.push(format!("{rel} appears twice in the patch"));
            continue;
        }
        // (b) The allowlist.
        if !view::is_allowed(rel) {
            reasons.push(format!("{rel} is not on the allowlist of editable files"));
            continue;
        }
        let original = match read_regular_file(&baseline.join(rel)) {
            Ok(text) => text,
            Err(err) => {
                reasons.push(format!("{rel}: {err}"));
                continue;
            }
        };
        let updated = match apply_hunks(&original, &file.hunks) {
            Ok(text) => text,
            Err(err) => {
                reasons.push(format!("{rel}: {err}"));
                continue;
            }
        };

        if rel.ends_with(".ts") {
            // (c) Numbers only, tunable lines only, within bounds.
            let hunk_reasons = check_numeric_hunks(rel, &file.hunks);
            if hunk_reasons.is_empty() {
                reasons.extend(check_ts_bounds(rel, &original, &updated));
            } else {
                reasons.extend(hunk_reasons);
            }
        } else {
            reasons.extend(check_text_file(rel, &file.hunks, &updated));
        }
        checked.push((rel.to_string(), updated));
    }

    if reasons.is_empty() {
        Ok(CheckedPatch { files: checked })
    } else {
        Err(reasons)
    }
}

fn read_regular_file(path: &Path) -> Result<String, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| "the file does not exist in the baseline".to_string())?;
    if !meta.is_file() {
        return Err("not a regular file in the baseline".into());
    }
    std::fs::read_to_string(path).map_err(|err| format!("could not read the baseline file: {err}"))
}

// ─── Patch parsing ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum HunkLine {
    Ctx(String),
    Del(String),
    Add(String),
}

#[derive(Debug, Clone)]
struct Hunk {
    old_start: usize,
    old_len: usize,
    lines: Vec<HunkLine>,
    /// `\ No newline at end of file` after the old / new side's last line.
    old_no_eol: bool,
    new_no_eol: bool,
}

#[derive(Debug, Clone)]
struct FilePatch {
    path: String,
    hunks: Vec<Hunk>,
}

/// Parses a git-style unified diff. Only in-place edits of existing text
/// files are accepted: any extended header other than `index` (new, deleted,
/// renamed, copied, mode change, binary) is a rejection.
fn parse_patch(patch: &str) -> Result<Vec<FilePatch>, Vec<String>> {
    let lines: Vec<&str> = patch.lines().collect();
    let mut errors = Vec::new();
    let mut files = Vec::new();
    if !lines.first().is_some_and(|line| line.starts_with("diff --git ")) {
        return Err(vec!["the patch must start with a `diff --git` line".into()]);
    }

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let Some(path) = parse_git_header(line) else {
            return Err(vec![format!("line {}: expected a `diff --git a/<path> b/<path>` header with one safe path", i + 1)]);
        };
        i += 1;

        let (mut minus, mut plus) = (None, None);
        while i < lines.len() && !lines[i].starts_with("@@") && !lines[i].starts_with("diff --git ") {
            let header = lines[i];
            if header.starts_with("index ") {
                // fine
            } else if let Some(p) = header.strip_prefix("--- ") {
                minus = Some(p);
            } else if let Some(p) = header.strip_prefix("+++ ") {
                plus = Some(p);
            } else {
                errors.push(format!("{path}: only edits to existing text files are allowed (found `{header}`)"));
            }
            i += 1;
        }
        let a = format!("a/{path}");
        let b = format!("b/{path}");
        if minus == Some("/dev/null") || plus == Some("/dev/null") {
            errors.push(format!("{path}: new or deleted files are not allowed"));
        } else if minus != Some(a.as_str()) || plus != Some(b.as_str()) {
            errors.push(format!("{path}: the ---/+++ lines must name the same file as the diff header"));
        }

        let mut hunks = Vec::new();
        while i < lines.len() && lines[i].starts_with("@@") {
            let Some((old_start, old_len, new_len)) = parse_hunk_header(lines[i]) else {
                errors.push(format!("{path}: bad hunk header `{}`", lines[i]));
                return Err(errors);
            };
            i += 1;
            let mut hunk = Hunk { old_start, old_len, lines: Vec::new(), old_no_eol: false, new_no_eol: false };
            let (mut old_seen, mut new_seen) = (0usize, 0usize);
            let mut last: Option<char> = None;
            loop {
                if i < lines.len() && lines[i].starts_with('\\') {
                    match last {
                        Some('+') => hunk.new_no_eol = true,
                        Some('-') => hunk.old_no_eol = true,
                        Some(' ') => {
                            hunk.old_no_eol = true;
                            hunk.new_no_eol = true;
                        }
                        _ => {}
                    }
                    i += 1;
                    continue;
                }
                if (old_seen >= old_len && new_seen >= new_len) || i >= lines.len() {
                    break;
                }
                let body = lines[i];
                let (kind, text) = match body.chars().next() {
                    Some(c @ (' ' | '-' | '+')) => (c, &body[1..]),
                    // Some tools trim the space off an empty context line.
                    None => (' ', ""),
                    Some(_) => {
                        errors.push(format!("{path}: bad hunk line `{body}`"));
                        return Err(errors);
                    }
                };
                match kind {
                    ' ' => {
                        hunk.lines.push(HunkLine::Ctx(text.to_string()));
                        old_seen += 1;
                        new_seen += 1;
                    }
                    '-' => {
                        hunk.lines.push(HunkLine::Del(text.to_string()));
                        old_seen += 1;
                    }
                    _ => {
                        hunk.lines.push(HunkLine::Add(text.to_string()));
                        new_seen += 1;
                    }
                }
                last = Some(kind);
                i += 1;
            }
            if old_seen != old_len || new_seen != new_len {
                errors.push(format!("{path}: a hunk's line counts do not match its header"));
                return Err(errors);
            }
            hunks.push(hunk);
        }
        if hunks.is_empty() {
            errors.push(format!("{path}: no changes"));
        }
        if i < lines.len() && !lines[i].starts_with("diff --git ") {
            errors.push(format!("line {}: unexpected text after a hunk", i + 1));
            return Err(errors);
        }
        files.push(FilePatch { path, hunks });
    }

    if errors.is_empty() {
        Ok(files)
    } else {
        Err(errors)
    }
}

/// `diff --git a/X b/X` → `X`, when both sides name the same safe relative path.
fn parse_git_header(line: &str) -> Option<String> {
    let rest = line.strip_prefix("diff --git ")?;
    let mut parts = rest.split(' ');
    let a = parts.next()?.strip_prefix("a/")?;
    let b = parts.next()?.strip_prefix("b/")?;
    if parts.next().is_some() || a != b || !is_safe_relative(a) {
        return None;
    }
    Some(a.to_string())
}

fn is_safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

/// `@@ -s[,l] +s[,l] @@ ...` → `(old_start, old_len, new_len)`.
fn parse_hunk_header(line: &str) -> Option<(usize, usize, usize)> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, _) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |s: &str| -> Option<(usize, usize)> {
        match s.split_once(',') {
            Some((start, len)) => Some((start.parse().ok()?, len.parse().ok()?)),
            None => Some((s.parse().ok()?, 1)),
        }
    };
    let (old_start, old_len) = range(old)?;
    let (_, new_len) = range(new)?;
    Some((old_start, old_len, new_len))
}

/// Applies the hunks to `original`, checking every context and removed line.
fn apply_hunks(original: &str, hunks: &[Hunk]) -> Result<String, String> {
    let old: Vec<&str> = original.lines().collect();
    let old_eol = original.is_empty() || original.ends_with('\n');
    let mut new_eol = old_eol;
    let mut out: Vec<String> = Vec::new();
    let mut pos = 0usize;

    for hunk in hunks {
        let start = if hunk.old_len == 0 { hunk.old_start } else { hunk.old_start.saturating_sub(1) };
        if start < pos || start > old.len() {
            return Err("the patch does not fit the baseline (hunks overlap or run past the end)".into());
        }
        out.extend(old[pos..start].iter().map(|s| s.to_string()));
        let mut at = start;
        for line in &hunk.lines {
            match line {
                HunkLine::Ctx(text) | HunkLine::Del(text) => {
                    if old.get(at) != Some(&text.as_str()) {
                        return Err(format!("the patch does not fit the baseline at line {}", at + 1));
                    }
                    if matches!(line, HunkLine::Ctx(_)) {
                        out.push(text.clone());
                    }
                    at += 1;
                }
                HunkLine::Add(text) => out.push(text.clone()),
            }
        }
        pos = at;
        if pos == old.len() {
            new_eol = if hunk.new_no_eol {
                false
            } else if hunk.old_no_eol {
                true
            } else {
                old_eol
            };
        }
    }
    out.extend(old[pos..].iter().map(|s| s.to_string()));

    let mut text = out.join("\n");
    if new_eol && !out.is_empty() {
        text.push('\n');
    }
    Ok(text)
}

// ─── (c) Numbers only ────────────────────────────────────────────────────────

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// Byte ranges of number literals (`3`, `0.95`, `60_000`) in a line. A digit
/// right after an identifier character (`V2`, `x1`) is not a number.
fn number_spans(line: &str) -> Vec<(usize, usize)> {
    let b = line.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() && (i == 0 || !is_ident_byte(b[i - 1])) {
            let start = i;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                i += 1;
            }
            if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
                i += 1;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                    i += 1;
                }
            }
            spans.push((start, i));
        } else {
            i += 1;
        }
    }
    spans
}

/// The line with every number literal replaced by `#`.
fn skeleton(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut last = 0;
    for (start, end) in number_spans(line) {
        out.push_str(&line[last..start]);
        out.push('#');
        last = end;
    }
    out.push_str(&line[last..]);
    out
}

fn parse_number(literal: &str) -> Option<f64> {
    literal.replace('_', "").parse().ok()
}

/// Every removed line is paired, in order, with an added line, and each pair
/// differs only in number literals. No line is added or removed outright.
fn check_numeric_hunks(rel: &str, hunks: &[Hunk]) -> Vec<String> {
    let mut reasons = Vec::new();
    for hunk in hunks {
        let mut i = 0;
        let lines = &hunk.lines;
        while i < lines.len() {
            match &lines[i] {
                HunkLine::Ctx(_) => i += 1,
                HunkLine::Add(text) => {
                    reasons.push(format!("{rel}: adds a line (only numbers may change): `{}`", text.trim()));
                    i += 1;
                }
                HunkLine::Del(_) => {
                    let mut dels = Vec::new();
                    while let Some(HunkLine::Del(text)) = lines.get(i) {
                        dels.push(text);
                        i += 1;
                    }
                    let mut adds = Vec::new();
                    while let Some(HunkLine::Add(text)) = lines.get(i) {
                        adds.push(text);
                        i += 1;
                    }
                    if dels.len() != adds.len() {
                        reasons.push(format!("{rel}: changes the number of lines (only numbers may change)"));
                        continue;
                    }
                    for (old, new) in dels.iter().zip(&adds) {
                        if skeleton(old) != skeleton(new) {
                            reasons.push(format!("{rel}: changes more than numbers: `{}` -> `{}`", old.trim(), new.trim()));
                        }
                    }
                }
            }
        }
    }
    reasons
}

/// The tunable lines may change, within bounds; any other number may not.
/// Called only after `check_numeric_hunks` passed, so both texts have the
/// same lines with the same skeletons.
fn check_ts_bounds(rel: &str, original: &str, updated: &str) -> Vec<String> {
    let old: Vec<&str> = original.lines().collect();
    let new: Vec<&str> = updated.lines().collect();
    if old.len() != new.len() {
        return vec![format!("{rel}: changes the number of lines")];
    }
    let changed: Vec<usize> = (0..old.len()).filter(|&i| old[i] != new[i]).collect();
    if rel == LIMIT_TS {
        check_limit_bounds(rel, &old, &new, &changed)
    } else if rel == GUARDRAIL_TS {
        check_guardrail_bounds(rel, &old, &new, &changed)
    } else {
        vec![format!("{rel}: no numbers in this file may change")]
    }
}

fn first_number(line: &str) -> Option<f64> {
    let (_, after) = line.split_once('=')?;
    let (start, end) = *number_spans(after).first()?;
    parse_number(&after[start..end])
}

fn check_limit_bounds(rel: &str, old: &[&str], new: &[&str], changed: &[usize]) -> Vec<String> {
    let mut reasons = Vec::new();
    let tunable_of = |line: &str| {
        let trimmed = line.trim_start();
        LIMIT_TUNABLES.iter().copied().find(|name| {
            trimmed
                .strip_prefix("export const ")
                .and_then(|rest| rest.strip_prefix(*name))
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        })
    };
    let mut values: std::collections::HashMap<&str, f64> = std::collections::HashMap::new();
    for line in new {
        if let Some(name) = tunable_of(line) {
            if let Some(value) = first_number(line) {
                values.insert(name, value);
            }
        }
    }
    let mut warn_or_land_changed = false;
    for &i in changed {
        let Some(name) = tunable_of(new[i]) else {
            reasons.push(format!("{rel} line {}: only {} may change: `{}`", i + 1, LIMIT_TUNABLES.join(", "), old[i].trim()));
            continue;
        };
        let Some(value) = first_number(new[i]) else {
            reasons.push(format!("{rel} line {}: could not read {name}", i + 1));
            continue;
        };
        match name {
            "DEFAULT_WRAP_UP_STEPS" => {
                if value.fract() != 0.0 || !WRAP_UP_STEPS_RANGE.contains(&value) {
                    reasons.push(format!(
                        "{rel}: {name} = {value} is out of bounds (a whole number in {}..={})",
                        WRAP_UP_STEPS_RANGE.start(),
                        WRAP_UP_STEPS_RANGE.end()
                    ));
                }
            }
            _ => {
                warn_or_land_changed = true;
                if !WARN_LAND_RANGE.contains(&value) {
                    reasons.push(format!(
                        "{rel}: {name} = {value} is out of bounds ({}..={})",
                        WARN_LAND_RANGE.start(),
                        WARN_LAND_RANGE.end()
                    ));
                }
            }
        }
    }
    if warn_or_land_changed {
        if let (Some(warn), Some(land)) = (values.get("DEFAULT_WARN_AT"), values.get("DEFAULT_LAND_AT")) {
            if warn > land {
                reasons.push(format!("{rel}: DEFAULT_WARN_AT ({warn}) must not be above DEFAULT_LAND_AT ({land})"));
            }
        }
    }
    reasons
}

/// A field's value in a row like `primary: { steps: 500, toolCalls: 1_000,
/// turnWallClockMs: 2 * 60 * 60_000 },`: a product of number literals.
fn row_field(line: &str, field: &str) -> Option<f64> {
    let key = format!("{field}:");
    let mut search = 0;
    let at = loop {
        let found = search + line[search..].find(&key)?;
        if found == 0 || !is_ident_byte(line.as_bytes()[found - 1]) {
            break found;
        }
        search = found + key.len();
    };
    let rest = &line[at + key.len()..];
    let end = rest.find(|c: char| c == ',' || c == '}').unwrap_or(rest.len());
    let mut product = 1.0;
    for factor in rest[..end].split('*') {
        let factor = factor.trim();
        let spans = number_spans(factor);
        if spans.len() != 1 || spans[0] != (0, factor.len()) {
            return None;
        }
        product *= parse_number(factor)?;
    }
    Some(product)
}

fn check_guardrail_bounds(rel: &str, old: &[&str], new: &[&str], changed: &[usize]) -> Vec<String> {
    let mut reasons = Vec::new();
    let block_start = new.iter().position(|line| line.contains("export const TURN_LIMIT_DEFAULTS"));
    let block_end = block_start.and_then(|start| new[start..].iter().position(|line| line.trim() == "}").map(|end| start + end));
    for &i in changed {
        let row = TURN_LIMIT_ROWS.iter().copied().find(|row| new[i].trim_start().starts_with(&format!("{row}:")));
        let in_block = matches!((block_start, block_end), (Some(start), Some(end)) if i > start && i < end);
        let Some(row) = row.filter(|_| in_block) else {
            reasons.push(format!("{rel} line {}: only the TURN_LIMIT_DEFAULTS rows may change: `{}`", i + 1, old[i].trim()));
            continue;
        };
        for field in TURN_LIMIT_FIELDS {
            let (Some(before), Some(after)) = (row_field(old[i], field), row_field(new[i], field)) else {
                reasons.push(format!("{rel}: could not read {row}.{field}"));
                continue;
            };
            if before == after {
                continue;
            }
            if *field != "turnWallClockMs" && after.fract() != 0.0 {
                reasons.push(format!("{rel}: {row}.{field} = {after} must be a whole number"));
                continue;
            }
            let ratio = if before > 0.0 { after / before } else { f64::INFINITY };
            if !TURN_LIMIT_RATIO.contains(&ratio) {
                reasons.push(format!(
                    "{rel}: {row}.{field} = {after} is out of bounds ({}x..={}x of {before})",
                    TURN_LIMIT_RATIO.start(),
                    TURN_LIMIT_RATIO.end()
                ));
            }
        }
    }
    reasons
}

// ─── .txt files ──────────────────────────────────────────────────────────────

fn check_text_file(rel: &str, hunks: &[Hunk], updated: &str) -> Vec<String> {
    let mut reasons = Vec::new();
    for hunk in hunks {
        for line in &hunk.lines {
            if let HunkLine::Add(text) = line {
                let lower = text.to_lowercase();
                if let Some(name) = VENDOR_NAMES.iter().find(|name| lower.contains(**name)) {
                    reasons.push(format!("{rel}: an added line names a vendor (\"{name}\"): `{}`", text.trim()));
                }
            }
        }
    }
    if updated.trim().is_empty() {
        reasons.push(format!("{rel}: the patch would leave the file empty"));
    }
    if let Some(name) = rel.strip_prefix(PROMPT_DIR).and_then(|rest| rest.strip_prefix('/')) {
        if let Some((_, cap)) = PROMPT_BYTE_CAPS.iter().find(|(file, _)| *file == name) {
            if updated.len() > *cap {
                reasons.push(format!("{rel}: {} bytes is over its cap of {cap} bytes", updated.len()));
            }
        }
    }
    reasons
}

// ─── (d) Guard tests and the ValidityCheck ───────────────────────────────────

/// The result of a guard-test run.
#[derive(Debug, Clone, PartialEq)]
pub struct GuardOutcome {
    pub passed: bool,
    /// The tail of the run's output, for the rejection reason.
    pub output: String,
}

/// Runs the guard tests in a candidate checkout.
pub trait GuardRunner: Send + Sync {
    fn run(&self, checkout: &Path) -> anyhow::Result<GuardOutcome>;
}

/// Runs [`GUARD_TEST_CMD`] as a child process in `<checkout>/cmd/gizzi-code`.
#[derive(Debug, Default)]
pub struct BunGuardTests;

impl GuardRunner for BunGuardTests {
    fn run(&self, checkout: &Path) -> anyhow::Result<GuardOutcome> {
        let dir = checkout.join(GUARD_TEST_DIR);
        let output = Command::new(GUARD_TEST_CMD[0])
            .args(&GUARD_TEST_CMD[1..])
            .current_dir(&dir)
            .output()
            .map_err(|err| anyhow::anyhow!("could not start `{}` in {}: {err}", GUARD_TEST_CMD.join(" "), dir.display()))?;
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        let lines: Vec<&str> = text.lines().collect();
        let tail = lines[lines.len().saturating_sub(40)..].join("\n");
        Ok(GuardOutcome { passed: output.status.success(), output: tail })
    }
}

/// The [`ValidityCheck`] for one harness patch. `check(candidate)` runs the
/// static checks against `baseline`, confirms the candidate checkout holds
/// exactly the patched files, then runs the guard tests there.
pub struct HarnessValidity {
    baseline: PathBuf,
    patch: String,
    guard: Box<dyn GuardRunner>,
    rejections: Mutex<Vec<String>>,
}

impl std::fmt::Debug for HarnessValidity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HarnessValidity").field("baseline", &self.baseline).finish_non_exhaustive()
    }
}

impl HarnessValidity {
    pub fn new(baseline: impl Into<PathBuf>, patch: impl Into<String>, guard: Box<dyn GuardRunner>) -> Self {
        Self { baseline: baseline.into(), patch: patch.into(), guard, rejections: Mutex::new(Vec::new()) }
    }

    /// Why the last `check` said no (empty after a pass).
    pub fn rejections(&self) -> Vec<String> {
        self.rejections.lock().map(|r| r.clone()).unwrap_or_default()
    }

    fn reject(&self, reasons: Vec<String>) -> eval::Result<bool> {
        if let Ok(mut slot) = self.rejections.lock() {
            *slot = reasons;
        }
        Ok(false)
    }
}

impl ValidityCheck for HarnessValidity {
    fn check(&self, candidate: &Path) -> eval::Result<bool> {
        let checked = match check_patch(&self.patch, &self.baseline) {
            Ok(checked) => checked,
            Err(reasons) => return self.reject(reasons),
        };
        let mut mismatched = Vec::new();
        for (rel, expected) in &checked.files {
            let actual = std::fs::read_to_string(candidate.join(rel)).unwrap_or_default();
            if !actual.lines().eq(expected.lines()) {
                mismatched.push(format!("{rel}: the candidate checkout does not match the patch"));
            }
        }
        if !mismatched.is_empty() {
            return self.reject(mismatched);
        }
        let outcome = self.guard.run(candidate).map_err(|err| EvalError::Evaluator(err.to_string()))?;
        if !outcome.passed {
            return self.reject(vec![format!("the guard tests failed:\n{}", outcome.output)]);
        }
        self.reject(Vec::new()).map(|_| true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    const LIMIT: &str = "export namespace SessionLimit {\n  export const DEFAULT_WARN_AT = 0.8\n  export const DEFAULT_LAND_AT = 0.95 // same default\n  export const DEFAULT_WRAP_UP_STEPS = 3\n  const pct = Math.round(ratio * 100)\n}\n";
    const GUARDRAIL: &str = "export const GUARDRAIL_DEFAULTS_VERSION = \"2026-10-01.1\"\n\nexport const TURN_LIMIT_DEFAULTS: Record<AgentKind, TurnLimits> = {\n  primary: { steps: 500, toolCalls: 1_000, turnWallClockMs: 2 * 60 * 60_000 },\n  subagent: { steps: 200, toolCalls: 400, turnWallClockMs: 60 * 60_000 },\n  internal: { steps: 20, toolCalls: 40, turnWallClockMs: 10 * 60_000 },\n}\n\nexport const TURN_LIMIT_CEILING: TurnLimits = {\n  steps: 2_000,\n  toolCalls: 5_000,\n}\n";
    const BASE_CODE: &str = "You are a careful coding assistant.\nKeep answers short.\n";
    const MODE_ASK: &str = "Ask before you act.\n";
    const READ_TOOL: &str = "Reads a file from disk.\n";

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn base_code() -> String {
        format!("{PROMPT_DIR}/base-code.txt")
    }
    fn mode_ask() -> String {
        format!("{PROMPT_DIR}/mode-ask.txt")
    }
    fn read_tool() -> String {
        format!("{}/read.txt", view::TOOL_TEXT_DIR)
    }
    const OTHER_CODE: &str = "cmd/gizzi-code/src/runtime/session/prompt.ts";

    fn fake_repo() -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        let r = repo.path();
        write(r, LIMIT_TS, LIMIT);
        write(r, GUARDRAIL_TS, GUARDRAIL);
        write(r, &base_code(), BASE_CODE);
        write(r, &mode_ask(), MODE_ASK);
        write(r, &read_tool(), READ_TOOL);
        write(r, OTHER_CODE, "export const x = 1\n");
        write(r, "cmd/gizzi-code/test/session/guardrails.test.ts", "expect(1)\n");
        repo
    }

    /// A one-hunk git diff from `old` to `new`. Same line count: changed lines
    /// become -/+ pairs between context lines. Otherwise: replace everything.
    fn unified(rel: &str, old: &str, new: &str) -> String {
        let o: Vec<&str> = old.lines().collect();
        let n: Vec<&str> = new.lines().collect();
        let mut s = format!(
            "diff --git a/{rel} b/{rel}\nindex 1111111..2222222 100644\n--- a/{rel}\n+++ b/{rel}\n@@ -1,{} +1,{} @@\n",
            o.len(),
            n.len()
        );
        if o.len() == n.len() {
            for (a, b) in o.iter().zip(&n) {
                if a == b {
                    s.push_str(&format!(" {a}\n"));
                } else {
                    s.push_str(&format!("-{a}\n+{b}\n"));
                }
            }
        } else {
            for a in &o {
                s.push_str(&format!("-{a}\n"));
            }
            for b in &n {
                s.push_str(&format!("+{b}\n"));
            }
        }
        s
    }

    fn rejected(patch: &str, repo: &Path) -> String {
        match check_patch(patch, repo) {
            Ok(checked) => panic!("expected a rejection, got {checked:?}"),
            Err(reasons) => reasons.join("\n"),
        }
    }

    // ── rejections ──

    #[test]
    fn rejects_a_path_outside_the_allowlist() {
        let repo = fake_repo();
        let patch = unified(OTHER_CODE, "export const x = 1\n", "export const x = 2\n");
        let why = rejected(&patch, repo.path());
        assert!(why.contains("not on the allowlist"), "{why}");
    }

    #[test]
    fn rejects_an_allowed_edit_bundled_with_an_off_allowlist_one() {
        let repo = fake_repo();
        let patch = unified(&base_code(), BASE_CODE, &format!("{BASE_CODE}Check your work.\n"))
            + &unified(OTHER_CODE, "export const x = 1\n", "export const x = 2\n");
        let why = rejected(&patch, repo.path());
        assert!(why.contains(OTHER_CODE), "{why}");
    }

    #[test]
    fn rejects_a_patch_that_touches_the_tests() {
        let repo = fake_repo();
        let patch = unified("cmd/gizzi-code/test/session/guardrails.test.ts", "expect(1)\n", "expect(true)\n");
        let why = rejected(&patch, repo.path());
        assert!(why.contains("protected"), "{why}");
    }

    #[test]
    fn rejects_a_new_file_even_in_an_allowed_folder() {
        let repo = fake_repo();
        let rel = format!("{PROMPT_DIR}/extra.txt");
        let patch = format!(
            "diff --git a/{rel} b/{rel}\nnew file mode 100644\nindex 0000000..1111111\n--- /dev/null\n+++ b/{rel}\n@@ -0,0 +1 @@\n+hello\n"
        );
        let why = rejected(&patch, repo.path());
        assert!(why.contains("new or deleted files"), "{why}");
    }

    #[test]
    fn rejects_a_non_numeric_ts_edit() {
        let repo = fake_repo();
        let new = LIMIT.replace("DEFAULT_WARN_AT = 0.8", "DEFAULT_WARN_AT = Number(process.env.WARN)");
        let why = rejected(&unified(LIMIT_TS, LIMIT, &new), repo.path());
        assert!(why.contains("changes more than numbers"), "{why}");

        // Same numbers, renamed constant: still not numbers-only.
        let new = LIMIT.replace("DEFAULT_WRAP_UP_STEPS = 3", "WRAP_STEPS = 3");
        let why = rejected(&unified(LIMIT_TS, LIMIT, &new), repo.path());
        assert!(why.contains("changes more than numbers"), "{why}");
    }

    #[test]
    fn rejects_an_added_ts_line() {
        let repo = fake_repo();
        let patch = format!(
            "diff --git a/{LIMIT_TS} b/{LIMIT_TS}\n--- a/{LIMIT_TS}\n+++ b/{LIMIT_TS}\n@@ -4,1 +4,2 @@\n   export const DEFAULT_WRAP_UP_STEPS = 3\n+  export const EXTRA = 1\n"
        );
        let why = rejected(&patch, repo.path());
        assert!(why.contains("adds a line"), "{why}");
    }

    #[test]
    fn rejects_out_of_bounds_numbers() {
        let repo = fake_repo();
        let cases = [
            (LIMIT_TS, LIMIT, LIMIT.replace("DEFAULT_LAND_AT = 0.95", "DEFAULT_LAND_AT = 1.5"), "DEFAULT_LAND_AT"),
            (LIMIT_TS, LIMIT, LIMIT.replace("DEFAULT_WARN_AT = 0.8", "DEFAULT_WARN_AT = 0.2"), "DEFAULT_WARN_AT"),
            (LIMIT_TS, LIMIT, LIMIT.replace("DEFAULT_WRAP_UP_STEPS = 3", "DEFAULT_WRAP_UP_STEPS = 20"), "DEFAULT_WRAP_UP_STEPS"),
            (LIMIT_TS, LIMIT, LIMIT.replace("DEFAULT_WRAP_UP_STEPS = 3", "DEFAULT_WRAP_UP_STEPS = 2.5"), "whole number"),
            (GUARDRAIL_TS, GUARDRAIL, GUARDRAIL.replace("steps: 500,", "steps: 5000,"), "primary.steps"),
            (GUARDRAIL_TS, GUARDRAIL, GUARDRAIL.replace("toolCalls: 40,", "toolCalls: 10,"), "internal.toolCalls"),
            (
                GUARDRAIL_TS,
                GUARDRAIL,
                GUARDRAIL.replace("turnWallClockMs: 2 * 60 * 60_000", "turnWallClockMs: 5 * 60 * 60_000"),
                "primary.turnWallClockMs",
            ),
        ];
        for (rel, old, new, expect) in cases {
            let why = rejected(&unified(rel, old, &new), repo.path());
            assert!(why.contains("out of bounds") || why.contains("whole number"), "{expect}: {why}");
            assert!(why.contains(expect), "{expect}: {why}");
        }
    }

    #[test]
    fn rejects_warn_above_land() {
        let repo = fake_repo();
        let new = LIMIT.replace("DEFAULT_WARN_AT = 0.8", "DEFAULT_WARN_AT = 0.97");
        let why = rejected(&unified(LIMIT_TS, LIMIT, &new), repo.path());
        assert!(why.contains("must not be above"), "{why}");
    }

    #[test]
    fn rejects_numbers_that_are_not_tunable() {
        let repo = fake_repo();
        let cases = [
            (LIMIT_TS, LIMIT, LIMIT.replace("ratio * 100", "ratio * 1000")),
            (GUARDRAIL_TS, GUARDRAIL, GUARDRAIL.replace("steps: 2_000,", "steps: 2_100,")),
            (GUARDRAIL_TS, GUARDRAIL, GUARDRAIL.replace("2026-10-01.1", "2026-10-01.2")),
        ];
        for (rel, old, new) in cases {
            let why = rejected(&unified(rel, old, &new), repo.path());
            assert!(why.contains("only"), "{why}");
        }
    }

    #[test]
    fn rejects_an_added_vendor_name_in_any_case() {
        let repo = fake_repo();
        for word in ["Work the way Claude does.", "Answer like GPT would.", "Use the gemini style."] {
            let patch = unified(&base_code(), BASE_CODE, &format!("{BASE_CODE}{word}\n"));
            let why = rejected(&patch, repo.path());
            assert!(why.contains("names a vendor"), "{word}: {why}");
        }
        let patch = unified(&read_tool(), READ_TOOL, "Reads a file, like Kimi.\n");
        assert!(rejected(&patch, repo.path()).contains("names a vendor"));
    }

    #[test]
    fn rejects_an_over_cap_prompt() {
        let repo = fake_repo();
        let long = format!("{}\n", "Ask first. ".repeat(40));
        let patch = unified(&mode_ask(), MODE_ASK, &long);
        let why = rejected(&patch, repo.path());
        assert!(why.contains("over its cap of 400 bytes"), "{why}");
    }

    #[test]
    fn rejects_a_patch_that_does_not_fit_the_baseline() {
        let repo = fake_repo();
        let patch = unified(&base_code(), "Some other text.\n", "New text.\n");
        let why = rejected(&patch, repo.path());
        assert!(why.contains("does not fit"), "{why}");
    }

    #[test]
    fn rejects_an_empty_patch() {
        let repo = fake_repo();
        assert!(rejected("", repo.path()).contains("empty"));
        assert!(rejected("\n\n", repo.path()).contains("empty"));
    }

    // ── accepts ──

    #[test]
    fn accepts_a_clean_numeric_change() {
        let repo = fake_repo();
        let limit = LIMIT
            .replace("DEFAULT_WARN_AT = 0.8", "DEFAULT_WARN_AT = 0.75")
            .replace("DEFAULT_WRAP_UP_STEPS = 3", "DEFAULT_WRAP_UP_STEPS = 5");
        let guard =
            GUARDRAIL.replace("steps: 500,", "steps: 600,").replace("turnWallClockMs: 2 * 60 * 60_000", "turnWallClockMs: 3 * 60 * 60_000");
        let patch = unified(LIMIT_TS, LIMIT, &limit) + &unified(GUARDRAIL_TS, GUARDRAIL, &guard);
        let checked = check_patch(&patch, repo.path()).unwrap();
        assert_eq!(checked.files, vec![(LIMIT_TS.to_string(), limit), (GUARDRAIL_TS.to_string(), guard)]);
    }

    #[test]
    fn accepts_a_clean_txt_change() {
        let repo = fake_repo();
        let prompt = format!("{BASE_CODE}Run the tests before you say done.\n");
        let tool = "Reads a text file from disk.\n";
        let patch = unified(&base_code(), BASE_CODE, &prompt) + &unified(&read_tool(), READ_TOOL, tool);
        let checked = check_patch(&patch, repo.path()).unwrap();
        assert_eq!(checked.files, vec![(base_code(), prompt), (read_tool(), tool.to_string())]);
    }

    // ── the ValidityCheck ──

    struct FakeGuard {
        passed: bool,
        runs: Arc<AtomicUsize>,
    }
    impl GuardRunner for FakeGuard {
        fn run(&self, _checkout: &Path) -> anyhow::Result<GuardOutcome> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            Ok(GuardOutcome { passed: self.passed, output: "1 fail".into() })
        }
    }

    fn candidate_with(repo: &Path, rel: &str, text: &str) -> tempfile::TempDir {
        let candidate = tempfile::tempdir().unwrap();
        for file in [LIMIT_TS, GUARDRAIL_TS] {
            write(candidate.path(), file, &std::fs::read_to_string(repo.join(file)).unwrap());
        }
        write(candidate.path(), rel, text);
        candidate
    }

    #[test]
    fn check_passes_a_clean_patch_when_the_guard_tests_pass() {
        let repo = fake_repo();
        let limit = LIMIT.replace("DEFAULT_LAND_AT = 0.95", "DEFAULT_LAND_AT = 0.9");
        let candidate = candidate_with(repo.path(), LIMIT_TS, &limit);
        let runs = Arc::new(AtomicUsize::new(0));
        let validity =
            HarnessValidity::new(repo.path(), unified(LIMIT_TS, LIMIT, &limit), Box::new(FakeGuard { passed: true, runs: runs.clone() }));
        assert!(validity.check(candidate.path()).unwrap());
        assert!(validity.rejections().is_empty());
    }

    #[test]
    fn check_fails_when_the_guard_tests_fail() {
        let repo = fake_repo();
        let limit = LIMIT.replace("DEFAULT_LAND_AT = 0.95", "DEFAULT_LAND_AT = 0.9");
        let candidate = candidate_with(repo.path(), LIMIT_TS, &limit);
        let runs = Arc::new(AtomicUsize::new(0));
        let validity = HarnessValidity::new(repo.path(), unified(LIMIT_TS, LIMIT, &limit), Box::new(FakeGuard { passed: false, runs }));
        assert!(!validity.check(candidate.path()).unwrap());
        assert!(validity.rejections()[0].contains("guard tests failed"));
    }

    #[test]
    fn check_rejects_a_bad_patch_without_starting_the_guard_tests() {
        let repo = fake_repo();
        let prompt = format!("{BASE_CODE}Copy what Codex does.\n");
        let candidate = candidate_with(repo.path(), &base_code(), &prompt);
        let runs = Arc::new(AtomicUsize::new(0));
        let validity = HarnessValidity::new(
            repo.path(),
            unified(&base_code(), BASE_CODE, &prompt),
            Box::new(FakeGuard { passed: true, runs: runs.clone() }),
        );
        assert!(!validity.check(candidate.path()).unwrap());
        assert!(validity.rejections()[0].contains("names a vendor"));
        assert_eq!(runs.load(Ordering::SeqCst), 0, "a static failure must not start the bun run");
    }

    #[test]
    fn check_rejects_a_checkout_that_does_not_hold_the_patch() {
        let repo = fake_repo();
        let limit = LIMIT.replace("DEFAULT_LAND_AT = 0.95", "DEFAULT_LAND_AT = 0.9");
        // The checkout still has the old file: the patch was never applied.
        let candidate = candidate_with(repo.path(), LIMIT_TS, LIMIT);
        let runs = Arc::new(AtomicUsize::new(0));
        let validity = HarnessValidity::new(repo.path(), unified(LIMIT_TS, LIMIT, &limit), Box::new(FakeGuard { passed: true, runs }));
        assert!(!validity.check(candidate.path()).unwrap());
        assert!(validity.rejections()[0].contains("does not match the patch"));
    }

    #[test]
    fn guard_command_is_the_five_guard_tests() {
        assert_eq!(&GUARD_TEST_CMD[..2], &["bun", "test"]);
        assert_eq!(GUARD_TEST_CMD.len(), 7);
        assert!(GUARD_TEST_CMD[2..].iter().all(|t| t.starts_with("test/session/") && t.ends_with(".test.ts")));
    }
}
