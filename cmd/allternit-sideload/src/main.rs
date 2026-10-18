//! allternit-sideload: signs the Phone Link iPhone helper (iCTRL XCUITest
//! runner + host app) with the user's own Apple ID, no Xcode. Allternit Desktop
//! runs it (phone-ios-runner.ts `SideloadSigner`) and installs the result with
//! go-ios; this program never talks to the phone.
//!
//!   allternit-sideload --json [--anisette-url URL]
//!
//! One sign per process. Requests on stdin, events on stdout, JSON lines; see
//! protocol.rs. Nothing secret is read from argv or the environment, and
//! nothing is logged (stderr stays quiet).

mod bundle;
mod bundle_ids;
mod errors;
mod mac_anisette;
mod protocol;
mod session;
mod signer;
mod storage;

use std::io::Write as _;
use std::process::ExitCode;
use std::sync::Arc;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, Mutex, Notify};
use zeroize::Zeroizing;

use crate::errors::{ErrorCode, Failure};
use crate::protocol::{parse_request, Event, Request, TwoFactorInput};
use crate::signer::{Emit, Signer};

/// Allternit-hosted anisette v3 server (live since 2026-10-17; README
/// "Anisette"). isideload's own default is a third party's
/// (`isideload::anisette::remote_v3::DEFAULT_ANISETTE_V3_URL`).
pub const DEFAULT_ANISETTE_URL: &str = "https://anisette.allternit.com";
pub const ANISETTE_URL_ENV: &str = "ALLTERNIT_ANISETTE_URL";

const MAX_LINE: usize = 1 << 20;

#[derive(Debug, PartialEq)]
struct Args {
    anisette_url: Option<String>,
}

#[derive(Debug, PartialEq)]
enum Cli {
    Run(Args),
    Help,
    Version,
}

fn parse_args(args: &[String]) -> Result<Cli, String> {
    let mut out = Args { anisette_url: None };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--json" => {}
            "--anisette-url" => out.anisette_url = Some(it.next().ok_or("--anisette-url needs a value")?.clone()),
            s if s.starts_with("--anisette-url=") => out.anisette_url = Some(s["--anisette-url=".len()..].to_string()),
            "-h" | "--help" => return Ok(Cli::Help),
            "-V" | "--version" => return Ok(Cli::Version),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(Cli::Run(out))
}

/// Request field > --anisette-url > $ALLTERNIT_ANISETTE_URL > default.
fn resolve_anisette_url(from_request: Option<&str>, from_flag: Option<&str>, from_env: Option<&str>) -> String {
    from_request
        .or(from_flag)
        .or(from_env)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(DEFAULT_ANISETTE_URL)
        .trim_end_matches('/')
        .to_string()
}

const HELP: &str = "allternit-sideload --json [--anisette-url URL]\n\
Signs the Allternit Phone Link iPhone helper with your Apple ID.\n\
Speaks JSON lines on stdin/stdout; see cmd/allternit-sideload/README.md.\n";

/// One line, in a buffer that is zeroed when dropped (it may hold a password).
async fn read_line<R: AsyncBufRead + Unpin>(reader: &mut R) -> Option<Zeroizing<String>> {
    // Pre-sized so the line (and the password in it) isn't left behind in
    // reallocated buffers.
    let mut line = Zeroizing::new(String::with_capacity(64 * 1024));
    match reader.read_line(&mut *line).await {
        Ok(0) | Err(_) => None,
        Ok(_) if line.len() > MAX_LINE => None,
        Ok(_) => Some(line),
    }
}

fn stdout_emitter() -> Emit {
    let out = Arc::new(std::sync::Mutex::new(std::io::stdout()));
    Arc::new(move |event: Event| {
        let line = event.to_line();
        if let Ok(mut out) = out.lock() {
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        }
    })
}

