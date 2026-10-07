//! A bot another computer runs, mirrored in this terminal (bots on another
//! computer, phase 3).
//!
//! `agents attach <bot@team>` on a remote bot, and each remote bot's pane on
//! `agents wall <team>`, run this. The bot's screen arrives as the engine's
//! `screen` / `gone` events (its peer `stream`, carried by allternit-api over
//! the mesh) and keystrokes go back as `input` batches: the same contract the
//! Desktop and web terminals use for a local pane. Ctrl-] leaves.

use std::io::{BufRead, BufReader, Read, Write};
use std::time::Duration;

use allternit_factory_engine::agents::team_apply::{ApiClient, FactoryApi};
use serde_json::{json, Value};

/// Ctrl-]: leave the mirror (the bot keeps running).
const DETACH: u8 = 0x1d;
/// Stream breaks (not a `gone`) tolerated in a row before giving up.
const MAX_RETRIES: u32 = 5;

/// One `input` batch: printable runs are `text`, control bytes and escape
/// sequences named `keys`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Batch {
    Text(String),
    Keys(Vec<String>),
}

fn named(seq: &str) -> Option<&'static str> {
    Some(match seq {
        "\r" | "\n" => "enter",
        "\t" => "tab",
        "\u{7f}" | "\u{8}" => "backspace",
        "\u{1b}" => "esc",
        "\u{1b}[A" | "\u{1b}OA" => "up",
        "\u{1b}[B" | "\u{1b}OB" => "down",
        "\u{1b}[C" | "\u{1b}OC" => "right",
        "\u{1b}[D" | "\u{1b}OD" => "left",
        "\u{1b}[Z" => "shift+tab",
        _ => return None,
    })
}

/// Terminal input bytes as pane input batches, in order (the web mirror's
/// `terminalDataToPaneInput`). Unknown escape sequences are dropped rather
/// than typed as garbage.
pub fn to_batches(data: &str) -> Vec<Batch> {
    let mut out: Vec<Batch> = Vec::new();
    let mut text = String::new();
    fn push_key(out: &mut Vec<Batch>, text: &mut String, key: String) {
        if !text.is_empty() {
            out.push(Batch::Text(std::mem::take(text)));
        }
        match out.last_mut() {
            Some(Batch::Keys(keys)) => keys.push(key),
            _ => out.push(Batch::Keys(vec![key])),
        }
    }
    let chars: Vec<char> = data.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '\u{1b}' {
            let three: String = chars[i..chars.len().min(i + 3)].iter().collect();
            if let Some(k) = named(&three).filter(|_| three.chars().count() == 3) {
                push_key(&mut out, &mut text, k.to_string());
                i += 3;
                continue;
            }
            // CSI with parameters (ESC [ 1 ; 5 A): skip the whole sequence.
            if chars.get(i + 1) == Some(&'[') {
                let mut j = i + 2;
                while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == ';') {
                    j += 1;
                }
                if j < chars.len() && (chars[j].is_ascii_alphabetic() || chars[j] == '~') {
                    if !text.is_empty() {
                        out.push(Batch::Text(std::mem::take(&mut text)));
                    }
                    i = j + 1;
                    continue;
                }
            }
            match chars.get(i + 1) {
                Some(&next) if next >= ' ' && next != '\u{7f}' => {
                    push_key(&mut out, &mut text, format!("alt+{next}"));
                    i += 2;
                }
                _ => {
                    push_key(&mut out, &mut text, "esc".into());
                    i += 1;
                }
            }
            continue;
        }
        if let Some(k) = named(&ch.to_string()) {
            push_key(&mut out, &mut text, k.to_string());
        } else if (ch as u32) < 32 {
            push_key(&mut out, &mut text, format!("ctrl+{}", char::from_u32(ch as u32 + 96).unwrap_or('?')));
        } else {
            text.push(ch);
        }
        i += 1;
    }
    if !text.is_empty() {
        out.push(Batch::Text(text));
    }
    out
}

/// A screen as terminal output: home, each line cleared to its end, the rest
/// of the screen cleared (no full clear, so it doesn't flicker).
pub fn frame(ansi: &str) -> String {
    let body = ansi.replace("\r\n", "\n").replace('\n', "\x1b[K\r\n");
    format!("\x1b[H{body}\x1b[K\x1b[J")
}

/// One server-sent event: `(event, data)`.
fn next_event(lines: &mut impl Iterator<Item = std::io::Result<String>>) -> Option<(String, String)> {
    let (mut event, mut data) = (String::from("message"), String::new());
    for line in lines {
        let line = line.ok()?;
        if line.is_empty() {
            if !data.is_empty() {
                return Some((event, data));
            }
            continue;
        }
        if let Some(v) = line.strip_prefix("event:") {
            event = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(v.strip_prefix(' ').unwrap_or(v));
        }
    }
    None
}

