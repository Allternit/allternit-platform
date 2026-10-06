//! Raw terminals on the pane engine (an Allternit addition beside the herdr
//! code, like `factory_backend` and `factory_host`).
//!
//! A Desktop / web terminal tile, a Gizzi PTY and a Factory agent pane are the
//! same thing: a pane in the pane engine. This module is what lets a client
//! that draws its own terminal (xterm.js in the app, Gizzi's PTY websocket)
//! drive one:
//!
//! - **Tap.** A pane spawned with [`TERMINAL_ENV_VAR`] in its launch env gets
//!   a tap keyed by that terminal id. The pane's PTY reader pushes every raw
//!   output byte into the tap (a bounded ring for replay, plus a wakeup for
//!   live readers) and the child watcher records the exit code. The env var
//!   is stripped from the child's environment.
//! - **Socket methods** (`factory.terminal.*`, hidden from the herdr schema):
//!   `create`, `write` (raw bytes, no bracketed paste), `resize` (absolute
//!   PTY size, held against layout passes), `close`, `get`, `list`, and the
//!   streaming `output` (ring replay, then live bytes, then one `exit` frame).
//! - **CLI.** `allternit-factory pane tty …` for shells and guest images
//!   (`ensure` starts the engine and prints its socket).
//!
//! Terminals live in the pane engine daemon, so they survive UI reconnects and
//! restarts of whatever process drew them (allternit-api, `gizzi serve`). They
//! show in the agent wall as workspaces labeled `term-<id>`, and an agent
//! started in one is detected like any other pane.

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::layout::PaneId;

/// Launch-env key carrying a pane's terminal id. Spawn-time configuration:
/// stripped from the child's environment (see `apply_pane_launch_env`).
pub(crate) const TERMINAL_ENV_VAR: &str = "ALLTERNIT_FACTORY_TERMINAL_ID";

/// Workspace label prefix for terminals created without a label.
pub const LABEL_PREFIX: &str = "term-";

/// Output kept per terminal for replay to a reconnecting client.
pub const RING_BYTES: usize = 2 * 1024 * 1024;

/// How long an exited terminal's tap (its scrollback and exit code) is kept
/// for a client to read before it is dropped.
const EXITED_TTL: Duration = Duration::from_secs(10 * 60);

/// How often a live `output` stream checks for a closed client.
const STREAM_POLL: Duration = Duration::from_millis(100);

/// Largest single `output` frame (bytes of terminal output).
const STREAM_FRAME_BYTES: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// Tap registry
// ---------------------------------------------------------------------------

struct TapState {
    ring: VecDeque<u8>,
    /// Absolute offset of the byte after the newest one (total bytes seen).
    end: u64,
    exit_code: Option<i32>,
    exited_at: Option<Instant>,
}

/// One terminal's output tap.
pub(crate) struct Tap {
    id: String,
    pane_raw: u32,
    pid: AtomicU32,
    viewers: AtomicUsize,
    state: Mutex<TapState>,
    wake: Condvar,
}

