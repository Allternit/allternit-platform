//! Skills sync mechanics — lib.js:44-197 port: enumeration, drift hash,
//! managed manifest, install/update/remove actions, uninstall.

use std::path::{Path, PathBuf};

use sha2::Digest as _;

use super::collate::locale_compare;
use super::json_val::JVal;
use super::{expand, hex_lower, iso8601_now, read_json, Action};

pub(crate) const MANAGED_MANIFEST: &str = ".allternit-harness.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkillsFormat {
    Dir,
    Flat,
}

/// `listSkills()`: immediate children that are directories or symlinks and
/// contain `SKILL.md`, sorted. JS uses default `.sort()` (UTF-16 code-unit
/// order); code-unit order equals code-point order for the BMP, and Rust's
/// `str` Ord is code-point (byte) order, so the two agree. Non-BMP names are
/// the documented non-ASCII edge (see collate.rs).
pub(crate) fn list_skills(src_dir: &str) -> std::io::Result<Vec<String>> {
    let dir = expand(src_dir);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_dir() && !file_type.is_symlink() {
            continue;
        }
        if entry.path().join("SKILL.md").exists() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// `hashDir()`: sha256 over the recursive walk, entries sorted with
/// `localeCompare` (NOT byte order — see collate.rs), updating
/// `relpath\0bytes\0` per file. Symlinks are skipped, matching Node's
/// `Dirent.isFile()` being false for symlink entries.
pub(crate) fn hash_dir(dir: &Path) -> std::io::Result<String> {
    let mut hash = sha2::Sha256::new();
    hash_dir_into(dir, "", &mut hash)?;
    Ok(hex_lower(&hash.finalize()))
}

fn hash_dir_into(dir: &Path, rel: &str, hash: &mut sha2::Sha256) -> std::io::Result<()> {
    let mut entries: Vec<(String, PathBuf, std::fs::FileType)> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        entries.push((
            entry.file_name().to_string_lossy().into_owned(),
            entry.path(),
            entry.file_type()?,
        ));
    }
    entries.sort_by(|a, b| locale_compare(&a.0, &b.0));
    for (name, path, file_type) in entries {
        let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        if file_type.is_dir() {
            hash_dir_into(&path, &r, hash)?;
        } else if file_type.is_file() {
            hash.update(r.as_bytes());
            hash.update([0]);
            hash.update(std::fs::read(&path)?);
            hash.update([0]);
        }
    }
    Ok(())
}

fn hash_file(file: &Path) -> std::io::Result<String> {
    let mut hash = sha2::Sha256::new();
    hash.update(std::fs::read(file)?);
    Ok(hex_lower(&hash.finalize()))
}

/// `copyDir()`: recursive copy; symlink entries are skipped, matching the JS
/// Dirent checks. (Top-level skill dirs may BE symlinks — the copy then
/// follows them because the source path is opened directly, which is also
/// what the JS does.)
pub(crate) fn copy_dir(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir(&entry.path(), &dest.join(entry.file_name()))?;
        } else if file_type.is_file() {
            std::fs::copy(entry.path(), dest.join(entry.file_name()))?;
        }
    }
    Ok(())
}

pub(crate) fn remove_dir(dir: &Path) {
    let _ = std::fs::remove_dir_all(expand_path(dir));
}

fn expand_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    expand(&text)
}

fn skill_target(tdir: &Path, name: &str, format: SkillsFormat) -> PathBuf {
    match format {
        SkillsFormat::Flat => tdir.join(format!("{name}.md")),
        SkillsFormat::Dir => tdir.join(name),
    }
}

fn skill_hash(target: &Path, format: SkillsFormat) -> std::io::Result<String> {
    match format {
        SkillsFormat::Flat => hash_file(target),
        SkillsFormat::Dir => hash_dir(target),
    }
}

fn source_skill_hash(src_dir: &str, name: &str, format: SkillsFormat) -> std::io::Result<String> {
    let src = expand(src_dir).join(name);
    match format {
        SkillsFormat::Flat => hash_file(&src.join("SKILL.md")),
        SkillsFormat::Dir => hash_dir(&src),
    }
}

