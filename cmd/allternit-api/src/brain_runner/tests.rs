//! Fixtures are the strings step 0.2 recorded (`FINDINGS-brain-runner-0.2.md`)
//! and, for kimi, the lines its CLI printed on Eoj's Mac that day.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use super::stream::{Outcome, Parser};
use super::vendors::{self, Msg, SignIn, Vendor};
use super::{normalize, protocol, Config};

fn cfg(home: &Path) -> Config {
    Config {
        socket_url: "ws://127.0.0.1:1/socket".into(),
        token: "t".into(),
        home: home.to_path_buf(),
        vendors: vec![Vendor::Claude],
        turn_timeout: Duration::from_secs(20),
        codex_catalog: true,
        bins: HashMap::new(),
        home_overrides: HashMap::new(),
    }
}

fn run_parser(vendor: Vendor, lines: &[&str], exit_ok: bool, stderr: &str) -> (String, Outcome) {
    let mut parser = Parser::new(vendor);
    let mut text = String::new();
    for line in lines {
        for delta in parser.line(line) {
            text.push_str(&delta);
        }
    }
    let (tail, outcome) = parser.finish(exit_ok, stderr);
    if let Some(tail) = tail {
        text.push_str(&tail);
    }
    (text, outcome)
}

// ─── Stream parsing ──────────────────────────────────────────────────────────

const CLAUDE_OK: &[&str] = &[
    r#"{"type":"system","subtype":"init","cwd":"/home/runner/claude","session_id":"5f0c","tools":[],"mcp_servers":[],"model":"claude-sonnet-5-5"}"#,
    r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg_1","role":"assistant","content":[]}}}"#,
    r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}"#,
    r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"O"}}}"#,
    r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"K"}}}"#,
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"OK"}]}}"#,
    r#"{"type":"result","subtype":"success","is_error":false,"result":"OK","usage":{"input_tokens":12,"cache_creation_input_tokens":0,"cache_read_input_tokens":3,"output_tokens":2}}"#,
];

#[test]
fn brain_runner_claude_stream_sends_each_text_delta_once() {
    let (text, outcome) = run_parser(Vendor::Claude, CLAUDE_OK, true, "");
    // The whole `assistant` message and the `result` repeat the deltas; they
    // must not be sent a second time.
    assert_eq!(text, "OK");
    assert_eq!(outcome, Outcome::Done { input_tokens: Some(15), output_tokens: Some(2) });
}

#[test]
fn brain_runner_claude_signed_out_is_login_expired() {
    let lines = [
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Not logged in · Please run /login"}]},"error":"authentication_failed"}"#,
        r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#,
    ];
    let (text, outcome) = run_parser(Vendor::Claude, &lines, false, "");
    assert_eq!(text, "", "the signed-out notice is not a reply");
    assert_eq!(outcome, Outcome::Error { code: "login_expired", message: "Sign-in expired".into() });
}

#[test]
fn brain_runner_claude_result_without_deltas_is_still_delivered() {
    let lines = [r#"{"type":"result","subtype":"success","is_error":false,"result":"Hello there","usage":{"input_tokens":4,"output_tokens":2}}"#];
    let (text, _) = run_parser(Vendor::Claude, &lines, true, "");
    assert_eq!(text, "Hello there");
}

#[test]
fn brain_runner_codex_stream_takes_agent_messages_only() {
    let lines = [
        r#"{"type":"thread.started","thread_id":"0199a1"}"#,
        r#"{"type":"turn.started"}"#,
        r#"{"type":"item.completed","item":{"id":"item_0","type":"reasoning","text":"**Answering briefly**"}}"#,
        r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"OK"}}"#,
        r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"Anything else?"}}"#,
        r#"{"type":"turn.completed","usage":{"input_tokens":20,"cached_input_tokens":0,"output_tokens":3}}"#,
    ];
    let (text, outcome) = run_parser(Vendor::Chatgpt, &lines, true, "");
    assert_eq!(text, "OK\n\nAnything else?");
    assert_eq!(outcome, Outcome::Done { input_tokens: Some(20), output_tokens: Some(3) });
}

