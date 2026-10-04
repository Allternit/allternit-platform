//! `allternit-bot`: act through your Allternit bot's phone and mailbox from a vendor agent's shell.
//!
//! The same ten tools the MCP connector offers (`cmd/allternit-api/src/mcp_vendor_bots.rs`), one
//! command each. Every call is a JSON-RPC `tools/call` to `<url>/bots/<bot id>` with the key as a
//! Bearer token, so every gate (consent, STOP, owner approval of email) is applied on the server.
//! Nothing here can widen what the key allows.
//!
//! Config, first found wins: `--bot`/`--url` flags, `ALLTERNIT_BOT_KEY` / `ALLTERNIT_BOT_ID` /
//! `ALLTERNIT_BOT_URL`, then `~/.config/allternit-bot/config.json` (written by `login`, mode 0600).
//! The key is never taken from argv, where `ps` could show it.
//!
//! Exit codes: 0 done, 1 Allternit refused (the reason is printed), 2 couldn't reach it or bad usage.

use std::io::{Read, Write};
use std::path::PathBuf;

use serde_json::{json, Value};

/// What an agent reads to learn the tools; the file the cloud edge serves too.
const HELP: &str = include_str!("../../allternit-api/assets/vendor-bot-instructions.txt");
const DEFAULT_URL: &str = "https://mcp.allternit.com/mcp";

#[derive(Debug, PartialEq)]
enum Command {
    Help,
    Version,
    Login { bot: String },
    Tool { name: &'static str, args: Value },
}

#[derive(Debug, PartialEq, Default)]
struct Flags {
    json: bool,
    bot: Option<String>,
    url: Option<String>,
}

#[derive(Debug, PartialEq)]
struct Usage(String);

fn usage<T>(m: impl Into<String>) -> Result<T, Usage> {
    Err(Usage(m.into()))
}

/// Pulls `--flag value` pairs and `--json` out of `args`, returning the positionals.
fn split_flags(args: &[String], value_flags: &[&str]) -> Result<(Vec<String>, Flags, Vec<(String, String)>), Usage> {
    let (mut pos, mut flags, mut named) = (Vec::new(), Flags::default(), Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => flags.json = true,
            "--bot" | "--url" => {
                let v = it.next().ok_or_else(|| Usage(format!("{a} needs a value.")))?.clone();
                if a == "--bot" { flags.bot = Some(v) } else { flags.url = Some(v) }
            }
            f if value_flags.contains(&f) => {
                let v = it.next().ok_or_else(|| Usage(format!("{f} needs a value.")))?.clone();
                named.push((f.trim_start_matches('-').to_string(), v));
            }
            f if f.starts_with("--") => return usage(format!("Unknown option {f}. Try: allternit-bot help")),
            _ => pos.push(a.clone()),
        }
    }
    Ok((pos, flags, named))
}

/// A text argument: the words joined, or all of stdin when it is `-`.
fn words(rest: &[String], stdin: &mut dyn Read) -> Result<String, Usage> {
    let joined = rest.join(" ");
    let text = if joined == "-" {
        let mut s = String::new();
        stdin.read_to_string(&mut s).map_err(|e| Usage(format!("Couldn't read stdin: {e}")))?;
        s.trim_end().to_string()
    } else {
        joined
    };
    if text.trim().is_empty() { usage("The text is empty.") } else { Ok(text) }
}

