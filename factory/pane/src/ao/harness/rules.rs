//! Rules marker-block mechanics — lib.js:199-268 port.

use super::{expand, write_text, Action};

pub(crate) const MARK_START: &str = "<!-- allternit-harness:start -->";
pub(crate) const MARK_END: &str = "<!-- allternit-harness:end -->";

/// `buildRulesBlock()`.
pub(crate) fn build_rules_block(content: &str) -> String {
    format!("{MARK_START}\n{}\n{MARK_END}\n", content.trim_end())
}

/// `frontmatterText()` — cursor's managed unit includes a YAML frontmatter
/// header when configured.
fn frontmatter_text(frontmatter: Option<&str>) -> String {
    frontmatter
        .map(|fm| format!("---\n{fm}\n---\n\n"))
        .unwrap_or_default()
}

/// `rulesBlockStatus()`: presence is MARK_START inclusion only.
pub(crate) fn rules_block_status(file: &str) -> std::io::Result<&'static str> {
    let f = expand(file);
    if !f.exists() {
        return Ok("no-file");
    }
    let text = std::fs::read_to_string(&f)?;
    Ok(if text.contains(MARK_START) { "present" } else { "missing" })
}

/// `upsertRulesBlock()` — lib.js:214-241. Marker search is first-occurrence
/// (`indexOf`); pairing requires end-after-start; the cursor frontmatter unit
/// is replaced only when it immediately precedes MARK_START.
pub(crate) fn upsert_rules_block(
    file: &str,
    block: &str,
    dry_run: bool,
    frontmatter: Option<&str>,
) -> std::io::Result<Vec<Action>> {
    let f = expand(file);
    let fm = frontmatter_text(frontmatter);
    let managed = format!("{fm}{block}");
    if !f.exists() {
        let actions = vec![Action::detailed(
            if dry_run { "would-create" } else { "create" },
            f.display().to_string(),
        )];
        if !dry_run {
            write_text(&f.to_string_lossy(), &managed)?;
        }
        return Ok(actions);
    }
    let old = std::fs::read_to_string(&f)?;
    let start = old.find(MARK_START);
    let end = old.find(MARK_END);
    if let (Some(s), Some(e)) = (start, end) {
        if e > s {
            let unit_start = if !fm.is_empty() && old[..s].ends_with(&fm) {
                s - fm.len()
            } else {
                s
            };
            let current = &old[unit_start..e + MARK_END.len()];
            if current == managed.trim_end() {
                return Ok(vec![Action::unchanged()]);
            }
            let actions = vec![Action::detailed(
                if dry_run { "would-update" } else { "update" },
                f.display().to_string(),
            )];
            if !dry_run {
                let out = format!(
                    "{}{}{}",
                    &old[..unit_start],
                    managed,
                    &old[e + MARK_END.len()..]
                );
                write_text(&f.to_string_lossy(), &out)?;
            }
            return Ok(actions);
        }
    }
    // Append: strip ALL trailing whitespace, add exactly two newlines.
    let actions = vec![Action::detailed(
        if dry_run { "would-append" } else { "append" },
        f.display().to_string(),
    )];
    if !dry_run {
        // old.replace(/\s*$/, "\n\n") + managed — trim_end + exactly two newlines.
        let out = format!("{}\n\n{managed}", old.trim_end());
        write_text(&f.to_string_lossy(), &out)?;
    }
    Ok(actions)
}