/// `syncSkills()` — lib.js:101-153. Removal pass is driven by the PREVIOUS
/// managed manifest, not by comparing against source.
pub(crate) fn sync_skills(
    src_dir: &str,
    target_dir: &str,
    dry_run: bool,
    format: SkillsFormat,
) -> std::io::Result<Vec<Action>> {
    let mut actions = Vec::new();
    let skills = list_skills(src_dir)?;
    let tdir = expand(target_dir);
    let manifest_path = tdir.join(MANAGED_MANIFEST);
    let prev = read_json(&manifest_path.to_string_lossy());

    // Object.keys(prev.skills || {}) in file order (JVal preserves it).
    // Non-object `skills` values yield no names here; JS would only differ
    // for the pathological case of a string skills value (index keys).
    let prev_names: Vec<String> = prev
        .as_ref()
        .and_then(|value| value.get("skills"))
        .and_then(|skills| match skills {
            JVal::Obj(fields) => Some(fields.iter().map(|(key, _)| key.clone()).collect()),
            _ => None,
        })
        .unwrap_or_default();

    if !tdir.exists() {
        actions.push(Action::detailed(
            if dry_run { "would-create-dir" } else { "create-dir" },
            tdir.display().to_string(),
        ));
    }

    let mut next_skills: Vec<(String, String)> = Vec::new();
    for name in &skills {
        let src = expand(src_dir).join(name);
        let dst = skill_target(&tdir, name, format);
        let hash = source_skill_hash(src_dir, name, format)?;
        next_skills.push((name.clone(), hash.clone()));
        if !dst.exists() {
            actions.push(Action::named(
                if dry_run { "would-install" } else { "install" },
                name,
            ));
            if !dry_run {
                if format == SkillsFormat::Flat {
                    std::fs::create_dir_all(&tdir)?;
                    std::fs::copy(src.join("SKILL.md"), &dst)?;
                } else {
                    copy_dir(&src, &dst)?;
                }
            }
        } else if skill_hash(&dst, format)? != hash {
            actions.push(Action::named(
                if dry_run { "would-update" } else { "update" },
                name,
            ));
            if !dry_run {
                if format == SkillsFormat::Flat {
                    std::fs::copy(src.join("SKILL.md"), &dst)?;
                } else {
                    remove_dir(&dst);
                    copy_dir(&src, &dst)?;
                }
            }
        }
    }

    for name in &prev_names {
        let dst = skill_target(&tdir, name, format);
        if next_skills.iter().any(|(next, _)| next == name) || !dst.exists() {
            continue;
        }
        let mut action = Action::named(
            if dry_run { "would-remove" } else { "remove" },
            name,
        );
        action.reason = Some("no longer in source".to_string());
        actions.push(action);
        if !dry_run {
            match format {
                SkillsFormat::Flat => {
                    let _ = std::fs::remove_file(&dst);
                }
                SkillsFormat::Dir => remove_dir(&dst),
            }
        }
    }

    if !dry_run {
        std::fs::create_dir_all(&tdir)?;
        let manifest = JVal::Obj(vec![
            ("version".to_string(), JVal::Num("1".to_string())),
            ("syncedAt".to_string(), JVal::Str(iso8601_now())),
            ("skills".to_string(), JVal::Obj(
                next_skills
                    .into_iter()
                    .map(|(name, hash)| (name, JVal::Str(hash)))
                    .collect(),
            )),
        ]);
        // Direct write — no .bak-harness backup (lib.js:148-151).
        std::fs::write(&manifest_path, format!("{}\n", super::json_val::print_pretty(&manifest)))
            .map_err(|err| std::io::Error::new(err.kind(), format!("cannot write {}: {err}", manifest_path.display())))?;
    }
    Ok(actions)
}

/// `skillsStatus()` — lib.js:155-173.
pub(crate) fn skills_status(src_dir: &str, target_dir: &str, format: SkillsFormat) -> std::io::Result<String> {
    let skills = list_skills(src_dir)?;
    if skills.is_empty() {
        return Ok("no source skills".to_string());
    }
    let tdir = expand(target_dir);
    if !tdir.exists() {
        return Ok("not synced".to_string());
    }
    let mut synced = 0usize;
    let mut missing = 0usize;
    let mut drifted = 0usize;
    for name in &skills {
        let dst = skill_target(&tdir, name, format);
        if !dst.exists() {
            missing += 1;
        } else if skill_hash(&dst, format)? != source_skill_hash(src_dir, name, format)? {
            drifted += 1;
        } else {
            synced += 1;
        }
    }
    let mut parts = vec![format!("{synced}/{} synced", skills.len())];
    if missing > 0 {
        parts.push(format!("{missing} missing"));
    }
    if drifted > 0 {
        parts.push(format!("{drifted} drifted"));
    }
    Ok(parts.join(", "))
}