fn parse(argv: &[String], stdin: &mut dyn Read) -> Result<(Command, Flags), Usage> {
    let Some(cmd) = argv.first().map(String::as_str) else { return Ok((Command::Help, Flags::default())) };
    let rest = &argv[1..];
    let named_flags: &[&str] = match cmd {
        "threads" => &["--channel", "--query"],
        "read" => &["--limit"],
        "result" => &["--data"],
        _ => &[],
    };
    let (pos, flags, named) = split_flags(rest, named_flags)?;
    let named = |k: &str| named.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let need = |i: usize, what: &str| pos.get(i).cloned().ok_or_else(|| Usage(format!("Missing {what}. Try: allternit-bot help")));
    let command = match cmd {
        "help" | "--help" | "-h" => Command::Help,
        "version" | "--version" | "-V" => Command::Version,
        "login" => Command::Login { bot: need(0, "the bot id")? },
        "threads" => {
            let mut args = json!({});
            if let Some(c) = named("channel") { args["channel"] = json!(c) }
            if let Some(q) = named("query") { args["query"] = json!(q) }
            Command::Tool { name: "list_threads", args }
        }
        "read" => {
            let mut args = json!({ "threadId": need(0, "the thread id")? });
            if let Some(l) = named("limit") {
                args["limit"] = json!(l.parse::<i64>().map_err(|_| Usage("--limit must be a number.".into()))?);
            }
            Command::Tool { name: "read_thread", args }
        }
        "text" => Command::Tool { name: "send_text", args: json!({ "to": need(0, "the number")?, "text": words(&pos[1..], stdin)? }) },
        "call" => Command::Tool { name: "start_call", args: json!({ "to": need(0, "the number")?, "purpose": words(&pos[1..], stdin)? }) },
        "email" => Command::Tool {
            name: "send_email",
            args: json!({ "to": need(0, "the address")?, "subject": need(1, "the subject")?, "body": words(pos.get(2..).unwrap_or(&[]), stdin)? }),
        },
        "post" => {
            let target: Value = serde_json::from_str(&need(1, "the target (JSON)")?).map_err(|_| Usage("The target must be JSON, like '{\"kind\":\"user\",\"id\":\"U123\"}'.".into()))?;
            if !target.is_object() {
                return usage("The target must be a JSON object.");
            }
            Command::Tool { name: "post_message", args: json!({ "provider": need(0, "the provider")?, "target": target, "text": words(pos.get(2..).unwrap_or(&[]), stdin)? }) }
        }
        "ask" => Command::Tool { name: "ask_bot", args: json!({ "text": words(&pos, stdin)? }) },
        "tickets" => Command::Tool { name: "list_open_tickets", args: json!({}) },
        "ticket" => Command::Tool { name: "get_ticket", args: json!({ "id": need(0, "the ticket id")? }) },
        "result" => {
            let mut args = json!({ "id": need(0, "the ticket id")?, "summary": words(&pos[1..], stdin)? });
            if let Some(d) = named("data") {
                args["data"] = serde_json::from_str(&d).ok().filter(Value::is_object).ok_or_else(|| Usage("--data must be a JSON object.".into()))?;
            }
            Command::Tool { name: "post_result", args }
        }
        other => return usage(format!("Unknown command {other}. Try: allternit-bot help")),
    };
    Ok((command, flags))
}

// ─── Config ────────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
struct Config {
    key: String,
    bot: String,
    url: String,
}

fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))).unwrap_or_default();
    base.join("allternit-bot/config.json")
}

fn resolve_config(flags: &Flags, env: &dyn Fn(&str) -> Option<String>, file: Option<Value>) -> Result<Config, Usage> {
    let file = file.unwrap_or(Value::Null);
    let pick = |env_key: &str, file_key: &str| env(env_key).filter(|v| !v.trim().is_empty()).or_else(|| file[file_key].as_str().map(str::to_string));
    let key = pick("ALLTERNIT_BOT_KEY", "key").ok_or_else(|| Usage("No key. Ask the owner for one on the bot's connector page, then run: ALLTERNIT_BOT_KEY=abk_... allternit-bot login <bot id>".into()))?;
    let bot = flags.bot.clone().or_else(|| pick("ALLTERNIT_BOT_ID", "bot")).ok_or_else(|| Usage("No bot id. Set ALLTERNIT_BOT_ID or pass --bot <id>.".into()))?;
    let url = flags.url.clone().or_else(|| pick("ALLTERNIT_BOT_URL", "url")).unwrap_or_else(|| DEFAULT_URL.to_string());
    Ok(Config { key: key.trim().to_string(), bot: bot.trim().to_string(), url: url.trim().trim_end_matches('/').to_string() })
}

