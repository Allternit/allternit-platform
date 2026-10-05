//! JSON-file MCP mechanics — lib.js:327-375 port. Uses the order-preserving
//! [`JVal`] (see json_val.rs) because the sync path is a full-file
//! `JSON.parse → mutate → JSON.stringify(obj, null, 2)` rewrite that must
//! preserve existing key order exactly like the JS.

use super::json_val::JVal;
use super::{expand, read_json, write_json, Action, McpCfg, McpServer};

/// `mcpEntry()`: the config entry shape. `commandArray` tools (gizzi,
/// opencode) get `{type: "local", command: [cmd, ...args]}`; everyone else
/// gets `{command, args}` plus `type` when `entryType` is set (qoder).
pub(crate) fn mcp_entry(cfg: &McpCfg, server: &McpServer) -> JVal {
    if cfg.command_array == Some(true) {
        let mut command = vec![JVal::Str(server.command.clone())];
        command.extend(server.args.iter().map(|arg| JVal::Str(arg.clone())));
        JVal::Obj(vec![
            ("type".to_string(), JVal::str("local")),
            ("command".to_string(), JVal::Arr(command)),
        ])
    } else {
        let mut fields = vec![
            ("command".to_string(), JVal::Str(server.command.clone())),
            (
                "args".to_string(),
                JVal::Arr(server.args.iter().map(|arg| JVal::Str(arg.clone())).collect()),
            ),
        ];
        if let Some(entry_type) = &cfg.entry_type {
            fields.push(("type".to_string(), JVal::Str(entry_type.clone())));
        }
        JVal::Obj(fields)
    }
}

/// `arrEqual()`: both must be arrays, element-wise `===`. (String elements
/// compare by value like `===`; exotic numeric element forms may differ —
/// manifest args are all strings.)
fn array_equal(a: Option<&JVal>, b: Option<&JVal>) -> bool {
    match (a, b) {
        (Some(JVal::Arr(x)), Some(JVal::Arr(y))) => x == y,
        _ => false,
    }
}

/// `mcpEntryMatches()`. NOTE: `type` is compared ONLY for commandArray
/// entries — qoder's `entryType: stdio` is ignored by the match test, exactly
/// like the JS.
fn entry_matches(cfg: &McpCfg, cur: &JVal, server: &McpServer) -> bool {
    if cfg.command_array == Some(true) {
        let expected = mcp_entry(cfg, server);
        cur.get("type") == Some(&JVal::str("local")) && array_equal(cur.get("command"), expected.get("command"))
    } else {
        cur.get("command") == Some(&JVal::Str(server.command.clone()))
            && array_equal(cur.get("args"), Some(&JVal::Arr(
                server.args.iter().map(|arg| JVal::Str(arg.clone())).collect(),
            )))
    }
}

/// JS `typeof obj === "object"` — objects AND arrays pass (arrays then fail
/// the `obj[serversKey]` lookup). Scalars return 'missing' regardless of
/// truthiness at the top level.
fn is_js_object(value: &JVal) -> bool {
    matches!(value, JVal::Obj(_) | JVal::Arr(_))
}

/// `jsonMcpStatus()`.
pub(crate) fn status(file: &str, cfg: &McpCfg, name: &str, server: &McpServer) -> std::io::Result<&'static str> {
    let Some(obj) = read_json(file) else {
        return Ok("missing");
    };
    if !is_js_object(&obj) {
        return Ok("missing");
    }
    let Some(cur) = obj
        .get(cfg.servers_key.as_deref().unwrap_or_default())
        .and_then(|servers| servers.get(name))
    else {
        return Ok("missing");
    };
    if !cur.js_truthy() {
        return Ok("missing");
    }
    Ok(if entry_matches(cfg, cur, server) { "ok" } else { "broken" })
}

