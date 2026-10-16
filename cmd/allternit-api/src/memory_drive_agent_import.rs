//! Import what other agents on this computer already know about the user:
//! Claude Code, Codex, Hermes and OpenClaw keep their memory and standing
//! instructions as files in the home folder. We read only the fixed paths
//! below, never follow a path the caller sends, and run every candidate
//! through the same checks and preview as a pasted import.
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::db::DbHandle;
use crate::memory_drive_service::{Result, ServiceError};
use crate::memory_drive_text_import::{self as text_import, Candidate, Plan};

/// Files larger than this are skipped: memory files are small, and a huge
/// file here is not one.
const MAX_FILE_BYTES: u64 = 512 * 1024;
/// Upper bound on files read for one agent.
const MAX_FILES: usize = 2000;

struct Agent {
    id: &'static str,
    name: &'static str,
    path: &'static str,
}

const AGENTS: &[Agent] = &[
    Agent { id: "claude-code", name: "Claude Code", path: "imports/claude-code.md" },
    Agent { id: "codex", name: "Codex", path: "imports/codex.md" },
    Agent { id: "hermes", name: "Hermes", path: "imports/hermes.md" },
    Agent { id: "openclaw", name: "OpenClaw", path: "imports/openclaw.md" },
];

fn agent(id: &str) -> Result<&'static Agent> {
    AGENTS
        .iter()
        .find(|a| a.id == id)
        .ok_or_else(|| ServiceError::Provenance("source must be claude-code, codex, hermes or openclaw".into()))
}

/// The home folder to read: `ALLTERNIT_AGENT_IMPORT_HOME` (tests), else the
/// user's home.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("ALLTERNIT_AGENT_IMPORT_HOME").map(PathBuf::from).or_else(dirs::home_dir)
}

#[derive(Debug, Clone, Serialize)]
pub struct Found {
    pub id: &'static str,
    pub name: &'static str,
    /// Memory and instruction files found.
    pub files: usize,
    /// Candidate memories in them, before checks.
    pub items: usize,
}

/// Agents with memory on this computer, and how much.
pub fn detect(home: &Path) -> Vec<Found> {
    AGENTS
        .iter()
        .filter_map(|a| {
            let files = files_for(home, a.id);
            if files.is_empty() {
                return None;
            }
            let items = files.iter().map(|f| read_items(f).len()).sum();
            Some(Found { id: a.id, name: a.name, files: files.len(), items })
        })
        .collect()
}

/// One file to read, how to read it, and the provenance label for its items.
struct MemFile {
    path: PathBuf,
    kind: Kind,
    source: String,
}

#[derive(Clone, Copy)]
enum Kind {
    /// Markdown instructions or notes: one item per bullet or paragraph.
    Markdown,
    /// A Claude Code auto-memory file: one item, its `description`.
    ClaudeMemory,
    /// Hermes memory: entries separated by `§`.
    Hermes,
}

fn small_file(p: &Path) -> bool {
    std::fs::symlink_metadata(p).map(|m| m.is_file() && m.len() <= MAX_FILE_BYTES).unwrap_or(false)
}

fn modified(p: &Path) -> std::time::SystemTime {
    std::fs::metadata(p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH)
}

/// Directories directly inside `dir`, symlinks excluded.
fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut out: Vec<_> = rd
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

/// `*.md` files directly inside `dir`, newest first.
fn md_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut out: Vec<_> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md") && small_file(p))
        .collect();
    out.sort_by_key(|p| std::cmp::Reverse(modified(p)));
    out
}