impl Tap {
    fn lock(&self) -> std::sync::MutexGuard<'_, TapState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(crate) fn pane_id(&self) -> PaneId {
        PaneId::from_raw(self.pane_raw)
    }

    /// Raw PTY output (called from the pane's reader thread).
    pub(crate) fn push(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut st = self.lock();
        st.end += bytes.len() as u64;
        if bytes.len() >= RING_BYTES {
            st.ring.clear();
            st.ring.extend(&bytes[bytes.len() - RING_BYTES..]);
        } else {
            let overflow = (st.ring.len() + bytes.len()).saturating_sub(RING_BYTES);
            st.ring.drain(..overflow);
            st.ring.extend(bytes);
        }
        drop(st);
        self.wake.notify_all();
    }

    pub(crate) fn set_pid(&self, pid: u32) {
        self.pid.store(pid, Ordering::Release);
    }

    /// The child exited (`None`: the wait failed, the code is unknown).
    pub(crate) fn exited(&self, code: Option<i32>) {
        let mut st = self.lock();
        if st.exited_at.is_none() {
            st.exit_code = code;
            st.exited_at = Some(Instant::now());
        }
        drop(st);
        self.wake.notify_all();
    }

    /// While a client draws this terminal itself, it answers the terminal's
    /// queries (device attributes, cursor position); the pane engine's own
    /// emulator must not answer as well, or the program reads two replies.
    pub(crate) fn has_viewers(&self) -> bool {
        self.viewers.load(Ordering::Acquire) > 0
    }

    fn running(&self) -> bool {
        self.lock().exited_at.is_none()
    }

    /// Bytes from absolute offset `from` (clamped to what the ring holds),
    /// the offset after them, and the exit code once exited and drained.
    fn read_from(&self, from: u64, max: usize) -> (Vec<u8>, u64, Option<Option<i32>>) {
        let st = self.lock();
        let start = st.end - st.ring.len() as u64;
        let from = from.clamp(start, st.end);
        let skip = (from - start) as usize;
        let take = (st.ring.len() - skip).min(max);
        let bytes: Vec<u8> = st.ring.iter().skip(skip).take(take).copied().collect();
        let next = from + take as u64;
        let exit = (next == st.end && st.exited_at.is_some()).then_some(st.exit_code);
        (bytes, next, exit)
    }

    /// Facts for `get`/`list`/`create`. `engine_terminal` is the pane
    /// engine's own terminal id (what `pane terminal attach` takes).
    pub(crate) fn info(
        &self,
        pane_id: Option<String>,
        workspace_id: Option<String>,
        engine_terminal: Option<String>,
    ) -> Value {
        let st = self.lock();
        json!({
            "terminal_id": self.id,
            "pane_id": pane_id,
            "workspace_id": workspace_id,
            "engine_terminal": engine_terminal,
            "running": st.exited_at.is_none(),
            "exit_code": st.exit_code,
            "pid": match self.pid.load(Ordering::Acquire) { 0 => None, pid => Some(pid) },
            "output_bytes": st.end,
        })
    }
}

fn registry() -> &'static Mutex<HashMap<String, Arc<Tap>>> {
    static TAPS: OnceLock<Mutex<HashMap<String, Arc<Tap>>>> = OnceLock::new();
    TAPS.get_or_init(Default::default)
}

fn taps() -> std::sync::MutexGuard<'static, HashMap<String, Arc<Tap>>> {
    let mut map = registry().lock().unwrap_or_else(|p| p.into_inner());
    // Drop exited terminals nobody read back within the TTL.
    map.retain(|_, tap| {
        tap.lock()
            .exited_at
            .is_none_or(|at| at.elapsed() < EXITED_TTL)
    });
    map
}

/// Registers a tap when the launch env names a terminal (called at spawn).
pub(crate) fn tap_from_launch_env(extra: &[(String, String)], pane_id: PaneId) -> Option<Arc<Tap>> {
    let id = extra
        .iter()
        .find(|(key, _)| key == TERMINAL_ENV_VAR)
        .map(|(_, value)| value.clone())?;
    let tap = Arc::new(Tap {
        id: id.clone(),
        pane_raw: pane_id.raw(),
        pid: AtomicU32::new(0),
        viewers: AtomicUsize::new(0),
        state: Mutex::new(TapState { ring: VecDeque::new(), end: 0, exit_code: None, exited_at: None }),
        wake: Condvar::new(),
    });
    taps().insert(id, tap.clone());
    Some(tap)
}

pub(crate) fn get(id: &str) -> Option<Arc<Tap>> {
    taps().get(id).cloned()
}

pub(crate) fn remove(id: &str) -> Option<Arc<Tap>> {
    let tap = taps().remove(id);
    if let Some(tap) = &tap {
        // Wake streams so they notice the terminal is gone.
        tap.exited(None);
    }
    tap
}

pub(crate) fn all() -> Vec<Arc<Tap>> {
    let mut list: Vec<_> = taps().values().cloned().collect();
    list.sort_by(|a, b| a.id.cmp(&b.id));
    list
}

/// Terminal ids are chosen by the client: 1–64 of `[A-Za-z0-9_-]`.
pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

// ---------------------------------------------------------------------------
// Socket method params (hidden `factory.terminal.*` methods)
// ---------------------------------------------------------------------------

fn default_cols() -> u16 {
    80
}