/// `uninstallSkills()` — lib.js:175-197. Only manifest-listed skills are
/// touched; no manifest → single `nothing (no managed manifest)` action.
pub(crate) fn uninstall_skills(
    target_dir: &str,
    dry_run: bool,
    format: SkillsFormat,
) -> std::io::Result<Vec<Action>> {
    let mut actions = Vec::new();
    let tdir = expand(target_dir);
    let manifest_path = tdir.join(MANAGED_MANIFEST);
    let manifest = read_json(&manifest_path.to_string_lossy());
    let Some(manifest) = manifest.filter(|value| value.js_truthy()) else {
        return Ok(vec![Action::nothing("no managed manifest")]);
    };
    let names: Vec<String> = manifest
        .get("skills")
        .and_then(|skills| match skills {
            JVal::Obj(fields) => Some(fields.iter().map(|(key, _)| key.clone()).collect()),
            _ => None,
        })
        .unwrap_or_default();
    for name in &names {
        let dst = skill_target(&tdir, name, format);
        if dst.exists() {
            actions.push(Action::named(
                if dry_run { "would-remove" } else { "remove" },
                name,
            ));
            if !dry_run {
                match format {
                    SkillsFormat::Flat => {
                        let _ = std::fs::remove_file(&dst);
                    }
                    SkillsFormat::Dir => remove_dir(&dst),
                }
            }
        }
    }
    actions.push(Action::detailed(
        if dry_run { "would-remove" } else { "remove" },
        MANAGED_MANIFEST,
    ));
    if !dry_run {
        let _ = std::fs::remove_file(&manifest_path);
    }
    Ok(actions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_skills(root: &Path) -> PathBuf {
        let src = root.join("src-skills");
        std::fs::create_dir_all(src.join("alpha")).unwrap();
        std::fs::write(src.join("alpha/SKILL.md"), "alpha skill\n").unwrap();
        std::fs::create_dir_all(src.join("beta/scripts")).unwrap();
        std::fs::write(src.join("beta/SKILL.md"), "beta skill\n").unwrap();
        std::fs::write(src.join("beta/scripts/run.sh"), "#!/bin/sh\n").unwrap();
        // Case-torture sibling set: exercises the localeCompare replica.
        std::fs::create_dir_all(src.join("case-test")).unwrap();
        std::fs::write(src.join("case-test/SKILL.md"), "case\n").unwrap();
        std::fs::write(src.join("case-test/compute.py"), "print(1)\n").unwrap();
        src
    }

    fn snapshot(root: &Path) -> Vec<String> {
        let mut files = Vec::new();
        fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let rel = path.strip_prefix(base).unwrap().to_string_lossy().into_owned();
                if path.is_dir() {
                    walk(base, &path, out);
                } else {
                    out.push(format!("{rel}:{}", std::fs::read_to_string(&path).unwrap()));
                }
            }
        }
        walk(root, root, &mut files);
        files.sort();
        files
    }

    #[test]
    fn sync_installs_and_writes_manifest() {
        let root = std::env::temp_dir().join(format!("ao-harness-skills-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = fixture_skills(&root);
        let target = root.join("target");

        let actions = sync_skills(&src.to_string_lossy(), &target.to_string_lossy(), false, SkillsFormat::Dir).unwrap();
        let kinds: Vec<&str> = actions.iter().map(|a| a.kind.as_str()).collect();
        assert_eq!(kinds, ["create-dir", "install", "install", "install"]);

        let manifest = std::fs::read_to_string(target.join(MANAGED_MANIFEST)).unwrap();
        assert!(manifest.starts_with("{\n  \"version\": 1,\n  \"syncedAt\": \""), "manifest shape: {manifest}");
        assert!(manifest.ends_with("}\n"));
        assert!(manifest.contains("\"alpha\": \""));

        // Re-sync: everything unchanged, no actions beyond nothing-at-all.
        let actions = sync_skills(&src.to_string_lossy(), &target.to_string_lossy(), false, SkillsFormat::Dir).unwrap();
        assert!(actions.is_empty(), "second sync must be a no-op: {actions:?}");

        // Drift a target skill → update action + .bak-harness NOT created for
        // skill dirs (skill copies bypass writeText), manifest rewritten.
        std::fs::write(target.join("alpha/SKILL.md"), "tampered\n").unwrap();
        let actions = sync_skills(&src.to_string_lossy(), &target.to_string_lossy(), false, SkillsFormat::Dir).unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, "update");
        assert_eq!(actions[0].name.as_deref(), Some("alpha"));
        assert_eq!(
            std::fs::read_to_string(target.join("alpha/SKILL.md")).unwrap(),
            "alpha skill\n"
        );

        // Remove from source → removal pass picks it up from prev manifest.
        std::fs::remove_dir_all(src.join("beta")).unwrap();
        let actions = sync_skills(&src.to_string_lossy(), &target.to_string_lossy(), false, SkillsFormat::Dir).unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, "remove");
        assert_eq!(actions[0].reason.as_deref(), Some("no longer in source"));
        assert!(!target.join("beta").exists());

        // Dry run reports would-* and writes nothing.
        let before = snapshot(&target);
        let actions = sync_skills(&src.to_string_lossy(), &target.to_string_lossy(), true, SkillsFormat::Dir).unwrap();
        assert!(actions.is_empty(), "in-sync dry run: {actions:?}");
        assert_eq!(snapshot(&target), before);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn uninstall_removes_only_manifest_listed() {
        let root = std::env::temp_dir().join(format!("ao-harness-uninstall-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = fixture_skills(&root);
        let target = root.join("target");
        sync_skills(&src.to_string_lossy(), &target.to_string_lossy(), false, SkillsFormat::Dir).unwrap();
        // A skill that exists but is NOT in the manifest must survive.
        std::fs::create_dir_all(target.join("foreign")).unwrap();
        std::fs::write(target.join("foreign/SKILL.md"), "not managed\n").unwrap();

        let actions = uninstall_skills(&target.to_string_lossy(), false, SkillsFormat::Dir).unwrap();
        let kinds: Vec<&str> = actions.iter().map(|a| a.kind.as_str()).collect();
        assert_eq!(kinds.first(), Some(&"remove"));
        assert_eq!(actions.last().unwrap().detail.as_deref(), Some(MANAGED_MANIFEST));
        assert!(target.join("foreign").exists(), "foreign skill untouched");
        assert!(!target.join(MANAGED_MANIFEST).exists());

        // No manifest → single nothing action.
        let actions = uninstall_skills(&target.to_string_lossy(), false, SkillsFormat::Dir).unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, "nothing");
        assert_eq!(actions[0].detail.as_deref(), Some("no managed manifest"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn flat_format_syncs_single_files() {
        let root = std::env::temp_dir().join(format!("ao-harness-flat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = fixture_skills(&root);
        let target = root.join("flat-target");
        let actions = sync_skills(&src.to_string_lossy(), &target.to_string_lossy(), false, SkillsFormat::Flat).unwrap();
        assert!(actions.iter().all(|a| a.kind == "install" || a.kind == "create-dir"));
        assert!(target.join("alpha.md").exists());
        assert!(!target.join("alpha").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hash_dir_uses_locale_collation_not_byte_order() {
        let root = std::env::temp_dir().join(format!("ao-harness-hash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("skill");
        std::fs::create_dir_all(&dir).unwrap();
        // Byte order would put SKILL.md first; localeCompare puts compute.py
        // first (case-insensitive primary). Both files hash either way, but
        // the hash must match the JS implementation's localeCompare order.
        std::fs::write(dir.join("SKILL.md"), "x\n").unwrap();
        std::fs::write(dir.join("compute.py"), "y\n").unwrap();
        let hash = hash_dir(&dir).unwrap();
        assert_eq!(hash.len(), 64);

        // Reference computed with node lib.js hashDir ordering on the same
        // bytes (localeCompare order: compute.py, SKILL.md):
        //   sha256("compute.py\0y\n\0SKILL.md\0x\n\0") in hex — verified below.
        let mut reference = sha2::Sha256::new();
        reference.update("compute.py");
        reference.update([0]);
        reference.update("y\n");
        reference.update([0]);
        reference.update("SKILL.md");
        reference.update([0]);
        reference.update("x\n");
        reference.update([0]);
        assert_eq!(hash, hex_lower(&reference.finalize()));
        let _ = std::fs::remove_dir_all(&root);
    }
}