/// `jsonMcpSync()` — full-file pretty rewrite. The JS primitive-assignment
/// semantics are reproduced: a non-object root ignores the assignment (the
/// file is still normalized), a falsy `serversKey` value is replaced with a
/// fresh object, and a truthy non-object `serversKey` value makes the entry
/// set a silent no-op.
pub(crate) fn sync(
    file: &str,
    cfg: &McpCfg,
    name: &str,
    server: &McpServer,
    dry_run: bool,
) -> std::io::Result<Vec<Action>> {
    let state = status(file, cfg, name, server)?;
    if state == "ok" {
        return Ok(vec![Action::unchanged()]);
    }
    let kind = if state == "missing" { "add" } else { "fix" };
    let expanded = expand(file);
    if dry_run {
        return Ok(vec![Action::detailed(format!("would-{kind}"), expanded.display().to_string())]);
    }
    let mut obj = read_json(file).unwrap_or_else(|| JVal::Obj(vec![]));
    let servers_key = cfg.servers_key.as_deref().unwrap_or_default();
    if let JVal::Obj(_) = &obj {
        let replace = match obj.get(servers_key) {
            Some(value) => !value.js_truthy(),
            None => true,
        };
        if replace {
            obj.set(servers_key, JVal::Obj(vec![]));
        }
        if let Some(JVal::Obj(_)) = obj.get(servers_key) {
            let entry = mcp_entry(cfg, server);
            if let Some(servers) = obj.get(servers_key) {
                let mut servers = servers.clone();
                servers.set(name, entry);
                obj.set(servers_key, servers);
            }
        }
    }
    write_json(file, &obj)?;
    Ok(vec![Action::detailed(kind, expanded.display().to_string())])
}