fn default_rows() -> u16 {
    24
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FactoryTerminalCreateParams {
    pub terminal_id: String,
    /// Workspace label in the agent wall (default `term-<id>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// argv; empty runs the pane engine's default shell.
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FactoryTerminalTarget {
    pub terminal_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FactoryTerminalWriteParams {
    pub terminal_id: String,
    /// Raw bytes for the PTY, as typed (`\r` is Enter). Not a paste.
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FactoryTerminalResizeParams {
    pub terminal_id: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FactoryTerminalOutputParams {
    pub terminal_id: String,
    /// Replay the kept scrollback first (default true).
    #[serde(default = "default_true")]
    pub replay: bool,
    /// Keep streaming live output until the terminal exits (default true).
    /// `false` returns the replay and closes: a read.
    #[serde(default = "default_true")]
    pub follow: bool,
}

pub(crate) fn success(id: &str, result: Value) -> String {
    json!({ "id": id, "result": result }).to_string()
}

pub(crate) fn failure(id: &str, code: &str, message: impl Into<String>) -> String {
    json!({ "id": id, "error": { "code": code, "message": message.into() } }).to_string()
}

pub(crate) fn not_found(id: &str, terminal_id: &str) -> String {
    failure(id, "terminal_not_found", format!("terminal {terminal_id} not found"))
}

// ---------------------------------------------------------------------------
// The `output` stream (served on the socket thread, no app round trip)
// ---------------------------------------------------------------------------

/// UTF-8 text for a frame, carrying an incomplete trailing sequence over to
/// the next frame so a multi-byte character split across reads stays whole.
fn take_text(carry: &mut Vec<u8>, bytes: &[u8]) -> String {
    carry.extend_from_slice(bytes);
    let cut = match std::str::from_utf8(carry) {
        Ok(_) => carry.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => carry.len(),
    };
    let rest = carry.split_off(cut);
    let text = String::from_utf8_lossy(carry).into_owned();
    *carry = rest;
    text
}

struct ViewerGuard(Arc<Tap>);

impl Drop for ViewerGuard {
    fn drop(&mut self) {
        self.0.viewers.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Serves `factory.terminal.output` on `stream`: a response line
/// `{id, result:{terminal_id, replay_bytes}}`, then `{"type":"output","data"}`
/// lines (with `follow`, one `{"type":"live"}` line marks the end of the
/// replay), then one `{"type":"exit","exit_code"}` line when the terminal
/// exits.
pub(crate) fn serve_output<S: Write>(
    stream: &mut S,
    request_id: &str,
    params: FactoryTerminalOutputParams,
    running: &AtomicBool,
    mut peer_closed: impl FnMut(&mut S) -> std::io::Result<bool>,
) -> std::io::Result<()> {
    let Some(tap) = get(&params.terminal_id) else {
        return write_line(stream, &not_found(request_id, &params.terminal_id));
    };
    let mut offset = if params.replay { 0 } else { tap.lock().end };
    let replay_end = tap.lock().end;
    write_line(
        stream,
        &success(request_id, json!({ "terminal_id": tap.id, "replay_bytes": replay_end.saturating_sub(offset) })),
    )?;
    let _viewer = params.follow.then(|| {
        tap.viewers.fetch_add(1, Ordering::AcqRel);
        ViewerGuard(tap.clone())
    });
    let mut carry = Vec::new();
    let mut live = !params.follow;
    loop {
        if !live && offset >= replay_end {
            live = true;
            write_line(stream, r#"{"type":"live"}"#)?;
        }
        let (bytes, next, exit) = tap.read_from(offset, STREAM_FRAME_BYTES);
        offset = next;
        if !bytes.is_empty() {
            let mut slice = bytes.as_slice();
            // With nothing carried, leading continuation bytes can only be the
            // ring's cut through an old character: skip to a boundary.
            if carry.is_empty() {
                while let [first, rest @ ..] = slice {
                    if (first & 0b1100_0000) != 0b1000_0000 {
                        break;
                    }
                    slice = rest;
                }
            }
            let text = take_text(&mut carry, slice);
            if !text.is_empty() {
                write_line(stream, &json!({ "type": "output", "data": text }).to_string())?;
            }
            continue;
        }
        if let Some(code) = exit {
            return write_line(stream, &json!({ "type": "exit", "exit_code": code }).to_string());
        }
        if !params.follow && offset >= replay_end {
            return Ok(());
        }
        if !running.load(Ordering::Relaxed) || peer_closed(stream)? {
            return Ok(());
        }
        let st = tap.lock();
        if st.end == offset && st.exited_at.is_none() {
            let _ = tap.wake.wait_timeout(st, STREAM_POLL);
        }
    }
}

fn write_line<S: Write>(stream: &mut S, line: &str) -> std::io::Result<()> {
    let result = (|| {
        stream.write_all(line.as_bytes())?;
        stream.write_all(b"\n")?;
        stream.flush()
    })();
    match result {
        Err(e) if crate::ipc::is_connection_closed_error(&e) => Ok(()),
        other => other,
    }
}

pub(crate) fn is_running(id: &str) -> bool {
    get(id).is_some_and(|t| t.running())
}

// ---------------------------------------------------------------------------
// CLI: `allternit-factory pane tty …`
// ---------------------------------------------------------------------------

const CLI_USAGE: &str = "\
usage: allternit-factory pane tty <command>

  ensure                         start the pane engine if needed; print {\"socket\": …}
  create <id> [--cwd DIR] [--cols N] [--rows N] [--label L] [--env K=V]… [-- argv…]
                                 open a terminal (no argv: the default shell)
  write <id> <data> [--enter]    type raw bytes (--enter adds a carriage return)
  read <id>                      print the kept output (scrollback) and exit
  resize <id> <cols> <rows>      set the terminal's size
  attach <id>                    take over this terminal in your own terminal
                                 (detach with ctrl+b q; the shell keeps running)
  close <id>                     close the terminal
  get <id> | list                terminal facts as JSON

Terminals are panes in the agent session's pane engine (workspace label
term-<id>). Every command prints JSON; errors exit 1.";

fn cli_call(method: &str, params: Value) -> std::io::Result<Value> {
    use std::io::{BufRead, BufReader};
    let client = crate::api::client::ApiClient::local();
    let mut stream = client.connect()?;
    let request = json!({ "id": "tty", "method": method, "params": params });
    stream.write_all(format!("{request}\n").as_bytes())?;
    stream.flush()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    serde_json::from_str(line.trim()).map_err(std::io::Error::other)
}

fn cli_print(value: Value) -> i32 {
    if value.get("error").is_some() {
        eprintln!("{}", value["error"]);
        return 1;
    }
    println!("{}", value["result"]);
    0
}

fn cli_read(id: &str) -> std::io::Result<i32> {
    use std::io::{BufRead, BufReader};
    let client = crate::api::client::ApiClient::local();
    let mut stream = client.connect()?;
    let request = json!({ "id": "tty", "method": "factory.terminal.output",
        "params": { "terminal_id": id, "replay": true, "follow": false } });
    stream.write_all(format!("{request}\n").as_bytes())?;
    stream.flush()?;
    let mut out = std::io::stdout().lock();
    for (i, line) in BufReader::new(stream).lines().enumerate() {
        let frame: Value = serde_json::from_str(&line?).map_err(std::io::Error::other)?;
        if i == 0 && frame.get("error").is_some() {
            eprintln!("{}", frame["error"]);
            return Ok(1);
        }
        if frame["type"] == "output" {
            out.write_all(frame["data"].as_str().unwrap_or_default().as_bytes())?;
        }
    }
    out.flush()?;
    Ok(0)
}

/// `allternit-factory pane tty …`.
pub(crate) fn run_cli(args: &[String]) -> std::io::Result<i32> {
    crate::cli::ao::ensure_ao_session();
    let Some(cmd) = args.first().map(String::as_str) else {
        eprintln!("{CLI_USAGE}");
        return Ok(2);
    };
    let rest = &args[1..];
    let usage = || {
        eprintln!("{CLI_USAGE}");
        Ok(2)
    };
    if !matches!(cmd, "help" | "--help" | "-h") {
        crate::cli::ao::ensure_engine_running()?;
    }
    match (cmd, rest) {
        ("ensure", []) => {
            println!("{}", json!({ "socket": crate::api::socket_path() }));
            Ok(0)
        }
        ("create", [id, opts @ ..]) => {
            let mut params = json!({ "terminal_id": id });
            let mut env = serde_json::Map::new();
            let mut it = opts.iter();
            while let Some(opt) = it.next() {
                match opt.as_str() {
                    "--" => {
                        params["command"] = json!(it.by_ref().collect::<Vec<_>>());
                        break;
                    }
                    "--cwd" | "--label" => match it.next() {
                        Some(v) => params[&opt[2..]] = json!(v),
                        None => return usage(),
                    },
                    "--cols" | "--rows" => match it.next().and_then(|v| v.parse::<u16>().ok()) {
                        Some(v) => params[&opt[2..]] = json!(v),
                        None => return usage(),
                    },
                    "--env" => match it.next().and_then(|v| v.split_once('=')) {
                        Some((k, v)) => {
                            env.insert(k.to_string(), json!(v));
                        }
                        None => return usage(),
                    },
                    _ => return usage(),
                }
            }
            params["env"] = Value::Object(env);
            Ok(cli_print(cli_call("factory.terminal.create", params)?))
        }
        ("write", [id, data, flags @ ..]) => {
            let enter = match flags {
                [] => false,
                [f] if f == "--enter" => true,
                _ => return usage(),
            };
            let data = if enter { format!("{data}\r") } else { data.clone() };
            Ok(cli_print(cli_call("factory.terminal.write", json!({ "terminal_id": id, "data": data }))?))
        }
        ("read", [id]) => cli_read(id),
        ("attach", [id]) => {
            let got = cli_call("factory.terminal.get", json!({ "terminal_id": id }))?;
            match got["result"]["terminal"]["engine_terminal"].as_str() {
                Some(engine_terminal) => {
                    crate::client::run_terminal_attach(engine_terminal.to_string(), true)?;
                    Ok(0)
                }
                None if got.get("error").is_some() => Ok(cli_print(got)),
                None => {
                    eprintln!("terminal {id} has exited");
                    Ok(1)
                }
            }
        }
        ("resize", [id, cols, rows]) => match (cols.parse::<u16>(), rows.parse::<u16>()) {
            (Ok(cols), Ok(rows)) => Ok(cli_print(cli_call(
                "factory.terminal.resize",
                json!({ "terminal_id": id, "cols": cols, "rows": rows }),
            )?)),
            _ => usage(),
        },
        ("close", [id]) => Ok(cli_print(cli_call("factory.terminal.close", json!({ "terminal_id": id }))?)),
        ("get", [id]) => Ok(cli_print(cli_call("factory.terminal.get", json!({ "terminal_id": id }))?)),
        ("list", []) => Ok(cli_print(cli_call("factory.terminal.list", json!({}))?)),
        ("help" | "--help" | "-h", _) => {
            println!("{CLI_USAGE}");
            Ok(0)
        }
        _ => usage(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap(id: &str) -> Arc<Tap> {
        tap_from_launch_env(&[(TERMINAL_ENV_VAR.to_string(), id.to_string())], PaneId::from_raw(9999)).unwrap()
    }

    #[test]
    fn ring_keeps_the_newest_bytes() {
        let t = tap("ring-test");
        t.push(&vec![b'a'; RING_BYTES]);
        t.push(b"xyz");
        let (bytes, next, exit) = t.read_from(0, usize::MAX);
        assert_eq!(bytes.len(), RING_BYTES);
        assert!(bytes.ends_with(b"aaxyz"));
        assert_eq!(next, RING_BYTES as u64 + 3);
        assert_eq!(exit, None);
        remove("ring-test");
    }

    #[test]
    fn exit_is_reported_after_the_output_is_drained() {
        let t = tap("exit-test");
        t.push(b"bye\r\n");
        t.exited(Some(3));
        let (bytes, next, exit) = t.read_from(0, 2);
        assert_eq!(bytes, b"by");
        assert_eq!(exit, None);
        let (_, _, exit) = t.read_from(next, usize::MAX);
        assert_eq!(exit, Some(Some(3)));
        remove("exit-test");
    }

    #[test]
    fn output_stream_replays_then_reports_exit() {
        let t = tap("stream-test");
        t.push("héllo ".as_bytes());
        t.push(&"wörld".as_bytes()[..2]);
        t.push(&"wörld".as_bytes()[2..]);
        t.exited(Some(0));
        let mut out = Vec::new();
        let running = AtomicBool::new(true);
        serve_output(
            &mut out,
            "r1",
            FactoryTerminalOutputParams { terminal_id: "stream-test".into(), replay: true, follow: true },
            &running,
            |_| Ok(false),
        )
        .unwrap();
        let lines: Vec<Value> =
            String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines[0]["id"], "r1");
        let text: String = lines.iter().filter(|l| l["type"] == "output").map(|l| l["data"].as_str().unwrap()).collect();
        assert_eq!(text, "héllo wörld");
        let live = lines.iter().position(|l| l["type"] == "live").expect("live marker");
        assert!(lines[1..live].iter().all(|l| l["type"] == "output"));
        assert_eq!(lines.last().unwrap()["type"], "exit");
        assert_eq!(lines.last().unwrap()["exit_code"], 0);
        remove("stream-test");
    }

    #[test]
    fn text_frames_keep_split_utf8_whole() {
        let mut carry = Vec::new();
        let bytes = "é".as_bytes();
        assert_eq!(take_text(&mut carry, &bytes[..1]), "");
        assert_eq!(take_text(&mut carry, &bytes[1..]), "é");
    }

    #[test]
    fn ids_are_validated() {
        assert!(valid_id("abc-123_X"));
        assert!(!valid_id(""));
        assert!(!valid_id("a/b"));
        assert!(!valid_id(&"a".repeat(65)));
    }
}
