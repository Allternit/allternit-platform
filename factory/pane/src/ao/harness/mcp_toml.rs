//! TOML-block MCP mechanics — lib.js:274-325 port. All TOML handling is
//! line/regex splicing (no TOML crate): `findTomlBlock` ports the JS
//! `^\[<section>\]\s*$` multiline regex plus "to next `^\[` or EOF", and the
//! block render is the fixed `[<section>]\ncommand = "..."\nargs = <JSON>` text.

use super::json_val::print_compact;
use super::{expand, write_text, Action, McpServer};

pub(crate) struct TomlBlock {
    pub start: usize,
    pub end: usize,
}

/// `findTomlBlock()`. The JS escapes the section name into a literal regex,
/// so the match is a literal `[<section>]` line: line starts with `[`, then
/// the section, then `]`, then only whitespace (a trailing `\r` is consumed
/// by `\s*`, as is any trailing spaces). The block runs to the next line
/// starting with `[` or EOF.
pub(crate) fn find_block(text: &str, section: &str) -> Option<TomlBlock> {
    let want = format!("[{section}]");
    let mut index = 0usize;
    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        if content.starts_with('[') && content.trim_end() == want {
            let match_len = content.len();
            let rest = &text[index + match_len..];
            let next = if rest.starts_with('[') {
                Some(0)
            } else {
                rest.find("\n[").map(|pos| pos + 1)
            };
            let end = next.map(|offset| index + match_len + offset).unwrap_or(text.len());
            return Some(TomlBlock { start: index, end });
        }
        index += line.len();
    }
    None
}

/// `tomlBlockText()` — args rendered as a COMPACT JSON array (JSON.stringify
/// without indent), e.g. `args = ["node", "/opt/ops/index.js"]`.
pub(crate) fn block_text(section: &str, server: &McpServer) -> String {
    let args = super::json_val::JVal::Arr(
        server.args.iter().map(|arg| super::json_val::JVal::Str(arg.clone())).collect(),
    );
    format!("[{section}]\ncommand = \"{}\"\nargs = {}\n", server.command, print_compact(&args))
}

/// `tomlMcpStatus()`: loose substring matching — a `command` line containing
/// `"<command>"` AND an `args` line containing every arg substring.
pub(crate) fn status(file: &str, section: &str, server: &McpServer) -> std::io::Result<&'static str> {
    let f = expand(file);
    if !f.exists() {
        return Ok("missing");
    }
    let text = std::fs::read_to_string(&f)?;
    let Some(block) = find_block(&text, section) else {
        return Ok("missing");
    };
    let block_text = &text[block.start..block.end];
    let command_line = block_text
        .split('\n')
        .find(|line| line.trim().starts_with("command"));
    let args_line = block_text
        .split('\n')
        .find(|line| line.trim().starts_with("args"));
    let command_ok = command_line.is_some_and(|line| line.contains(&format!("\"{}\"", server.command)));
    let args_ok = args_line.is_some_and(|line| server.args.iter().all(|arg| line.contains(arg.as_str())));
    Ok(if command_ok && args_ok { "ok" } else { "broken" })
}

/// `tomlMcpSync()` — lib.js:300-314.
pub(crate) fn sync(
    file: &str,
    section: &str,
    server: &McpServer,
    dry_run: bool,
) -> std::io::Result<Vec<Action>> {
    let state = status(file, section, server)?;
    if state == "ok" {
        return Ok(vec![Action::unchanged()]);
    }
    let kind = if state == "missing" { "add" } else { "fix" };
    let f = expand(file);
    if dry_run {
        return Ok(vec![Action::detailed(format!("would-{kind}"), f.display().to_string())]);
    }
    let old = if f.exists() { std::fs::read_to_string(&f)? } else { String::new() };
    let block = find_block(&old, section);
    let block_text = block_text(section, server);
    let out = match block {
        Some(found) => {
            // Replace the block slice; the tail loses ONE leading newline
            // (.replace(/^\n/, "")) — parity quirk, reproduced.
            let tail = &old[found.end..];
            let tail = tail.strip_prefix('\n').unwrap_or(tail);
            format!("{}{}{}", &old[..found.start], block_text, tail)
        }
        // Append: strip ALL trailing whitespace, add exactly two newlines
        // (old.replace(/\s*$/, "\n\n") + blockText).
        None => format!("{}\n\n{block_text}", old.trim_end()),
    };
    write_text(&f.to_string_lossy(), &out)?;
    // Non-dry sync returns NO detail (lib.js:313) — describe prints "add"/"fix".
    Ok(vec![Action::new(kind)])
}