/// `removeRulesBlock()` — lib.js:243-268. NOTE: unlike upsert, this does NOT
/// require end-after-start (JS only checks both markers present) — the splice
/// is reproduced faithfully even for inverted markers.
pub(crate) fn remove_rules_block(
    file: &str,
    dry_run: bool,
    frontmatter: Option<&str>,
) -> std::io::Result<Vec<Action>> {
    let f = expand(file);
    let fm = frontmatter_text(frontmatter);
    if !f.exists() {
        return Ok(vec![Action::nothing("no file")]);
    }
    let old = std::fs::read_to_string(&f)?;
    let start = old.find(MARK_START);
    let end = old.find(MARK_END);
    if start.is_none() || end.is_none() {
        return Ok(vec![Action::nothing("no harness block")]);
    }
    let s = start.unwrap();
    let e = end.unwrap();
    let unit_start = if !fm.is_empty() && old[..s].ends_with(&fm) {
        s - fm.len()
    } else {
        s
    };
    let remainder = format!("{}{}", &old[..unit_start], &old[e + MARK_END.len()..]).trim().to_string();
    if remainder.is_empty() {
        let actions = vec![Action::detailed(
            if dry_run { "would-delete-file" } else { "delete-file" },
            f.display().to_string(),
        )];
        if !dry_run {
            let _ = std::fs::remove_file(&f);
        }
        Ok(actions)
    } else {
        let actions = vec![Action::detailed(
            if dry_run { "would-update" } else { "update" },
            f.display().to_string(),
        )];
        if !dry_run {
            write_text(&f.to_string_lossy(), &format!("{remainder}\n"))?;
        }
        Ok(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: &str = "# Rules\n\nBe careful.\n";

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ao-harness-rules-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    use std::path::PathBuf;

    #[test]
    fn create_then_unchanged_then_update() {
        let dir = tmp("basic");
        let file = dir.join("AGENTS.md");
        let file_str = file.to_string_lossy().into_owned();
        let block = build_rules_block(RULES);

        let actions = upsert_rules_block(&file_str, &block, false, None).unwrap();
        assert_eq!(actions[0].kind, "create");
        let written = std::fs::read_to_string(&file).unwrap();
        assert_eq!(written, block);

        let actions = upsert_rules_block(&file_str, &block, false, None).unwrap();
        assert_eq!(actions[0].kind, "unchanged");

        let block2 = build_rules_block("# New\n");
        let actions = upsert_rules_block(&file_str, &block2, false, None).unwrap();
        assert_eq!(actions[0].kind, "update");
        let written = std::fs::read_to_string(&file).unwrap();
        // Update splices [unitStart, endOfEndMarker): the newline that
        // followed the old end marker survives (verified against the JS).
        assert_eq!(written, format!("{block2}\n"));
        // Backup side effect: update goes through writeText.
        assert_eq!(std::fs::read_to_string(file.with_file_name("AGENTS.md.bak-harness")).unwrap(), block);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn appends_with_exactly_two_newlines() {
        let dir = tmp("append");
        let file = dir.join("AGENTS.md");
        std::fs::write(&file, "User notes.   \n\n\n").unwrap();
        let block = build_rules_block(RULES);
        let actions = upsert_rules_block(&file.to_string_lossy(), &block, false, None).unwrap();
        assert_eq!(actions[0].kind, "append");
        let written = std::fs::read_to_string(&file).unwrap();
        assert_eq!(written, format!("User notes.\n\n{block}"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cursor_frontmatter_is_part_of_the_unit() {
        let dir = tmp("frontmatter");
        let file = dir.join("allternit.mdc");
        let fm = "description: Allternit ops harness rules\nglobs: \nalwaysApply: true";
        let block = build_rules_block(RULES);
        let managed = format!("---\n{fm}\n---\n\n{block}");
        std::fs::write(&file, &managed).unwrap();

        // Same content → unchanged.
        let actions = upsert_rules_block(&file.to_string_lossy(), &block, false, Some(fm)).unwrap();
        assert_eq!(actions[0].kind, "unchanged");

        // Changed rules → update replaces fm + block as one unit.
        let block2 = build_rules_block("# v2\n");
        let actions = upsert_rules_block(&file.to_string_lossy(), &block2, false, Some(fm)).unwrap();
        assert_eq!(actions[0].kind, "update");
        let written = std::fs::read_to_string(&file).unwrap();
        assert_eq!(written, format!("---\n{fm}\n---\n\n{block2}\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_splices_and_trims() {
        let dir = tmp("remove");
        let file = dir.join("AGENTS.md");
        let block = build_rules_block(RULES);
        std::fs::write(&file, format!("Prefix\n\n{block}\nSuffix\n")).unwrap();
        let actions = remove_rules_block(&file.to_string_lossy(), false, None).unwrap();
        assert_eq!(actions[0].kind, "update");
        let written = std::fs::read_to_string(&file).unwrap();
        // The newline after the end marker plus the blank line around the
        // block survive the splice (JS-verified): four internal newlines.
        assert_eq!(written, "Prefix\n\n\n\nSuffix\n");

        // Whole-file block → delete-file.
        std::fs::write(&file, block.clone()).unwrap();
        let actions = remove_rules_block(&file.to_string_lossy(), false, None).unwrap();
        assert_eq!(actions[0].kind, "delete-file");
        assert!(!file.exists());

        // Missing file / no markers → nothing actions.
        let actions = remove_rules_block(&file.to_string_lossy(), false, None).unwrap();
        assert_eq!(actions[0].kind, "nothing");
        assert_eq!(actions[0].detail.as_deref(), Some("no file"));
        std::fs::write(&file, "plain\n").unwrap();
        let actions = remove_rules_block(&file.to_string_lossy(), false, None).unwrap();
        assert_eq!(actions[0].detail.as_deref(), Some("no harness block"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