// ─── Transport ─────────────────────────────────────────────────────────────────

/// How a call ends when it did not produce a tool result.
#[derive(Debug, PartialEq)]
enum Failure {
    /// Allternit answered and said no (exit 1).
    Refused(String),
    /// Couldn't reach it, or it is unwell (exit 2).
    Unreachable(String),
}

trait Transport {
    /// POST `body` as JSON to `url` with the Bearer `key`; `(status, body)`.
    fn post(&self, url: &str, key: &str, body: &Value) -> Result<(u16, String), String>;
}

struct Http(reqwest::blocking::Client);

impl Http {
    fn new() -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(70))
            .user_agent(concat!("allternit-bot/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("an http client");
        Self(client)
    }
}

impl Transport for Http {
    fn post(&self, url: &str, key: &str, body: &Value) -> Result<(u16, String), String> {
        let r = self.0.post(url).bearer_auth(key).header("accept", "application/json").json(body).send().map_err(|e| {
            if e.is_timeout() { "Allternit took too long to answer. Try again in a minute.".to_string() } else { format!("Couldn't reach Allternit at {url}: {}", e.without_url()) }
        })?;
        let status = r.status().as_u16();
        Ok((status, r.text().unwrap_or_default()))
    }
}

/// One tool call: the tool's result (`structuredContent`, else its text), or why there isn't one.
fn call(t: &dyn Transport, cfg: &Config, tool: &str, args: &Value) -> Result<Value, Failure> {
    let url = format!("{}/bots/{}", cfg.url, cfg.bot);
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": tool, "arguments": args } });
    let (status, text) = t.post(&url, &cfg.key, &body).map_err(Failure::Unreachable)?;
    match status {
        200 => {}
        401 | 403 => return Err(Failure::Refused("Allternit refused that key. It may have been revoked, or it is for another bot. Ask the owner for a new one.".into())),
        404 => return Err(Failure::Refused("Allternit doesn't have that bot. Check the bot id.".into())),
        s => return Err(Failure::Unreachable(format!("Allternit answered {s}. Try again in a minute."))),
    }
    let v: Value = serde_json::from_str(&text).map_err(|_| Failure::Unreachable("Allternit answered something unreadable.".into()))?;
    if let Some(e) = v.get("error") {
        // The edge's plain "your computer is offline" arrives as a JSON-RPC error with code -32000.
        let msg = e["message"].as_str().unwrap_or("Allternit couldn't do that.").to_string();
        return Err(if e["code"] == -32000 { Failure::Unreachable(msg) } else { Failure::Refused(msg) });
    }
    let result = &v["result"];
    if result["isError"] == true {
        return Err(Failure::Refused(result["content"][0]["text"].as_str().unwrap_or("Allternit refused that.").to_string()));
    }
    Ok(match result.get("structuredContent").filter(|s| !s.is_null()) {
        Some(s) => s.clone(),
        None => result["content"][0]["text"].clone(),
    })
}

// ─── Output ────────────────────────────────────────────────────────────────────