#[test]
fn brain_runner_codex_plan_limit_is_vendor_limited_with_the_retry_time() {
    let limit = "You've hit your usage limit. Upgrade to Pro (https://chatgpt.com/explore/pro), visit https://chatgpt.com/codex/settings/usage to purchase more credits or try again at 1:39 PM.";
    let lines = [
        r#"{"type":"thread.started","thread_id":"0199a1"}"#.to_string(),
        json!({"type": "error", "message": limit}).to_string(),
        json!({"type": "turn.failed", "error": {"message": limit}}).to_string(),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let (text, outcome) = run_parser(Vendor::Chatgpt, &refs, false, "");
    assert_eq!(text, "");
    match outcome {
        Outcome::Error { code, message } => {
            assert_eq!(code, "vendor_limited");
            assert!(message.contains("try again at 1:39 PM"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn brain_runner_codex_missing_bearer_is_login_expired() {
    let lines = [r#"{"type":"error","message":"unexpected status 401 Unauthorized: Missing bearer or basic authentication in header"}"#];
    let (_, outcome) = run_parser(Vendor::Chatgpt, &lines, false, "");
    assert_eq!(outcome, Outcome::Error { code: "login_expired", message: "Sign-in expired".into() });
}

#[test]
fn brain_runner_codex_reconnect_errors_do_not_fail_a_finished_turn() {
    let lines = [
        r#"{"type":"error","message":"Reconnecting... 1/5"}"#,
        r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"OK"}}"#,
        r#"{"type":"turn.completed","usage":{"input_tokens":9,"output_tokens":1}}"#,
    ];
    let (text, outcome) = run_parser(Vendor::Chatgpt, &lines, true, "");
    assert_eq!(text, "OK");
    assert!(matches!(outcome, Outcome::Done { .. }));
}

#[test]
fn brain_runner_kimi_stream_skips_meta_and_reads_assistant_content() {
    let lines = [
        // Printed by kimi 2.5.2 on 2026-11-14 (scratch kimi.out).
        r#"{"role":"meta","type":"system.version","version":"2.5.2"}"#,
        r#"{"role":"assistant","content":[{"type":"think","think":"short"},{"type":"text","text":"OK"}]}"#,
    ];
    let (text, outcome) = run_parser(Vendor::Kimi, &lines, true, "");
    assert_eq!(text, "OK");
    assert!(matches!(outcome, Outcome::Done { .. }));
}

#[test]
fn brain_runner_kimi_lapsed_subscription_is_vendor_limited() {
    // kimi 2.5.2 stderr on 2026-11-14 (scratch kimi.err).
    let stderr = "Warning: [loop_control] 'max_retries_per_step' is deprecated and no longer used; rename it to 'max_attempts_per_step'. Run /update-config to fix it.\nerror: failed to run prompt: provider.auth_error: Your current subscription does not have access to Kimi Code right now. Upgrade your plan to keep coding with Kimi Code: https://www.kimi.com/code/#pricing\nSee log: /var/folders/9v/T/tmp.TPgVUwMZuJ/logs/kimi-code.log\n";
    let lines = [r#"{"role":"meta","type":"system.version","version":"2.5.2"}"#];
    match run_parser(Vendor::Kimi, &lines, false, stderr).1 {
        Outcome::Error { code, message } => {
            assert_eq!(code, "vendor_limited");
            assert!(message.starts_with("error: failed to run prompt: provider.auth_error"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn brain_runner_kimi_and_grok_signed_out_are_login_expired() {
    let kimi = run_parser(Vendor::Kimi, &[], false, "error: No model configured. Run `kimi login` first.\n").1;
    assert_eq!(kimi, Outcome::Error { code: "login_expired", message: "Sign-in expired".into() });
    let grok = run_parser(Vendor::Grok, &[], false, "Not signed in. Run `grok login --device-code` to sign in.\n").1;
    assert_eq!(grok, Outcome::Error { code: "login_expired", message: "Sign-in expired".into() });
}

#[test]
fn brain_runner_unknown_failure_reports_the_last_stderr_line() {
    let outcome = run_parser(Vendor::Grok, &[], false, "starting\nError: connect ECONNREFUSED 10.0.0.1:443\n\n").1;
    assert_eq!(outcome, Outcome::Error { code: "cli_failed", message: "Error: connect ECONNREFUSED 10.0.0.1:443".into() });
}

// ─── Status ──────────────────────────────────────────────────────────────────

#[test]
fn brain_runner_status_outputs_parse_per_cli() {
    let claude_in = "{\n  \"loggedIn\": true,\n  \"authMethod\": \"claude.ai\",\n  \"apiProvider\": \"firstParty\",\n  \"subscriptionType\": \"max\"\n}\n";
    assert_eq!(vendors::parse_status(Vendor::Claude, true, claude_in), SignIn::In { plan: Some("max".into()) });
    assert_eq!(vendors::parse_status(Vendor::Claude, false, "{\"loggedIn\": false}\n"), SignIn::Out);

    assert_eq!(vendors::parse_status(Vendor::Chatgpt, true, "Logged in using ChatGPT\n"), SignIn::In { plan: None });
    assert_eq!(vendors::parse_status(Vendor::Chatgpt, false, "Not logged in\n"), SignIn::Out);

    // grok exits 0 when signed out; only the text tells.
    assert_eq!(vendors::parse_status(Vendor::Grok, true, "You are not authenticated.\n"), SignIn::Out);
    assert_eq!(vendors::parse_status(Vendor::Grok, true, "grok-4.6 (default)\ngrok-4.5\n"), SignIn::In { plan: None });

    // kimi exits 0 always.
    assert_eq!(vendors::parse_status(Vendor::Kimi, true, "No providers configured.\n"), SignIn::Out);
    assert_eq!(
        vendors::parse_status(Vendor::Kimi, true, "managed:kimi-code  kimi-code/kimi-for-coding  source=oauth\n"),
        SignIn::In { plan: None }
    );
}

// ─── Login views ─────────────────────────────────────────────────────────────

#[test]
fn brain_runner_login_links_split_into_page_and_code() {
    // Grok and Kimi print the code inside the link.
    let (url, code, with_code) = vendors::login_fields(Vendor::Grok, Some("https://accounts.x.ai/oauth2/device?user_code=4782-CGRP"), Some("4782-CGRP"));
    assert_eq!(url.as_deref(), Some("https://accounts.x.ai/oauth2/device"));
    assert_eq!(code.as_deref(), Some("4782-CGRP"));
    assert_eq!(with_code.as_deref(), Some("https://accounts.x.ai/oauth2/device?user_code=4782-CGRP"));

    // Kimi's code is only in the link (the code regex skips links on purpose).
    let (url, code, with_code) = vendors::login_fields(Vendor::Kimi, Some("https://www.kimi.ai/code/authorize_device?user_code=QW3R-T7YP"), None);
    assert_eq!(url.as_deref(), Some("https://www.kimi.ai/code/authorize_device"));
    assert_eq!(code.as_deref(), Some("QW3R-T7YP"));
    assert!(with_code.is_some());

    // Codex: page + separate code. Claude: the link only, the code comes back by paste.
    let (url, code, with_code) = vendors::login_fields(Vendor::Chatgpt, Some("https://auth.openai.com/codex/device"), Some("ZWBG-JYPGA"));
    assert_eq!((url.as_deref(), code.as_deref(), with_code), (Some("https://auth.openai.com/codex/device"), Some("ZWBG-JYPGA"), None));
    let claude = "https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback";
    let (url, code, with_code) = vendors::login_fields(Vendor::Claude, Some(claude), None);
    assert_eq!((url.as_deref(), code, with_code), (Some(claude), None, None));
}

// ─── Turn commands ───────────────────────────────────────────────────────────

fn history() -> Vec<Msg> {
    vec![
        Msg { user: true, content: "Hi".into() },
        Msg { user: false, content: "Hello".into() },
        Msg { user: true, content: "Reply with OK".into() },
    ]
}

#[test]
fn brain_runner_claude_replays_history_on_stdin_with_tools_off() {
    let dir = tempfile::tempdir().unwrap();
    let plan = vendors::turn_plan(&cfg(dir.path()), Vendor::Claude, "sonnet", "Be brief", &history()).unwrap();
    let pos = |flag: &str| plan.args.iter().position(|a| a == flag).unwrap();
    assert_eq!(plan.args[pos("--tools") + 1], "");
    assert_eq!(plan.args[pos("--setting-sources") + 1], "");
    assert_eq!(plan.args[pos("--mcp-config") + 1], r#"{"mcpServers":{}}"#);
    assert_eq!(plan.args[pos("--system-prompt") + 1], "Be brief");
    assert_eq!(plan.args[pos("--model") + 1], "sonnet");
    let lines: Vec<Value> = plan.stdin.unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[1], json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": "Hello"}]}}));
    assert_eq!(lines[2], json!({"type": "user", "message": {"role": "user", "content": "Reply with OK"}}));
}

#[test]
fn brain_runner_codex_runs_against_the_tools_off_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let c = cfg(dir.path());
    let plan = vendors::turn_plan(&c, Vendor::Chatgpt, "gpt-6-luna", "Say \"hi\"\nthen stop", &history()).unwrap();
    let catalog = format!("model_catalog_json={}", serde_json::to_string(&vendors::codex_catalog_path(&c).to_string_lossy()).unwrap());
    assert!(plan.args.contains(&catalog), "{:?}", plan.args);
    // Quotes and newlines in the system prompt stay inside one TOML string.
    assert!(plan.args.contains(&r#"developer_instructions="Say \"hi\"\nthen stop""#.to_string()));
    for flag in ["multi_agent", "multi_agent_v2", "view_image"] {
        assert!(plan.args.windows(2).any(|w| w[0] == "--disable" && w[1] == flag));
    }
    assert_eq!(plan.args.last().map(String::as_str), Some("-"));
    let stdin = plan.stdin.unwrap();
    assert!(stdin.starts_with("Here is the conversation so far:\n\n[User]\nHi\n\n[Assistant]\nHello\n\n"));
    assert!(stdin.ends_with("Reply to the user's latest message:\n\nReply with OK"));
}

#[test]
fn brain_runner_kimi_writes_a_tools_free_agent_file() {
    let dir = tempfile::tempdir().unwrap();
    let c = cfg(dir.path());
    let plan = vendors::turn_plan(&c, Vendor::Kimi, "kimi-code/kimi-for-coding", "Be brief", &history()[2..]).unwrap();
    assert_eq!(plan.files, vec![(vendors::kimi_agent_file(&c), "---\nname: chat-only\ndescription: Plain chat, no tools\ntools: []\n---\nBe brief\n".to_string())]);
    let prompt = &plan.args[plan.args.iter().position(|a| a == "-p").unwrap() + 1];
    assert_eq!(prompt, "Reply with OK", "a single message needs no preamble");
}

#[test]
fn brain_runner_long_history_drops_oldest_turns_to_fit_argv() {
    let mut long = Vec::new();
    for i in 0..40 {
        long.push(Msg { user: i % 2 == 0, content: format!("turn {i} {}", "x".repeat(5_000)) });
    }
    long.push(Msg { user: true, content: "latest".into() });
    let text = vendors::transcript(&long, vendors::MAX_ARG_BYTES).unwrap();
    assert!(text.len() <= vendors::MAX_ARG_BYTES);
    assert!(!text.contains("turn 0 "), "oldest turn should be dropped");
    assert!(text.contains("turn 39 "), "newest earlier turn should stay");
    assert!(text.ends_with("latest"));
    let too_long = vec![Msg { user: true, content: "y".repeat(vendors::MAX_ARG_BYTES + 1) }];
    assert!(vendors::transcript(&too_long, vendors::MAX_ARG_BYTES).is_err());
}

#[test]
fn brain_runner_models_cannot_smuggle_flags() {
    let dir = tempfile::tempdir().unwrap();
    for bad in ["--dangerously-skip-permissions", "", "sonnet --tools Bash", "a;b"] {
        assert!(vendors::turn_plan(&cfg(dir.path()), Vendor::Claude, bad, "s", &history()).is_err(), "{bad:?}");
    }
    assert!(vendors::valid_model("kimi-code/k3.1-pro-highspeed"));
}

#[test]
fn brain_runner_codex_catalog_edit_clears_every_tool_field() {
    let catalog = json!({"models": [
        {"slug": "gpt-6-luna", "shell_type": "shell_command", "apply_patch_tool_type": "freeform", "tool_mode": "agentic",
         "input_modalities": ["text", "image"], "multi_agent_version": "v2", "experimental_supported_tools": ["read_file"],
         "supports_search_tool": true, "base_instructions": "You are Codex."},
        {"slug": "gpt-6.1-sol", "shell_type": "local"}
    ]});
    let edited = vendors::codex_tools_off(catalog).unwrap();
    for model in edited["models"].as_array().unwrap() {
        assert_eq!(model["shell_type"], "disabled");
        assert_eq!(model["apply_patch_tool_type"], Value::Null);
        assert_eq!(model["tool_mode"], "default");
        assert_eq!(model["input_modalities"], json!(["text"]));
        assert_eq!(model["multi_agent_version"], Value::Null);
        assert_eq!(model["experimental_supported_tools"], json!([]));
        assert_eq!(model["supports_search_tool"], false);
    }
    assert_eq!(edited["models"][0]["base_instructions"], "You are Codex.");
    assert!(vendors::codex_tools_off(json!({"models": []})).is_err());
}

#[test]
fn brain_runner_turn_messages_are_chat_only() {
    let wire = |role: &str, content: Value| protocol::WireMessage { role: role.into(), content };
    let (system, history) = normalize(
        Some("A".into()),
        vec![wire("system", json!("B")), wire("user", json!([{"type": "text", "text": "Hi"}]))],
    )
    .unwrap();
    assert_eq!(system, "A\n\nB");
    assert_eq!(history, vec![Msg { user: true, content: "Hi".into() }]);
    assert!(normalize(None, vec![wire("user", json!("Hi")), wire("tool", json!("ls output"))]).is_err());
    assert!(normalize(None, vec![wire("user", json!([{"type": "image_url", "image_url": {"url": "x"}}]))]).is_err());
    assert!(normalize(None, vec![wire("user", json!("Hi")), wire("assistant", json!("Hello"))]).is_err());
    assert_eq!(normalize(None, vec![wire("user", json!("Hi"))]).unwrap().0, vendors::DEFAULT_SYSTEM);
}

#[test]
fn brain_runner_config_reads_env_and_rejects_unknown_vendors() {
    let env = |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|(key, _)| *key == k).map(|(_, v)| v.to_string());
    let c = Config::from_lookup(env(&[("ALLTERNIT_RUNNER_URL", "wss://api.allternit.com/api/v1/brain-runner/socket"), ("ALLTERNIT_RUNNER_TOKEN", "x")])).unwrap();
    assert_eq!(c.vendors, vec![Vendor::Claude]);
    assert_eq!(c.home, PathBuf::from("/home/runner"));
    assert_eq!(c.turn_timeout, Duration::from_secs(180));
    assert!(c.codex_catalog);
    let c = Config::from_lookup(env(&[
        ("ALLTERNIT_RUNNER_URL", "wss://h/s"), ("ALLTERNIT_RUNNER_TOKEN", "x"),
        ("ALLTERNIT_RUNNER_VENDORS", "claude, kimi,claude"), ("ALLTERNIT_RUNNER_CODEX_CATALOG", "0"),
    ]))
    .unwrap();
    assert_eq!(c.vendors, vec![Vendor::Claude, Vendor::Kimi]);
    assert!(!c.codex_catalog);
    assert!(Config::from_lookup(env(&[("ALLTERNIT_RUNNER_URL", "wss://h/s"), ("ALLTERNIT_RUNNER_TOKEN", "x"), ("ALLTERNIT_RUNNER_VENDORS", "copilot")])).is_err());
    assert!(Config::from_lookup(env(&[("ALLTERNIT_RUNNER_URL", "https://h/s"), ("ALLTERNIT_RUNNER_TOKEN", "x")])).is_err());
}

// ─── Fake socket + fake claude ───────────────────────────────────────────────

/// Prints what claude 2.1.325 printed in step 0.2, and checks it was started
/// the way the runner must start it.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
if [ "$1" = auth ]; then
  case "$2" in
    status) printf '%s\n' '{"loggedIn": true, "authMethod": "claude.ai", "subscriptionType": "max"}'; exit 0 ;;
    login)
      [ "$3" = --claudeai ] || { echo "missing --claudeai" >&2; exit 2; }
      echo 'Opening browser to sign in...'
      echo "If the browser didn't open, visit: https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c&response_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback"
      printf '%s' 'Paste code here if prompted > '
      read -r code
      if [ "$code" = 'abc123#xyz' ]; then echo 'Login successful.'; exit 0; fi
      echo 'OAuth error: Invalid code'; exit 1 ;;
    logout) rm -f "$HOME/.credentials.json"; exit 0 ;;
  esac
fi
[ "$1" = -p ] || { echo "not a turn: $*" >&2; exit 3; }
prev=''; tools_off=no; model=''; sys=''
for a in "$@"; do
  if [ "$prev" = --tools ] && [ -z "$a" ]; then tools_off=yes; fi
  if [ "$prev" = --model ]; then model="$a"; fi
  if [ "$prev" = --system-prompt ]; then sys="$a"; fi
  prev="$a"
done
[ "$tools_off" = yes ] || { echo 'tools were not turned off' >&2; exit 4; }
[ "$BROWSER" = true ] && [ "$DISABLE_AUTOUPDATER" = 1 ] || { echo 'runner env missing' >&2; exit 5; }
input=$(cat)
case "$input" in *slow*) sleep 30 ;; esac
case "$input" in *signout*)
  echo '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Not logged in · Please run /login"}]},"error":"authentication_failed"}'
  echo '{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}'
  exit 1 ;;
esac
lines=$(printf '%s\n' "$input" | grep -c '"type"')
echo '{"type":"system","subtype":"init","tools":[],"mcp_servers":[]}'
echo '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"O"}}}'
echo '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"K"}}}'
echo "{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" ($lines lines, $model, $sys)\"}}}"
echo '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"OK"}]}}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"OK","usage":{"input_tokens":12,"cache_read_input_tokens":3,"output_tokens":2}}'
"#;

/// Prints codex 0.169.0's device-code sign-in (with its ANSI colours).
const FAKE_CODEX: &str = "#!/bin/sh\n[ -n \"$CODEX_HOME\" ] || exit 9\nif [ \"$1\" = login ] && [ \"$2\" = --device-auth ]; then\n  printf 'Follow these steps to sign in with ChatGPT using device code authorization:\\n\\n1. Open this link in your browser and sign in to your account\\n   \\033[94mhttps://auth.openai.com/codex/device\\033[0m\\n\\n2. Enter this one-time code \\033[90m(expires in 15 minutes)\\033[0m\\n   \\033[94mZWBG-JYPGA\\033[0m\\n'\n  sleep 30\nfi\nif [ \"$1\" = login ] && [ \"$2\" = status ]; then echo 'Not logged in'; exit 1; fi\n";

fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

type Ws = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

async fn next_frame(ws: &mut Ws) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(15), ws.next())
            .await
            .expect("runner answered in time")
            .expect("socket open")
            .expect("frame");
        if let Message::Text(text) = message {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

async fn send(ws: &mut Ws, frame: Value) {
    ws.send(Message::Text(frame.to_string())).await.unwrap();
}

/// Text and end frame of one turn.
async fn collect_turn(ws: &mut Ws, id: &str) -> (String, Value) {
    let mut text = String::new();
    loop {
        let frame = next_frame(ws).await;
        assert_eq!(frame["id"], id, "{frame}");
        match frame["type"].as_str() {
            Some("turn_delta") => text.push_str(frame["text"].as_str().unwrap()),
            _ => return (text, frame),
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn brain_runner_fake_socket_drives_login_and_a_streamed_turn() {
    let scripts = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let mut config = cfg(home.path());
    config.socket_url = format!("ws://{addr}/api/v1/brain-runner/socket");
    config.token = "runner-token-1".into();
    config.vendors = vec![Vendor::Claude, Vendor::Chatgpt];
    config.bins.insert(Vendor::Claude, write_script(scripts.path(), "claude", FAKE_CLAUDE));
    config.bins.insert(Vendor::Chatgpt, write_script(scripts.path(), "codex", FAKE_CODEX));
    let shutdown = CancellationToken::new();
    let runner = tokio::spawn(super::run(config, shutdown.clone()));

    let (stream, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept()).await.unwrap().unwrap();
    let mut auth = None;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
    let mut ws = tokio_tungstenite::accept_hdr_async(stream, |req: &Request, resp: Response| {
        auth = req.headers().get("authorization").and_then(|v| v.to_str().ok()).map(str::to_string);
        Ok(resp)
    })
    .await
    .unwrap();
    assert_eq!(auth.as_deref(), Some("Bearer runner-token-1"));

    let hello = next_frame(&mut ws).await;
    assert_eq!(hello, json!({"type": "hello", "runner": "0.1.0", "vendors": {"chatgpt": "signed_out", "claude": "signed_in"}}));

    send(&mut ws, json!({"type": "ping", "id": "p1"})).await;
    assert_eq!(next_frame(&mut ws).await, json!({"type": "pong", "id": "p1"}));
    send(&mut ws, json!({"type": "something_new", "id": "u1"})).await; // ignored

    // Claude: link + paste.
    send(&mut ws, json!({"type": "login_start", "id": "l1", "vendor": "claude"})).await;
    let view = next_frame(&mut ws).await;
    assert_eq!(view["type"], "login_view", "{view}");
    assert_eq!(view["id"], "l1");
    assert_eq!(view["kind"], "link_paste");
    assert_eq!(view["url"], "https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c&response_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback");
    assert_eq!(view["code"], Value::Null);
    assert!(view["expires_at"].as_str().unwrap().ends_with('Z'));
    send(&mut ws, json!({"type": "login_submit", "id": "l2", "vendor": "claude", "value": "abc123#xyz"})).await;
    assert_eq!(next_frame(&mut ws).await, json!({"type": "submit_result", "id": "l2", "status": "done"}));
    send(&mut ws, json!({"type": "login_status", "id": "l3", "vendor": "claude"})).await;
    assert_eq!(next_frame(&mut ws).await, json!({"type": "login_status_result", "id": "l3", "status": "done"}));

    // ChatGPT: device code, ANSI stripped; still pending while codex waits.
    send(&mut ws, json!({"type": "login_start", "id": "c1", "vendor": "chatgpt"})).await;
    let view = next_frame(&mut ws).await;
    assert_eq!(
        (view["kind"].as_str(), view["url"].as_str(), view["code"].as_str()),
        (Some("device_code"), Some("https://auth.openai.com/codex/device"), Some("ZWBG-JYPGA")),
        "{view}"
    );
    send(&mut ws, json!({"type": "login_status", "id": "c2", "vendor": "chatgpt"})).await;
    assert_eq!(next_frame(&mut ws).await["status"], "pending");

    // A turn with history: deltas in order, then done with usage.
    let turn = json!({"type": "turn", "id": "t1", "vendor": "claude", "model": "sonnet", "system": "Be brief", "messages": [
        {"role": "user", "content": "Hi"}, {"role": "assistant", "content": "Hello"}, {"role": "user", "content": "Reply with the single word OK."}]});
    send(&mut ws, turn).await;
    let (text, end) = collect_turn(&mut ws, "t1").await;
    assert_eq!(text, "OK (3 lines, sonnet, Be brief)");
    assert_eq!(end, json!({"type": "turn_done", "id": "t1", "input_tokens": 15, "output_tokens": 2}));

    // Signed out → login_expired.
    send(&mut ws, json!({"type": "turn", "id": "t2", "vendor": "claude", "messages": [{"role": "user", "content": "signout"}]})).await;
    let (text, end) = collect_turn(&mut ws, "t2").await;
    assert_eq!(text, "");
    assert_eq!(end, json!({"type": "error", "id": "t2", "code": "login_expired", "message": "Sign-in expired"}));

    // One turn at a time; cancel kills the running one and frees the slot.
    send(&mut ws, json!({"type": "turn", "id": "t3", "vendor": "claude", "messages": [{"role": "user", "content": "slow"}]})).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    send(&mut ws, json!({"type": "turn", "id": "t4", "vendor": "claude", "messages": [{"role": "user", "content": "Hi"}]})).await;
    let busy = next_frame(&mut ws).await;
    assert_eq!((busy["id"].as_str(), busy["code"].as_str()), (Some("t4"), Some("bad_request")), "{busy}");
    send(&mut ws, json!({"type": "cancel", "id": "t3"})).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    send(&mut ws, json!({"type": "turn", "id": "t5", "vendor": "claude", "messages": [{"role": "user", "content": "Hi"}]})).await;
    let (text, end) = collect_turn(&mut ws, "t5").await;
    assert_eq!(end["type"], "turn_done", "the cancelled turn sent nothing and freed the slot: {end}");
    assert_eq!(text, "OK (1 lines, sonnet, You are a helpful assistant. Answer in plain text.)");

    // Vendors off the list and tool messages are refused.
    send(&mut ws, json!({"type": "turn", "id": "t6", "vendor": "grok", "messages": [{"role": "user", "content": "Hi"}]})).await;
    assert_eq!(next_frame(&mut ws).await["code"], "bad_request");
    send(&mut ws, json!({"type": "turn", "id": "t7", "vendor": "claude", "messages": [{"role": "user", "content": "Hi"}, {"role": "tool", "content": "x"}]})).await;
    assert_eq!(next_frame(&mut ws).await["code"], "bad_request");

    // Logout of a vendor with a sign-in still pending stops that sign-in.
    send(&mut ws, json!({"type": "logout", "id": "o0", "vendor": "chatgpt"})).await;
    assert_eq!(next_frame(&mut ws).await["type"], "logout_done");
    assert_eq!(
        crate::cli_tools::login_view("brain-runner:chatgpt").await.map(|(_, exited, _)| exited),
        None,
        "the pending codex sign-in was stopped and forgotten"
    );
    std::fs::create_dir_all(home.path().join("chatgpt")).unwrap();

    // Logout deletes only that vendor's home.
    let claude_home = home.path().join("claude");
    std::fs::write(claude_home.join(".credentials.json"), "{}").unwrap();
    send(&mut ws, json!({"type": "logout", "id": "o1", "vendor": "claude"})).await;
    assert_eq!(next_frame(&mut ws).await, json!({"type": "logout_done", "id": "o1", "home_deleted": true}));
    assert!(!claude_home.exists());
    assert!(home.path().join("chatgpt").exists());

    // The runner dials back after the socket drops.
    drop(ws);
    let (stream, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept()).await.unwrap().unwrap();
    let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
    assert_eq!(next_frame(&mut ws).await["type"], "hello");

    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), runner).await.unwrap().unwrap();
}

/// Live check on a dev Mac (not CI): the real runner against a fake cloud-api
/// socket, with the real CLIs. Prints codex's device-code login object (a
/// throwaway `CODEX_HOME`; the code just expires) and one streamed tools-off
/// claude turn using the signed-in claude in `$HOME` (a home override, so the
/// runner never deletes it). Run with
/// `cargo test -p allternit-api --lib brain_runner_live -- --ignored --nocapture`.
#[cfg(unix)]
#[tokio::test]
#[ignore = "uses the real claude and codex CLIs and the network"]
async fn brain_runner_live_device_code_and_claude_turn() {
    let codex_home = tempfile::tempdir().unwrap();
    let runner_home = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut config = cfg(runner_home.path());
    config.socket_url = format!("ws://{addr}/api/v1/brain-runner/socket");
    config.token = "live-token".into();
    config.turn_timeout = Duration::from_secs(120);
    config.vendors = vec![Vendor::Claude, Vendor::Chatgpt];
    config.home_overrides.insert(Vendor::Claude, PathBuf::from(std::env::var("HOME").unwrap()));
    config.home_overrides.insert(Vendor::Chatgpt, codex_home.path().to_path_buf());
    let shutdown = CancellationToken::new();
    let runner = tokio::spawn(super::run(config, shutdown.clone()));

    let (stream, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept()).await.unwrap().unwrap();
    let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
    let hello = tokio::time::timeout(Duration::from_secs(30), next_frame(&mut ws)).await.unwrap();
    println!("hello: {hello}");
    assert_eq!(hello["vendors"]["claude"], "signed_in", "claude must be signed in on this Mac");

    send(&mut ws, json!({"type": "ping", "id": "ping-1"})).await;
    let pong = next_frame(&mut ws).await;
    println!("pong: {pong}");
    assert_eq!(pong, json!({"type": "pong", "id": "ping-1"}));

    send(&mut ws, json!({"type": "login_start", "id": "r_login", "vendor": "chatgpt"})).await;
    let view = tokio::time::timeout(Duration::from_secs(30), next_frame(&mut ws)).await.unwrap();
    println!("login_view: {view}");
    assert_eq!(view["kind"], "device_code", "{view}");
    assert!(view["code"].as_str().is_some_and(|c| c.contains('-')), "{view}");

    send(&mut ws, json!({"type": "turn", "id": "t_live", "vendor": "claude", "model": "haiku",
        "messages": [{"role": "user", "content": "Reply with exactly: brain runner ok"}]})).await;
    let mut text = String::new();
    let end = loop {
        let frame = tokio::time::timeout(Duration::from_secs(120), next_frame(&mut ws)).await.unwrap();
        println!("frame: {frame}");
        match frame["type"].as_str() {
            Some("turn_delta") => text.push_str(frame["text"].as_str().unwrap()),
            _ => break frame,
        }
    };
    println!("turn text: {text:?}");
    assert_eq!(end["type"], "turn_done", "{end}");
    assert!(text.to_lowercase().contains("brain runner ok"), "{text}");

    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), runner).await.unwrap().unwrap();
}

/// Live check (dev Mac, not CI): kimi and grok sign-in starts through the
/// fake socket (throwaway homes; the codes just expire, nothing is
/// completed), then one tools-off grok turn when `grok` is signed in in
/// `$HOME`. Run with
/// `cargo test -p allternit-api --lib brain_runner_live_kimi_grok -- --ignored --nocapture`.
#[cfg(unix)]
#[tokio::test]
#[ignore = "uses the real kimi and grok CLIs and the network"]
async fn brain_runner_live_kimi_grok_login_start() {
    async fn start(config: Config) -> (Ws, CancellationToken, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = config;
        config.socket_url = format!("ws://{}/api/v1/brain-runner/socket", listener.local_addr().unwrap());
        let shutdown = CancellationToken::new();
        let runner = tokio::spawn(super::run(config, shutdown.clone()));
        let (stream, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept()).await.unwrap().unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let hello = tokio::time::timeout(Duration::from_secs(30), next_frame(&mut ws)).await.unwrap();
        println!("hello: {hello}");
        (ws, shutdown, runner)
    }
    let runner_home = tempfile::tempdir().unwrap();
    let mut config = cfg(runner_home.path());
    config.vendors = vec![Vendor::Kimi, Vendor::Grok];
    config.turn_timeout = Duration::from_secs(120);
    let (mut ws, shutdown, runner) = start(config).await;
    for (id, vendor) in [("r_kimi", "kimi"), ("r_grok", "grok")] {
        send(&mut ws, json!({"type": "login_start", "id": id, "vendor": vendor})).await;
        let view = tokio::time::timeout(Duration::from_secs(30), next_frame(&mut ws)).await.unwrap();
        println!("login_view {vendor}: {view}");
        assert_eq!(view["type"], "login_view", "{view}");
        assert!(view["url"].as_str().is_some_and(|u| u.starts_with("https://")), "{view}");
    }
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), runner).await.unwrap().unwrap();

    // Grok turn against the signed-in grok in $HOME (an override: never deleted).
    let mut config = cfg(runner_home.path());
    config.vendors = vec![Vendor::Grok];
    config.turn_timeout = Duration::from_secs(120);
    config.home_overrides.insert(Vendor::Grok, PathBuf::from(std::env::var("HOME").unwrap()));
    let (mut ws, shutdown, runner) = start(config).await;
    send(&mut ws, json!({"type": "turn", "id": "t_grok", "vendor": "grok",
        "messages": [{"role": "user", "content": "Reply with exactly: brain runner ok"}]})).await;
    let mut text = String::new();
    let end = loop {
        let frame = tokio::time::timeout(Duration::from_secs(120), next_frame(&mut ws)).await.unwrap();
        println!("grok frame: {frame}");
        match frame["type"].as_str() {
            Some("turn_delta") => text.push_str(frame["text"].as_str().unwrap()),
            _ => break frame,
        }
    };
    println!("grok turn text: {text:?} end: {end}");
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), runner).await.unwrap().unwrap();
}