/// `jsonMcpRemove()`.
pub(crate) fn remove(file: &str, cfg: &McpCfg, name: &str, dry_run: bool) -> std::io::Result<Vec<Action>> {
    let Some(mut obj) = read_json(file) else {
        return Ok(vec![Action::nothing("no entry")]);
    };
    let servers_key = cfg.servers_key.as_deref().unwrap_or_default();
    let present = obj
        .get(servers_key)
        .filter(|value| value.js_truthy())
        .and_then(|servers| servers.get(name))
        .map(|entry| entry.js_truthy())
        .unwrap_or(false);
    if !present {
        return Ok(vec![Action::nothing("no entry")]);
    }
    if dry_run {
        return Ok(vec![Action::detailed("would-remove", name)]);
    }
    if let Some(JVal::Obj(_)) = obj.get(servers_key) {
        if let Some(servers) = obj.get(servers_key) {
            let mut servers = servers.clone();
            servers.remove(name);
            obj.set(servers_key, servers);
        }
    }
    write_json(file, &obj)?;
    Ok(vec![Action::detailed("remove", name)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_cfg(servers_key: &str, command_array: bool, entry_type: Option<&str>) -> McpCfg {
        McpCfg {
            kind: "json".to_string(),
            path: None,
            servers_key: Some(servers_key.to_string()),
            section: None,
            bin: None,
            add_args: None,
            remove_args: None,
            config_path: None,
            config_format: None,
            command_array: Some(command_array),
            entry_type: entry_type.map(str::to_string),
        }
    }

    fn server() -> McpServer {
        McpServer {
            name: "allternit-ops".to_string(),
            command: "node".to_string(),
            args: vec!["/opt/ops/index.js".to_string()],
        }
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ao-harness-mcpjson-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn add_fix_remove_cycle_preserves_unrelated_key_order() {
        let dir = tmp("cycle");
        let file = dir.join("settings.json");
        // Unsorted keys + non-2-space formatting: the sync normalizes the
        // whole file but must keep key ORDER (JSON.parse semantics).
        std::fs::write(&file, "{\"zebra\": 1, \"mcpServers\": {}, \"apple\": true}").unwrap();
        let file_str = file.to_string_lossy().into_owned();
        let cfg = json_cfg("mcpServers", false, None);
        let server = server();

        let actions = sync(&file_str, &cfg, "allternit-ops", &server, false).unwrap();
        assert_eq!(actions[0].kind, "add");
        let written = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            written,
            "{\n  \"zebra\": 1,\n  \"mcpServers\": {\n    \"allternit-ops\": {\n      \"command\": \"node\",\n      \"args\": [\n        \"/opt/ops/index.js\"\n      ]\n    }\n  },\n  \"apple\": true\n}\n"
        );

        let actions = sync(&file_str, &cfg, "allternit-ops", &server, false).unwrap();
        assert_eq!(actions[0].kind, "unchanged");

        // Stale entry → fix; broken entry detected by status.
        assert_eq!(status(&file_str, &cfg, "allternit-ops", &server).unwrap(), "ok");
        let stale = server_with_command("python");
        assert_eq!(status(&file_str, &cfg, "allternit-ops", &stale).unwrap(), "broken");

        let actions = remove(&file_str, &cfg, "allternit-ops", false).unwrap();
        assert_eq!(actions[0].kind, "remove");
        let written = std::fs::read_to_string(&file).unwrap();
        assert_eq!(written, "{\n  \"zebra\": 1,\n  \"mcpServers\": {},\n  \"apple\": true\n}\n");
        // Backup side effect through writeJson/writeText.
        assert!(file.with_file_name("settings.json.bak-harness").exists());

        let actions = remove(&file_str, &cfg, "allternit-ops", false).unwrap();
        assert_eq!(actions[0].kind, "nothing");
        assert_eq!(actions[0].detail.as_deref(), Some("no entry"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn server_with_command(command: &str) -> McpServer {
        McpServer { command: command.to_string(), ..server() }
    }

    #[test]
    fn command_array_entry_shape_and_match() {
        let dir = tmp("cmdarray");
        let file = dir.join("gizzi.json");
        let file_str = file.to_string_lossy().into_owned();
        let cfg = json_cfg("mcp", true, None);
        let server = server();
        sync(&file_str, &cfg, "allternit-ops", &server, false).unwrap();
        let written = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            written,
            "{\n  \"mcp\": {\n    \"allternit-ops\": {\n      \"type\": \"local\",\n      \"command\": [\n        \"node\",\n        \"/opt/ops/index.js\"\n      ]\n    }\n  }\n}\n"
        );
        assert_eq!(status(&file_str, &cfg, "allternit-ops", &server).unwrap(), "ok");
    }

    #[test]
    fn entry_type_is_written_but_not_matched() {
        let dir = tmp("entrytype");
        let file = dir.join("settings.json");
        let file_str = file.to_string_lossy().into_owned();
        let cfg = json_cfg("mcpServers", false, Some("stdio"));
        let server = server();
        sync(&file_str, &cfg, "allternit-ops", &server, false).unwrap();
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(written.contains("\"type\": \"stdio\""));
        // The JS match test ignores `type` for non-commandArray entries: an
        // entry WITHOUT type still reads as ok.
        std::fs::write(
            &file,
            "{\"mcpServers\": {\"allternit-ops\": {\"command\": \"node\", \"args\": [\"/opt/ops/index.js\"]}}}",
        )
        .unwrap();
        assert_eq!(status(&file_str, &cfg, "allternit-ops", &server).unwrap(), "ok");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strict_json_comments_mean_missing() {
        let dir = tmp("jsonc");
        let file = dir.join("opencode.jsonc");
        std::fs::write(&file, "{\n  // comment\n  \"mcp\": {}\n}\n").unwrap();
        let file_str = file.to_string_lossy().into_owned();
        let cfg = json_cfg("mcp", true, None);
        assert_eq!(status(&file_str, &cfg, "allternit-ops", &server()).unwrap(), "missing");
        // Sync rewrites the unparseable file from scratch (JS: readJson →
        // null → {}), destroying the comments — the fragility is parity.
        sync(&file_str, &cfg, "allternit-ops", &server(), false).unwrap();
        let written = std::fs::read_to_string(&file).unwrap();
        assert!(!written.contains("//"));
        assert!(written.contains("\"allternit-ops\""));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