use text_import::provenance_label as label;

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn files_for(home: &Path, id: &str) -> Vec<MemFile> {
    let mut out = Vec::new();
    let mut push = |path: PathBuf, kind: Kind, source: String| {
        if out.len() < MAX_FILES && small_file(&path) {
            out.push(MemFile { path, kind, source });
        }
    };
    match id {
        "claude-code" => {
            let root = home.join(".claude");
            push(root.join("CLAUDE.md"), Kind::Markdown, "claude-code:CLAUDE.md".into());
            for project in subdirs(&root.join("projects")) {
                let dir = project.join("memory");
                for f in md_files(&dir) {
                    if name_of(&f) == "MEMORY.md" {
                        continue; // the index only points at the other files
                    }
                    let source = label("claude-code", &format!("memory/{}/{}", name_of(&project), name_of(&f)));
                    push(f, Kind::ClaudeMemory, source);
                }
            }
        }
        "codex" => {
            let root = home.join(".codex");
            push(root.join("AGENTS.md"), Kind::Markdown, "codex:AGENTS.md".into());
            let mem = root.join("memories");
            for f in md_files(&mem).into_iter().chain(subdirs(&mem).iter().flat_map(|d| md_files(d))) {
                let rel = f.strip_prefix(&mem).map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|_| name_of(&f));
                push(f, Kind::Markdown, label("codex", &format!("memories/{rel}")));
            }
        }
        "hermes" => {
            let root = home.join(".hermes");
            let mut dirs = vec![(root.join("memories"), String::new())];
            for p in subdirs(&root.join("profiles")) {
                dirs.push((p.join("memories"), format!("profiles/{}/", name_of(&p))));
            }
            for (dir, prefix) in dirs {
                for file in ["USER.md", "MEMORY.md"] {
                    push(dir.join(file), Kind::Hermes, label("hermes", &format!("{prefix}memories/{file}")));
                }
            }
        }
        "openclaw" => {
            let ws = home.join(".openclaw").join("workspace");
            for file in ["USER.md", "MEMORY.md"] {
                push(ws.join(file), Kind::Markdown, label("openclaw", &format!("workspace/{file}")));
            }
        }
        _ => {}
    }
    out
}

fn strip_frontmatter(text: &str) -> (&str, Option<&str>) {
    let t = text.trim_start_matches('\u{feff}');
    if let Some(rest) = t.strip_prefix("---\n").or_else(|| t.strip_prefix("---\r\n")) {
        if let Some(end) = rest.find("\n---") {
            let after = &rest[end + 4..];
            let body = after.find('\n').map_or("", |i| &after[i + 1..]);
            return (body, Some(&rest[..end]));
        }
    }
    (t, None)
}

/// Clean one markdown line or paragraph into plain text.
fn plain(s: &str) -> String {
    let s = s.replace("**", "").replace("__", "").replace('`', "");
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Items in a markdown file: each bullet (with its wrapped lines) and each
/// paragraph. Headings, tables, code blocks, quotes markers and HTML
/// comments are left out.
pub fn markdown_items(text: &str) -> Vec<String> {
    static BULLET: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let bullet = BULLET.get_or_init(|| regex::Regex::new(r"^\s*(?:[-*+•]|\d{1,4}[.)])\s+(.*)$").expect("constant regex"));
    let (body, _) = strip_frontmatter(text);
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut fence = false;
    let mut comment = false;
    let flush = |cur: &mut String, items: &mut Vec<String>| {
        let p = plain(cur);
        if !p.is_empty() {
            items.push(p);
        }
        cur.clear();
    };
    for line in body.lines() {
        let t = line.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            flush(&mut cur, &mut items);
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        if comment || t.starts_with("<!--") {
            comment = !t.contains("-->");
            flush(&mut cur, &mut items);
            continue;
        }
        if t.is_empty() || t.starts_with('#') || t.starts_with('|') || t.chars().all(|c| matches!(c, '-' | '=' | '*' | '_')) {
            flush(&mut cur, &mut items);
            continue;
        }
        let t = t.trim_start_matches('>').trim();
        if let Some(c) = bullet.captures(line) {
            flush(&mut cur, &mut items);
            cur.push_str(&c[1]);
        } else {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(t);
        }
    }
    flush(&mut cur, &mut items);
    items
}

/// The one memory in a Claude Code auto-memory file: its `description`, else
/// the first item of its body.
fn claude_memory_item(text: &str) -> Option<String> {
    let (body, front) = strip_frontmatter(text);
    let desc = front.and_then(|f| {
        f.lines().find_map(|l| l.strip_prefix("description:")).map(|d| d.trim().trim_matches(['"', '\'']).to_string())
    });
    desc.filter(|d| !d.is_empty()).or_else(|| markdown_items(body).into_iter().next())
}

fn hermes_items(text: &str) -> Vec<String> {
    text.split('§').map(plain).filter(|s| !s.is_empty()).collect()
}

fn read_items(f: &MemFile) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(&f.path) else { return vec![] };
    match f.kind {
        Kind::Markdown => markdown_items(&text),
        Kind::ClaudeMemory => claude_memory_item(&text).into_iter().collect(),
        Kind::Hermes => hermes_items(&text),
    }
}