fn render(tool: &str, v: &Value) -> String {
    let s = |x: &Value| x.as_str().unwrap_or("").to_string();
    match tool {
        "list_threads" => {
            let rows: Vec<String> = v["threads"]
                .as_array()
                .map(|a| a.iter().map(|t| format!("{}  {}  {}", s(&t["threadId"]), if t["channel"].is_null() { "-".into() } else { s(&t["channel"]) }, s(&t["title"]))).collect())
                .unwrap_or_default();
            if rows.is_empty() { "No threads are shared with this bot.".into() } else { rows.join("\n") }
        }
        "read_thread" => {
            let lines: Vec<String> = v["messages"]
                .as_array()
                .map(|a| a.iter().map(|m| format!("[{}] {}: {}", s(&m["at"]), s(&m["actor"]["type"]), if m["text"].is_null() { s(&m["type"]) } else { s(&m["text"]) })).collect())
                .unwrap_or_default();
            format!("{}\n{}", s(&v["title"]), if lines.is_empty() { "(nothing yet)".into() } else { lines.join("\n") })
        }
        "send_text" => format!("Sent. Thread {}.", s(&v["threadId"])),
        "start_call" => format!("Calling. Thread {}.", s(&v["threadId"])),
        "send_email" if v["status"] == "pending_approval" => "Drafted. It has not been sent: it waits for the owner's OK.".into(),
        "send_email" => "Queued.".into(),
        "ask_bot" => ["reply", "text", "message"].iter().find_map(|k| v[k].as_str()).map(str::to_string).unwrap_or_else(|| serde_json::to_string_pretty(v).unwrap_or_default()),
        _ => match v {
            Value::String(t) => t.clone(),
            other => serde_json::to_string_pretty(other).unwrap_or_default(),
        },
    }
}

// ─── Run ───────────────────────────────────────────────────────────────────────

fn login(bot: &str, stdin: &mut dyn Read, env: &dyn Fn(&str) -> Option<String>, path: &std::path::Path, flags: &Flags) -> Result<String, Usage> {
    let key = match env("ALLTERNIT_BOT_KEY").filter(|k| !k.trim().is_empty()) {
        Some(k) => k,
        None => {
            let mut s = String::new();
            stdin.read_to_string(&mut s).map_err(|e| Usage(format!("Couldn't read the key: {e}")))?;
            s.lines().next().unwrap_or("").to_string()
        }
    };
    let key = key.trim();
    if !key.starts_with("abk_") && !key.starts_with("ak_") {
        return usage("That doesn't look like an Allternit bot key (abk_...). Pipe it in or set ALLTERNIT_BOT_KEY.");
    }
    let mut cfg = json!({ "key": key, "bot": bot });
    if let Some(u) = &flags.url {
        cfg["url"] = json!(u);
    }
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut f = std::fs::OpenOptions::new();
        f.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut f, 0o600);
        f.open(path)?.write_all(serde_json::to_string_pretty(&cfg).unwrap().as_bytes())
    };
    write().map_err(|e| Usage(format!("Couldn't save the config at {}: {e}", path.display())))?;
    Ok(format!("Saved. allternit-bot will act as bot {bot}."))
}