struct RawMode;

impl RawMode {
    fn on() -> Option<Self> {
        crossterm::terminal::enable_raw_mode().ok().map(|_| RawMode)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

fn leave(msg: &str) -> ! {
    let _ = crossterm::terminal::disable_raw_mode();
    eprintln!("\r\n{msg}");
    std::process::exit(0);
}

/// Mirror `address` (`bot@team`) on `computer` until the bot is gone or the
/// person presses Ctrl-]. `Err` carries why it could not.
pub fn run(api: ApiClient, computer: &str, computer_name: &str, address: &str) -> Result<(), String> {
    let to = address.replace('@', "%40");
    // Check the bot is reachable before taking over the terminal.
    let mut resp = api.peer_stream(computer, &format!("stream?to={to}")).map_err(|e| e.fact)?;
    let _raw = RawMode::on();
    print!("\x1b[2J\x1b[HMirroring {address} on {computer_name}. Ctrl-] leaves; the bot keeps running.\r\n");
    let _ = std::io::stdout().flush();

    let input_api = api.clone();
    let (input_computer, input_to) = (computer.to_string(), address.to_string());
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 1024];
        loop {
            let n = match stdin.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            let chunk = &buf[..n];
            let (chunk, detach) = match chunk.iter().position(|b| *b == DETACH) {
                Some(p) => (&chunk[..p], true),
                None => (chunk, false),
            };
            for batch in to_batches(&String::from_utf8_lossy(chunk)) {
                let body = match batch {
                    Batch::Text(text) => json!({ "to": input_to, "text": text }),
                    Batch::Keys(keys) => json!({ "to": input_to, "keys": keys }),
                };
                let _ = input_api.peer_call(&input_computer, "POST", "input", Some(body));
            }
            if detach {
                leave(&format!("Left the mirror of {input_to}; it keeps running."));
            }
        }
    });

    let mut retries = 0;
    loop {
        let mut lines = BufReader::new(&mut resp).lines();
        while let Some((event, data)) = next_event(&mut lines) {
            retries = 0;
            let v: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
            match event.as_str() {
                "screen" => {
                    let mut out = std::io::stdout();
                    let _ = out.write_all(frame(v["ansi"].as_str().unwrap_or_default()).as_bytes());
                    let _ = out.flush();
                }
                "gone" => {
                    let reason = v["reason"].as_str().unwrap_or("the pane closed");
                    leave(&format!("{address} on {computer_name} is gone: {reason}"));
                }
                _ => {}
            }
        }
        // The stream broke without a `gone` (the mesh, a restart): reopen.
        retries += 1;
        if retries > MAX_RETRIES {
            return Err(format!("lost the stream from {computer_name} {MAX_RETRIES} times in a row"));
        }
        std::thread::sleep(Duration::from_secs(2));
        resp = match api.peer_stream(computer, &format!("stream?to={to}")) {
            Ok(r) => r,
            Err(e) if e.code == "not_found" => return Err(e.fact),
            Err(_) => continue,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_bytes_become_text_and_named_keys_in_order() {
        assert_eq!(
            to_batches("ls\r\u{1b}[A\u{3}x"),
            vec![
                Batch::Text("ls".into()),
                Batch::Keys(vec!["enter".into(), "up".into(), "ctrl+c".into()]),
                Batch::Text("x".into()),
            ]
        );
        assert_eq!(to_batches("\u{1b}[1;5A"), vec![]);
        assert_eq!(to_batches("\u{1b}b"), vec![Batch::Keys(vec!["alt+b".into()])]);
        assert_eq!(to_batches("\u{1b}"), vec![Batch::Keys(vec!["esc".into()])]);
        assert_eq!(to_batches("\u{7f}\t"), vec![Batch::Keys(vec!["backspace".into(), "tab".into()])]);
        assert_eq!(to_batches("héllo"), vec![Batch::Text("héllo".into())]);
    }

    #[test]
    fn frames_redraw_from_home_without_a_full_clear() {
        assert_eq!(frame("a\nb"), "\x1b[Ha\x1b[K\r\nb\x1b[K\x1b[J");
    }

    #[test]
    fn events_parse_with_keepalives_between() {
        let raw = ":\n\nevent: screen\ndata: {\"ansi\":\"hi\"}\n\nevent: gone\ndata: {\"reason\":\"x\"}\n\n";
        let mut lines = raw.lines().map(|l| Ok(l.to_string()));
        assert_eq!(next_event(&mut lines), Some(("screen".into(), "{\"ansi\":\"hi\"}".into())));
        assert_eq!(next_event(&mut lines), Some(("gone".into(), "{\"reason\":\"x\"}".into())));
        assert_eq!(next_event(&mut lines), None);
    }
}