fn candidates(home: &Path, id: &str) -> Vec<Candidate> {
    files_for(home, id)
        .iter()
        .flat_map(|f| read_items(f).into_iter().map(|text| Candidate { text, source: f.source.clone() }))
        .collect()
}

pub fn plan(db: &DbHandle, owner: &str, home: &Path, id: &str) -> Result<Plan> {
    let a = agent(id)?;
    let c = text_import::check(db, owner, candidates(home, id))?;
    Ok(Plan {
        from: id.to_string(),
        path: a.path.to_string(),
        entries: c.entries.into_iter().map(|e| e.text).collect(),
        skipped: c.skipped,
        remaining: c.remaining,
    })
}

pub fn apply(db: &DbHandle, owner: &str, home: &Path, id: &str, selected: Option<&[String]>) -> Result<(Option<String>, usize)> {
    let a = agent(id)?;
    let c = text_import::check(db, owner, candidates(home, id))?;
    text_import::commit(db, owner, c.entries, selected, a.path, &format!("Import memory from {}", a.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn markdown_items_keeps_bullets_and_paragraphs_only() {
        let md = "---\nname: x\n---\n# Rules\n\n- Prefers short answers\n  that get to the point.\n- Uses **Rust** and `TypeScript`.\n\nA wrapped\nparagraph.\n\n```sh\nrm -rf /\n```\n| a | b |\n<!-- note -->\n> Quoted line\n";
        assert_eq!(
            markdown_items(md),
            vec!["Prefers short answers that get to the point.", "Uses Rust and TypeScript.", "A wrapped paragraph.", "Quoted line"]
        );
    }

    #[test]
    fn finds_each_agent_and_labels_every_item() {
        let tmp = tempfile::tempdir().unwrap();
        let h = tmp.path();
        write(&h.join(".claude/CLAUDE.md"), "# Me\n- My name is Eoj\n");
        write(&h.join(".claude/projects/-Users-eoj/memory/MEMORY.md"), "- [x](x.md) — index\n");
        write(&h.join(".claude/projects/-Users-eoj/memory/tea.md"), "---\nname: tea\ndescription: \"Eoj drinks green tea\"\n---\nbody\n");
        write(&h.join(".codex/AGENTS.md"), "- Run tests before committing\n");
        write(&h.join(".hermes/memories/USER.md"), "Lives in Saint Paul§Has a dog named Rex");
        write(&h.join(".hermes/profiles/work/memories/MEMORY.md"), "Works at Allternit");
        write(&h.join(".openclaw/workspace/MEMORY.md"), "- Likes jazz\n");

        let found = detect(h);
        let ids: Vec<_> = found.iter().map(|f| f.id).collect();
        assert_eq!(ids, vec!["claude-code", "codex", "hermes", "openclaw"]);

        let cc = candidates(h, "claude-code");
        assert_eq!(cc.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), vec!["My name is Eoj", "Eoj drinks green tea"]);
        assert_eq!(cc[1].source, "claude-code:memory/-Users-eoj/tea.md");
        let hermes = candidates(h, "hermes");
        assert_eq!(hermes.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), vec!["Lives in Saint Paul", "Has a dog named Rex", "Works at Allternit"]);
        assert_eq!(hermes[2].source, "hermes:profiles/work/memories/MEMORY.md");
        for id in ["claude-code", "codex", "hermes", "openclaw"] {
            for c in candidates(h, id) {
                crate::memory_drive::validate_source(&c.source).unwrap_or_else(|e| panic!("{}: {e:?}", c.source));
            }
        }
    }

    #[test]
    fn nothing_found_on_an_empty_home() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(detect(tmp.path()).is_empty());
        assert!(agent("cursor").is_err());
    }

    #[test]
    fn labels_strip_unsafe_characters() {
        assert_eq!(label("claude-code", "memory/My Project [1]/a b.md"), "claude-code:memory/My-Project--1-/a-b.md");
    }
}