fn run(argv: &[String], stdin: &mut dyn Read, env: &dyn Fn(&str) -> Option<String>, t: &dyn Transport, cfg_path: &std::path::Path, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let (command, flags) = match parse(argv, stdin) {
        Ok(x) => x,
        Err(Usage(m)) => {
            let _ = writeln!(err, "{m}");
            return 2;
        }
    };
    match command {
        Command::Help => {
            let _ = write!(out, "{HELP}");
            0
        }
        Command::Version => {
            let _ = writeln!(out, "allternit-bot {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Command::Login { bot } => match login(&bot, stdin, env, cfg_path, &flags) {
            Ok(m) => {
                let _ = writeln!(out, "{m}");
                0
            }
            Err(Usage(m)) => {
                let _ = writeln!(err, "{m}");
                2
            }
        },
        Command::Tool { name, args } => {
            let file = std::fs::read_to_string(cfg_path).ok().and_then(|t| serde_json::from_str(&t).ok());
            let cfg = match resolve_config(&flags, env, file) {
                Ok(c) => c,
                Err(Usage(m)) => {
                    let _ = writeln!(err, "{m}");
                    return 2;
                }
            };
            match call(t, &cfg, name, &args) {
                Ok(v) => {
                    let _ = if flags.json { writeln!(out, "{}", serde_json::to_string_pretty(&v).unwrap_or_default()) } else { writeln!(out, "{}", render(name, &v)) };
                    0
                }
                Err(Failure::Refused(m)) => {
                    let _ = writeln!(err, "{m}");
                    1
                }
                Err(Failure::Unreachable(m)) => {
                    let _ = writeln!(err, "{m}");
                    2
                }
            }
        }
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let env = |k: &str| std::env::var(k).ok();
    let code = run(&argv, &mut std::io::stdin(), &env, &Http::new(), &config_path(), &mut std::io::stdout(), &mut std::io::stderr());
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn a(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }
    fn p(s: &str) -> Result<(Command, Flags), Usage> {
        parse(&a(s), &mut std::io::empty())
    }
    fn tool(s: &str) -> (&'static str, Value) {
        match p(s).unwrap().0 {
            Command::Tool { name, args } => (name, args),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn every_tool_has_a_command_and_the_args_match_the_connectors_schema() {
        assert_eq!(tool("threads --channel sms --query lease"), ("list_threads", json!({ "channel": "sms", "query": "lease" })));
        assert_eq!(tool("read th_1 --limit 5"), ("read_thread", json!({ "threadId": "th_1", "limit": 5 })));
        assert_eq!(tool("text +14155550123 on my way"), ("send_text", json!({ "to": "+14155550123", "text": "on my way" })));
        assert_eq!(tool("call +14155550123 confirm the booking"), ("start_call", json!({ "to": "+14155550123", "purpose": "confirm the booking" })));
        assert_eq!(tool("email a@b.co Hello the body is here"), ("send_email", json!({ "to": "a@b.co", "subject": "Hello", "body": "the body is here" })));
        assert_eq!(tool(r#"post slack {"kind":"user","id":"U1"} hi there"#).1["target"], json!({ "kind": "user", "id": "U1" }));
        assert_eq!(tool("ask what is the plan"), ("ask_bot", json!({ "text": "what is the plan" })));
        assert_eq!(tool("tickets"), ("list_open_tickets", json!({})));
        assert_eq!(tool("ticket T-3"), ("get_ticket", json!({ "id": "T-3" })));
        assert_eq!(tool(r#"result T-3 all done --data {"n":1}"#), ("post_result", json!({ "id": "T-3", "summary": "all done", "data": { "n": 1 } })));
    }

    #[test]
    fn the_cli_covers_exactly_the_tools_the_connector_lists() {
        // The tool names the connector exposes (mcp_vendor_bots::tool_descriptors); the instructions file lists each.
        for t in ["list_threads", "read_thread", "send_text", "start_call", "send_email", "post_message", "ask_bot", "get_ticket", "post_result", "list_open_tickets"] {
            assert!(HELP.contains(t), "{t}");
        }
        for c in ["threads", "read x", "text +1 a", "call +1 a", "email a b c", "post p {} t", "ask a", "tickets", "ticket T-1", "result T-1 s"] {
            assert!(p(c).is_ok(), "{c}");
        }
    }

    #[test]
    fn bad_usage_says_what_is_missing() {
        assert!(matches!(p("text +14155550123"), Err(Usage(m)) if m == "The text is empty."));
        assert!(matches!(p("read"), Err(Usage(m)) if m.contains("thread id")));
        assert!(matches!(p("post slack notjson hi"), Err(Usage(m)) if m.contains("JSON")));
        assert!(matches!(p("frobnicate"), Err(Usage(m)) if m.contains("Unknown command")));
        assert!(matches!(p("threads --bogus"), Err(Usage(m)) if m.contains("Unknown option")));
        assert!(matches!(p("read x --limit abc"), Err(Usage(_))));
        assert_eq!(p("").unwrap().0, Command::Help);
        assert_eq!(p("help").unwrap().0, Command::Help);
    }

    #[test]
    fn a_dash_reads_the_text_from_stdin() {
        let (c, _) = parse(&a("text +14155550123 -"), &mut "line one\nline two\n".as_bytes()).unwrap();
        assert_eq!(c, Command::Tool { name: "send_text", args: json!({ "to": "+14155550123", "text": "line one\nline two" }) });
    }

    #[test]
    fn config_comes_from_flags_then_env_then_file_and_the_key_never_from_argv() {
        let file = json!({ "key": "abk_file", "bot": "b_file", "url": "https://file.example/mcp/" });
        let env_none = |_: &str| None;
        let c = resolve_config(&Flags::default(), &env_none, Some(file.clone())).unwrap();
        assert_eq!((c.key.as_str(), c.bot.as_str(), c.url.as_str()), ("abk_file", "b_file", "https://file.example/mcp"));
        let env = |k: &str| match k {
            "ALLTERNIT_BOT_KEY" => Some("abk_env".to_string()),
            "ALLTERNIT_BOT_ID" => Some("b_env".to_string()),
            _ => None,
        };
        let c = resolve_config(&Flags { bot: Some("b_flag".into()), ..Default::default() }, &env, Some(file)).unwrap();
        assert_eq!((c.key.as_str(), c.bot.as_str()), ("abk_env", "b_flag"));
        assert_eq!(resolve_config(&Flags::default(), &env_none, None).unwrap_err().0.contains("No key"), true);
        assert_eq!(resolve_config(&Flags::default(), &env, None).unwrap().url, DEFAULT_URL);
    }

    struct Fake {
        reply: (u16, String),
        seen: RefCell<Vec<(String, String, Value)>>,
    }
    impl Fake {
        fn new(status: u16, body: Value) -> Self {
            Self { reply: (status, body.to_string()), seen: RefCell::new(vec![]) }
        }
    }
    impl Transport for Fake {
        fn post(&self, url: &str, key: &str, body: &Value) -> Result<(u16, String), String> {
            self.seen.borrow_mut().push((url.into(), key.into(), body.clone()));
            Ok(self.reply.clone())
        }
    }
    fn cfg() -> Config {
        Config { key: "abk_k".into(), bot: "b1".into(), url: "https://mcp.example/mcp".into() }
    }
    fn ok(structured: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": 1, "result": { "content": [{ "type": "text", "text": "x" }], "structuredContent": structured, "isError": false } })
    }

    #[test]
    fn a_call_is_one_tools_call_to_the_bots_url_with_the_key_as_bearer() {
        let f = Fake::new(200, ok(json!({ "sent": true, "threadId": "th_9" })));
        let v = call(&f, &cfg(), "send_text", &json!({ "to": "+1", "text": "hi" })).unwrap();
        assert_eq!(render("send_text", &v), "Sent. Thread th_9.");
        let seen = f.seen.borrow();
        assert_eq!((seen[0].0.as_str(), seen[0].1.as_str()), ("https://mcp.example/mcp/bots/b1", "abk_k"));
        assert_eq!(seen[0].2["method"], "tools/call");
        assert_eq!(seen[0].2["params"], json!({ "name": "send_text", "arguments": { "to": "+1", "text": "hi" } }));
    }

    #[test]
    fn a_refusal_is_the_servers_own_sentence() {
        let f = Fake::new(200, json!({ "jsonrpc": "2.0", "id": 1, "result": { "content": [{ "type": "text", "text": "They haven't contacted this number." }], "isError": true } }));
        assert_eq!(call(&f, &cfg(), "send_text", &json!({})), Err(Failure::Refused("They haven't contacted this number.".into())));
    }

    #[test]
    fn http_and_edge_failures_map_to_refused_or_unreachable() {
        let run = |status, body: Value| call(&Fake::new(status, body), &cfg(), "list_threads", &json!({}));
        assert!(matches!(run(401, json!({})), Err(Failure::Refused(m)) if m.contains("revoked")));
        assert!(matches!(run(404, json!({})), Err(Failure::Refused(m)) if m.contains("bot id")));
        assert!(matches!(run(503, json!({})), Err(Failure::Unreachable(_))));
        let offline = json!({ "jsonrpc": "2.0", "id": 1, "error": { "code": -32000, "message": "Your Allternit computer is offline; it will be woken — try again in a minute." } });
        assert!(matches!(run(200, offline), Err(Failure::Unreachable(m)) if m.contains("offline")));
        let bad = json!({ "jsonrpc": "2.0", "id": 1, "error": { "code": -32602, "message": "Unknown tool: x" } });
        assert!(matches!(run(200, bad), Err(Failure::Refused(_))));
    }

    #[test]
    fn rendering_is_plain_and_the_email_says_it_waits() {
        let threads = json!({ "threads": [{ "threadId": "t1", "title": "Lease", "channel": "sms" }, { "threadId": "t2", "title": "Bot chat", "channel": null }] });
        assert_eq!(render("list_threads", &threads), "t1  sms  Lease\nt2  -  Bot chat");
        assert_eq!(render("list_threads", &json!({ "threads": [] })), "No threads are shared with this bot.");
        assert!(render("send_email", &json!({ "status": "pending_approval" })).contains("owner's OK"));
        let read = json!({ "title": "Lease", "messages": [{ "at": "t", "actor": { "type": "contact" }, "text": "hello" }] });
        assert_eq!(render("read_thread", &read), "Lease\n[t] contact: hello");
    }

    #[test]
    fn run_returns_the_exit_codes_and_writes_json_on_request() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let env = |k: &str| match k {
            "ALLTERNIT_BOT_KEY" => Some("abk_k".to_string()),
            "ALLTERNIT_BOT_ID" => Some("b1".to_string()),
            _ => None,
        };
        let go = |args: &str, f: &Fake| {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = run(&a(args), &mut std::io::empty(), &env, f, &path, &mut out, &mut err);
            (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
        };
        let good = Fake::new(200, ok(json!({ "threads": [] })));
        assert_eq!(go("threads --json", &good).1.trim(), "{\n  \"threads\": []\n}");
        assert_eq!(go("threads", &good).0, 0);
        let refused = Fake::new(401, json!({}));
        let (code, _, err) = go("threads", &refused);
        assert_eq!(code, 1);
        assert!(err.contains("revoked"));
        assert_eq!(go("frobnicate", &good).0, 2);
        assert_eq!(go("help", &good).0, 0);
        assert!(go("help", &good).1.contains("allternit-bot text"));
    }

    #[test]
    fn the_real_http_client_sends_the_bearer_key_to_the_bots_path_and_reads_the_answer() {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(sock.try_clone().unwrap());
            let (mut head, mut len) = (String::new(), 0usize);
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" { break }
                if let Some(v) = line.to_lowercase().strip_prefix("content-length:") { len = v.trim().parse().unwrap() }
                head.push_str(&line);
            }
            let mut body = vec![0u8; len];
            reader.read_exact(&mut body).unwrap();
            let reply = ok(json!({ "threads": [] })).to_string();
            write!(sock, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}", reply.len()).unwrap();
            (head, String::from_utf8(body).unwrap())
        });
        let cfg = Config { key: "abk_wire".into(), bot: "b7".into(), url: format!("http://127.0.0.1:{port}/mcp") };
        let v = call(&Http::new(), &cfg, "list_threads", &json!({})).unwrap();
        assert_eq!(v, json!({ "threads": [] }));
        let (head, body) = server.join().unwrap();
        assert!(head.starts_with("POST /mcp/bots/b7 HTTP/1.1"), "{head}");
        assert!(head.to_lowercase().contains("authorization: bearer abk_wire"), "{head}");
        assert!(body.contains("\"list_threads\""));
    }

    #[test]
    fn login_saves_the_key_privately_and_refuses_something_that_is_not_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.json");
        let env_none = |_: &str| None;
        let msg = login("b1", &mut "abk_secret\n".as_bytes(), &env_none, &path, &Flags::default()).unwrap();
        assert!(msg.contains("b1") && !msg.contains("abk_secret"));
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved, json!({ "key": "abk_secret", "bot": "b1" }));
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions()) & 0o777, 0o600);
        assert!(login("b1", &mut "hunter2".as_bytes(), &env_none, &path, &Flags::default()).is_err());
    }
}
