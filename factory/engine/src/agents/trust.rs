//! Folder trust for a spawned Terminal bot.
//!
//! A harness opened in a folder it has never seen stops on a "trust this
//! folder" prompt (Claude Code also asks once about external CLAUDE.md
//! includes). Nobody is watching a Factory pane at spawn, so the bot sat idle
//! and every send queued behind the dialog. The team's workdir was chosen by
//! the person who ran `agents up`, and the bot already runs gated with
//! bypassPermissions, so the engine records the trust each harness would ask
//! for before the pane starts.
//!
//! Best effort: a missing or unreadable store is left alone (never
//! rewritten from scratch), and the store is replaced atomically, so a
//! running harness never reads a half-written file.

use std::path::Path;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Record trust for `workdir` in the stores of `harness` (`claude`,
/// `codex` or `kimi`/`kimi-code`; others have no folder prompt). `home` is `$HOME`.
pub fn pre_trust(harness: &str, workdir: &Path, home: &Path) {
    let name = Path::new(harness.trim())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    // The harness may key the folder by the path it was given or by the real
    // one (/tmp vs /private/tmp on macOS); trust both when they differ.
    let mut dirs = vec![workdir.to_path_buf()];
    if let Ok(real) = workdir.canonicalize() {
        if real != workdir {
            dirs.push(real);
        }
    }
    for dir in &dirs {
        match name.as_str() {
            "claude" | "claude-code" => trust_claude(&home.join(".claude.json"), dir),
            "kimi" | "kimi-code" => trust_kimi(&home.join(".kimi-code").join("workspace-trust"), dir),
            "codex" => trust_codex(&home.join(".codex").join("config.toml"), dir),
            _ => {}
        }
    }
}

/// `~/.codex/config.toml` → `[projects."<workdir>"]` with
/// `trust_level = "trusted"`. The table is appended as text so the person's
/// comments and layout stay untouched. An existing table (whatever its
/// trust_level) is the person's choice and is left alone.
fn trust_codex(store: &Path, workdir: &Path) {
    let Ok(text) = std::fs::read_to_string(store) else { return };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else { return };
    let key = workdir.to_string_lossy().to_string();
    match doc.get("projects") {
        None => {}
        // An inline or dotted `projects` can't take a `[projects."x"]` header.
        Some(projects) => {
            let Some(table) = projects.as_table() else { return };
            if table.contains_key(&key) {
                return;
            }
        }
    }
    let escaped = key.replace('\\', "\\\\").replace('"', "\\\"");
    let mut out = text;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!("\n[projects.\"{escaped}\"]\ntrust_level = \"trusted\"\n"));
    let _ = replace_atomically(store, out.as_bytes());
}

/// `~/.claude.json` → `projects[<workdir>]` gets both dialog flags.
fn trust_claude(store: &Path, workdir: &Path) {
    let Ok(text) = std::fs::read_to_string(store) else { return };
    let Ok(mut doc) = serde_json::from_str::<Value>(&text) else { return };
    let Some(root) = doc.as_object_mut() else { return };
    let projects = root.entry("projects").or_insert_with(|| json!({}));
    let Some(projects) = projects.as_object_mut() else { return };
    let key = workdir.to_string_lossy().to_string();
    let project = projects.entry(key).or_insert_with(|| json!({}));
    let Some(project) = project.as_object_mut() else { return };
    if project.get("hasTrustDialogAccepted") == Some(&json!(true))
        && project.get("hasClaudeMdExternalIncludesApproved") == Some(&json!(true))
    {
        return;
    }
    project.insert("hasTrustDialogAccepted".into(), json!(true));
    project.insert("hasClaudeMdExternalIncludesApproved".into(), json!(true));
    if let Ok(out) = serde_json::to_string_pretty(&doc) {
        let _ = replace_atomically(store, out.as_bytes());
    }
}

/// `~/.kimi-code/workspace-trust/wd_<basename>_<sha256(path)[:12]>` holding
/// `{"root": <path>, "trustedAt": <ms>}`.
fn trust_kimi(dir: &Path, workdir: &Path) {
    if !dir.parent().is_some_and(Path::is_dir) {
        return;
    }
    let path = workdir.to_string_lossy();
    let hash = hex::encode(Sha256::digest(path.as_bytes()));
    let base = workdir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let file = dir.join(format!("wd_{base}_{}", &hash[..12]));
    if file.exists() || std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let body = json!({ "root": path, "trustedAt": chrono::Utc::now().timestamp_millis() });
    let _ = replace_atomically(&file, body.to_string().as_bytes());
}