#[tokio::main]
async fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(Cli::Run(a)) => a,
        Ok(Cli::Help) => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Ok(Cli::Version) => {
            println!("allternit-sideload {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("allternit-sideload: {e}\n{HELP}");
            return ExitCode::from(2);
        }
    };
    // isideload's reqwest is built without a TLS crypto provider.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let emit = stdout_emitter();
    let mut reader = BufReader::new(tokio::io::stdin());

    let request = match read_line(&mut reader).await.map(|line| parse_request(&line)) {
        Some(Ok(Request::Sign(req))) => req,
        Some(Ok(_)) => return fail(&emit, Failure::new(ErrorCode::InvalidRequest, "The first request must be op \"sign\".")),
        Some(Err(f)) => return fail(&emit, f),
        None => return fail(&emit, Failure::new(ErrorCode::InvalidRequest, "No request on stdin.")),
    };

    let (codes_tx, codes_rx) = mpsc::channel(4);
    let cancelled = Arc::new(Notify::new());
    {
        let cancelled = cancelled.clone();
        let emit = emit.clone();
        tokio::spawn(async move {
            loop {
                let Some(line) = read_line(&mut reader).await else {
                    // Desktop went away (or closed stdin on purpose): stop.
                    cancelled.notify_one();
                    return;
                };
                match parse_request(&line) {
                    Ok(Request::TwoFactor { code }) => {
                        if codes_tx.send(TwoFactorInput::Code(code)).await.is_err() {
                            return;
                        }
                    }
                    Ok(Request::TwoFactorSms) => {
                        if codes_tx.send(TwoFactorInput::Text).await.is_err() {
                            return;
                        }
                    }
                    Ok(Request::Cancel) => {
                        cancelled.notify_one();
                        return;
                    }
                    Ok(Request::Sign(_)) => {} // one sign per process; ignore
                    Err(f) if f.code == ErrorCode::TwoFactorFailed => {
                        // Malformed code: ask again rather than ending the sign.
                        emit(Event::TwoFactorRequired { sms: false, phone: None, retry_reason: Some(f.message) });
                    }
                    Err(_) => {}
                }
            }
        });
    }

    let anisette_url = resolve_anisette_url(
        request.anisette_url.as_deref(),
        args.anisette_url.as_deref(),
        std::env::var(ANISETTE_URL_ENV).ok().as_deref(),
    );
    if !protocol::is_allowed_anisette_url(&anisette_url) {
        return fail(&emit, Failure::new(ErrorCode::InvalidRequest, "The anisette URL must be https (or http on 127.0.0.1)."));
    }
    let signer = Signer {
        anisette_url,
        emit: emit.clone(),
        codes: Arc::new(Mutex::new(codes_rx)),
        storage: storage::platform_storage,
    };

    tokio::select! {
        result = signer.sign(request) => match result {
            Ok(done) => {
                emit(Event::Done(done));
                ExitCode::SUCCESS
            }
            Err(f) => fail(&emit, f),
        },
        _ = cancelled.notified() => ExitCode::from(130),
    }
}

fn fail(emit: &Emit, f: Failure) -> ExitCode {
    emit(Event::from(f));
    ExitCode::from(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_cli() {
        assert_eq!(parse_args(&strings(&["--json"])).unwrap(), Cli::Run(Args { anisette_url: None }));
        assert_eq!(
            parse_args(&strings(&["--json", "--anisette-url", "https://a.example"])).unwrap(),
            Cli::Run(Args { anisette_url: Some("https://a.example".into()) })
        );
        assert_eq!(
            parse_args(&strings(&["--anisette-url=http://127.0.0.1:6969"])).unwrap(),
            Cli::Run(Args { anisette_url: Some("http://127.0.0.1:6969".into()) })
        );
        assert_eq!(parse_args(&strings(&["--version"])).unwrap(), Cli::Version);
        assert!(parse_args(&strings(&["--password", "x"])).is_err());
        assert!(parse_args(&strings(&["--anisette-url"])).is_err());
    }

    #[test]
    fn anisette_url_precedence() {
        assert_eq!(resolve_anisette_url(None, None, None), DEFAULT_ANISETTE_URL);
        assert_eq!(resolve_anisette_url(None, None, Some("https://env.example/")), "https://env.example");
        assert_eq!(resolve_anisette_url(None, Some("https://flag.example"), Some("https://env.example")), "https://flag.example");
        assert_eq!(resolve_anisette_url(Some("https://req.example"), Some("https://flag.example"), None), "https://req.example");
        assert_eq!(resolve_anisette_url(Some(""), None, None), DEFAULT_ANISETTE_URL);
    }

    #[tokio::test]
    async fn reads_lines_into_zeroizing_buffers() {
        let input: &[u8] = b"{\"op\":\"cancel\"}\nsecond\n";
        let mut r = BufReader::new(input);
        assert_eq!(read_line(&mut r).await.unwrap().as_str(), "{\"op\":\"cancel\"}\n");
        assert_eq!(read_line(&mut r).await.unwrap().as_str(), "second\n");
        assert!(read_line(&mut r).await.is_none());
    }

    #[tokio::test]
    async fn two_factor_codes_flow_from_stdin_to_the_signer() {
        // The channel the reader task feeds and the 2FA callback drains.
        let (tx, rx) = mpsc::channel(4);
        let rx = Arc::new(Mutex::new(rx));
        let Request::TwoFactor { code } = parse_request(r#"{"op":"two_factor","code":"654321"}"#).unwrap() else { panic!() };
        tx.send(TwoFactorInput::Code(code)).await.unwrap();
        let Some(TwoFactorInput::Code(got)) = rx.lock().await.recv().await else { panic!() };
        assert_eq!(got.expose(), "654321");
        assert!(matches!(parse_request(r#"{"op":"two_factor_sms"}"#).unwrap(), Request::TwoFactorSms));
    }
}