/// `tomlMcpRemove()` — lib.js:316-325.
pub(crate) fn remove(file: &str, section: &str, dry_run: bool) -> std::io::Result<Vec<Action>> {
    let f = expand(file);
    if !f.exists() {
        return Ok(vec![Action::nothing("no file")]);
    }
    let old = std::fs::read_to_string(&f)?;
    let Some(block) = find_block(&old, section) else {
        return Ok(vec![Action::nothing("no entry")]);
    };
    if dry_run {
        return Ok(vec![Action::detailed("would-remove", section)]);
    }
    let spliced = format!("{}{}", &old[..block.start], &old[block.end..]);
    // .replace(/\n{3,}/g, "\n\n") — collapse runs of 3+ newlines to exactly 2.
    let mut collapsed = spliced;
    while collapsed.contains("\n\n\n") {
        collapsed = collapsed.replace("\n\n\n", "\n\n");
    }
    write_text(&f.to_string_lossy(), &collapsed)?;
    Ok(vec![Action::detailed("remove", section)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> McpServer {
        McpServer {
            name: "allternit-ops".to_string(),
            command: "node".to_string(),
            args: vec!["/opt/ops/index.js".to_string()],
        }
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ao-harness-mcptoml-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn find_block_boundaries() {
        let text = "top = 1\n\n[mcp_servers.allternit-ops]\ncommand = \"node\"\nargs = [\"x\"]\n\n[mcp_servers.other]\ncommand = \"y\"\n";
        let block = find_block(text, "mcp_servers.allternit-ops").unwrap();
        assert_eq!(&text[block.start..block.end], "[mcp_servers.allternit-ops]\ncommand = \"node\"\nargs = [\"x\"]\n\n");
        // Dotted section names are literal (the JS regex escapes the dots).
        assert!(find_block(text, "mcp_servers.allternit-ops").is_some());
        assert!(find_block(text, "mcp_serversXallternit-ops").is_none());
        // Trailing spaces/\r on the section line still match (\s*$).
        let text2 = "[sec]  \r\ncommand = \"node\"\r\n";
        assert!(find_block(text2, "sec").is_some());
        // Leading space does not match (^ anchor).
        assert!(find_block(" [sec]\n", "sec").is_none());
        // Last block runs to EOF.
        let text3 = "[a]\nx = 1\n[b]\ny = 2\n";
        let block = find_block(text3, "b").unwrap();
        assert_eq!(&text3[block.start..block.end], "[b]\ny = 2\n");
    }

    #[test]
    fn status_loose_substring_matching() {
        let dir = tmp("status");
        let file = dir.join("config.toml");
        let file_str = file.to_string_lossy().into_owned();
        assert_eq!(status(&file_str, "mcp_servers.allternit-ops", &server()).unwrap(), "missing");
        std::fs::write(
            &file,
            "[mcp_servers.allternit-ops]\ncommand = \"node\"\nargs = [\"/opt/ops/index.js\"]\nenabled = true\n",
        )
        .unwrap();
        assert_eq!(status(&file_str, "mcp_servers.allternit-ops", &server()).unwrap(), "ok");
        std::fs::write(&file, "[mcp_servers.allternit-ops]\ncommand = \"python\"\n").unwrap();
        assert_eq!(status(&file_str, "mcp_servers.allternit-ops", &server()).unwrap(), "broken");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sync_append_fix_remove_cycle() {
        let dir = tmp("cycle");
        let file = dir.join("config.toml");
        let file_str = file.to_string_lossy().into_owned();
        let section = "mcp_servers.allternit-ops";
        let server = server();

        std::fs::write(&file, "model = \"gpt\"\n").unwrap();
        let actions = sync(&file_str, section, &server, false).unwrap();
        assert_eq!(actions[0].kind, "add");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "model = \"gpt\"\n\n[mcp_servers.allternit-ops]\ncommand = \"node\"\nargs = [\"/opt/ops/index.js\"]\n"
        );
        assert_eq!(actions[0].detail, None, "non-dry toml sync carries no detail");

        // Second sync → unchanged.
        let actions = sync(&file_str, section, &server, false).unwrap();
        assert_eq!(actions[0].kind, "unchanged");

        // Tamper → fix; tail keeps ONE leading newline stripped after the block.
        let tampered = "model = \"gpt\"\n\n[mcp_servers.allternit-ops]\ncommand = \"python\"\nargs = [\"old\"]\n\n[other]\nx = 1\n";
        std::fs::write(&file, tampered).unwrap();
        let actions = sync(&file_str, section, &server, false).unwrap();
        assert_eq!(actions[0].kind, "fix");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "model = \"gpt\"\n\n[mcp_servers.allternit-ops]\ncommand = \"node\"\nargs = [\"/opt/ops/index.js\"]\n[other]\nx = 1\n"
        );

        // Remove splices the block and collapses newlines.
        let actions = remove(&file_str, section, false).unwrap();
        assert_eq!(actions[0].kind, "remove");
        assert_eq!(actions[0].detail.as_deref(), Some(section));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "model = \"gpt\"\n\n[other]\nx = 1\n"
        );

        let actions = remove(&file_str, section, false).unwrap();
        assert_eq!(actions[0].kind, "nothing");
        assert_eq!(actions[0].detail.as_deref(), Some("no entry"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_file_append_gets_leading_blank_lines() {
        let dir = tmp("empty");
        let file = dir.join("config.toml");
        let file_str = file.to_string_lossy().into_owned();
        std::fs::write(&file, "").unwrap();
        sync(&file_str, "sec", &server(), false).unwrap();
        // ''.replace(/\s*$/, "\n\n") + block → two leading newlines, parity.
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "\n\n[sec]\ncommand = \"node\"\nargs = [\"/opt/ops/index.js\"]\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