fn replace_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = tempfile::NamedTempFile::new_in(dir)?;
    std::fs::write(tmp.path(), bytes)?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(tmp.path(), meta.permissions());
    }
    tmp.persist(path).map(|_| ()).map_err(|e| e.error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_trust_is_merged_into_the_existing_store() {
        let home = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let store = home.path().join(".claude.json");
        std::fs::write(&store, r#"{"oauthAccount":{"id":"keep"},"projects":{"/other":{"x":1}}}"#).unwrap();
        pre_trust("claude", work.path(), home.path());
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&store).unwrap()).unwrap();
        let key = work.path().canonicalize().unwrap().to_string_lossy().to_string();
        assert_eq!(doc["projects"][&key]["hasTrustDialogAccepted"], json!(true));
        assert_eq!(doc["projects"][&key]["hasClaudeMdExternalIncludesApproved"], json!(true));
        assert_eq!(doc["oauthAccount"]["id"], json!("keep"));
        assert_eq!(doc["projects"]["/other"]["x"], json!(1));
    }

    #[test]
    fn a_missing_or_broken_claude_store_is_left_alone() {
        let home = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        pre_trust("/usr/local/bin/claude", work.path(), home.path());
        assert!(!home.path().join(".claude.json").exists());
        std::fs::write(home.path().join(".claude.json"), "{not json").unwrap();
        pre_trust("claude", work.path(), home.path());
        assert_eq!(std::fs::read_to_string(home.path().join(".claude.json")).unwrap(), "{not json");
    }

    #[test]
    fn kimi_trust_file_matches_kimis_naming() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".kimi-code")).unwrap();
        let work = tempfile::tempdir().unwrap();
        pre_trust("kimi", work.path(), home.path());
        let real = work.path().canonicalize().unwrap();
        let hash = hex::encode(Sha256::digest(real.to_string_lossy().as_bytes()));
        let name = format!("wd_{}_{}", real.file_name().unwrap().to_string_lossy(), &hash[..12]);
        let doc: Value = serde_json::from_str(
            &std::fs::read_to_string(home.path().join(".kimi-code/workspace-trust").join(name)).unwrap(),
        )
        .unwrap();
        assert_eq!(doc["root"], json!(real.to_string_lossy()));
    }

    fn codex_home(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".codex")).unwrap();
        let store = home.path().join(".codex/config.toml");
        std::fs::write(&store, body).unwrap();
        (home, store)
    }

    #[test]
    fn codex_trust_is_appended_and_idempotent() {
        let (home, store) = codex_home("# mine\nmodel = \"x\"\n\n[projects.\"/other\"]\ntrust_level = \"trusted\"\n");
        let work = tempfile::tempdir().unwrap();
        pre_trust("codex", work.path(), home.path());
        let text = std::fs::read_to_string(&store).unwrap();
        assert!(text.starts_with("# mine\nmodel = \"x\"\n\n[projects.\"/other\"]"));
        let doc: toml::Value = toml::from_str(&text).unwrap();
        let key = work.path().canonicalize().unwrap().to_string_lossy().to_string();
        assert_eq!(doc["projects"][&key]["trust_level"].as_str(), Some("trusted"));
        assert_eq!(doc["projects"]["/other"]["trust_level"].as_str(), Some("trusted"));
        pre_trust("codex", work.path(), home.path());
        assert_eq!(std::fs::read_to_string(&store).unwrap(), text);
    }

    #[test]
    fn codex_keeps_an_existing_choice() {
        let work = tempfile::tempdir().unwrap();
        let key = work.path().canonicalize().unwrap().to_string_lossy().to_string();
        let body = format!("[projects.\"{key}\"]\ntrust_level = \"untrusted\"\n");
        let (home, store) = codex_home(&body);
        pre_trust("codex", work.path(), home.path());
        let after = std::fs::read_to_string(&store).unwrap();
        assert!(after.starts_with(&body));
        // The given path may differ from the canonical one (/var vs /private/var);
        // only the canonical entry the person chose must stay untouched.
        let doc: toml::Value = toml::from_str(&after).unwrap();
        assert_eq!(doc["projects"][&key]["trust_level"].as_str(), Some("untrusted"));
    }

    #[test]
    fn codex_without_a_config_writes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        pre_trust("codex", work.path(), home.path());
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    }

    #[test]
    fn other_harnesses_and_no_kimi_install_write_nothing() {
        let home = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        pre_trust("grok", work.path(), home.path());
        pre_trust("kimi", work.path(), home.path());
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    }
}
