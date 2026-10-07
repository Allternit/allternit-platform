//! `allternit-factory pane fabric pair|serve|status` — the Fabric Transport node commands.

use std::io::Write as _;
use std::sync::Arc;

use super::cloud::{CloudClient, HeartbeatError};
use super::identity::NodeIdentity;
use super::{DEFAULT_AO_PORT};
use super::wire::ExchangeOutcome;

const USAGE: &str = "usage: allternit-factory pane fabric pair [--no-browser] [--runtime-type <t>] [--name <n>] [--re-pair]
       allternit-factory pane fabric serve [--port <p>]
       allternit-factory pane fabric status";

/// Default the engine session to `ao` exactly like the other ao contract
/// commands (mirrors `cli::ao::ensure_ao_session`).
fn ensure_ao_session() {
    if crate::session::explicit_session_requested() {
        return;
    }
    if std::env::var_os(crate::api::SOCKET_PATH_ENV_VAR).is_some() {
        return;
    }
    if std::env::var_os(crate::session::SESSION_ENV_VAR).is_none() {
        std::env::set_var(crate::session::SESSION_ENV_VAR, "ao");
    }
}

pub(crate) fn run(args: &[String]) -> std::io::Result<i32> {
    ensure_ao_session();
    let Some(action) = args.first().map(String::as_str) else {
        eprintln!("{USAGE}");
        return Ok(2);
    };
    let rest = &args[1..];
    match action {
        "pair" => pair(rest),
        "serve" => serve(rest),
        "status" => status(rest),
        "--help" | "-h" | "help" => {
            println!("{USAGE}");
            Ok(0)
        }
        _ => {
            eprintln!("{USAGE}");
            Ok(2)
        }
    }
}

// ---------------------------------------------------------------------------
// allternit-factory pane fabric pair
// ---------------------------------------------------------------------------

fn pair(args: &[String]) -> std::io::Result<i32> {
    let mut no_browser = std::env::var_os("ALLTERNIT_FACTORY_FABRIC_NO_BROWSER").is_some();
    let mut runtime_type: Option<String> = None;
    let mut name: Option<String> = None;
    let mut re_pair = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--no-browser" => no_browser = true,
            "--re-pair" => re_pair = true,
            "--runtime-type" => {
                index += 1;
                runtime_type = args.get(index).cloned();
            }
            "--name" => {
                index += 1;
                name = args.get(index).cloned();
            }
            other => {
                eprintln!("unknown flag: {other}\n{USAGE}");
                return Ok(2);
            }
        }
        index += 1;
    }

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("error: cannot start async runtime: {err}");
            return Ok(1);
        }
    };

    let code = runtime.block_on(async_pair(no_browser, runtime_type, name, re_pair));
    Ok(code)
}

async fn async_pair(
    no_browser: bool,
    runtime_type: Option<String>,
    name: Option<String>,
    re_pair: bool,
) -> i32 {
    let cloud = match CloudClient::new() {
        Ok(cloud) => cloud,
        Err(err) => {
            eprintln!("error: {err}");
            return 1;
        }
    };

    let mut identity = match NodeIdentity::load() {
        Some(identity) => identity,
        None => {
            let fresh = NodeIdentity::generate();
            if let Err(err) = fresh.save() {
                eprintln!("error: cannot write identity: {err}");
                return 1;
            }
            fresh
        }
    };

    if identity.is_paired() && !re_pair {
        println!(
            "already paired as {} ({})",
            identity.user_email.as_deref().unwrap_or("?"),
            identity.runtime_id.as_deref().unwrap_or("?")
        );
        println!("use --re-pair to pair again (the keypair is reused)");
        return 0;
    }

    // Spike D2: ao requests "ao" once the enum PR is deployed; until then
    // the live API rejects it, so a --runtime-type override (e.g. "desktop")
    // is the documented fallback. Single client-side constant otherwise.
    let runtime_type = runtime_type.unwrap_or_else(|| "ao".to_string());
    let name = name.unwrap_or_else(|| {
        let host = std::env::var_os("HOSTNAME")
            .map(|value| value.to_string_lossy().into_owned())
            .or_else(|| {
                std::process::Command::new("hostname")
                    .output()
                    .ok()
                    .filter(|output| output.status.success())
                    .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            })
            .unwrap_or_else(|| "factory-node".to_string());
        format!("{host} Factory")
    });

    let start = match cloud.create_pairing(&identity, &name, &runtime_type).await {
        Ok(start) => start,
        Err(err) => {
            eprintln!("error: {err}");
            if runtime_type == "ao" {
                eprintln!(
                    "note: runtimeType \"ao\" needs the cloud-api enum deployed (PR ao/runtime-type-ao-enum); \
                     retry with --runtime-type desktop until then"
                );
            }
            return 1;
        }
    };

    println!();
    println!("Pair this Factory node with Allternit:");
    println!("  {}", start.verification_url);
    println!("  Code: {}", start.user_code);
    println!();
    println!("Waiting for approval… (the pairing expires at {})", start.expires_at);

    if !no_browser {
        open_browser_best_effort(&start.verification_url);
    }

    // Fail fast if the identity cannot sign (corrupt key) before polling.
    if let Err(err) = identity.pairing_signature(&start.pairing_id, &start.challenge) {
        eprintln!("error: {err}");
        return 1;
    }
    // The signature is fixed per (pairingId, challenge); exchange() re-derives
    // it on every poll.

    let poll_interval = std::time::Duration::from_secs(start.poll_interval_seconds.max(2));
    let deadline = time::OffsetDateTime::parse(
        &start.expires_at,
        &time::format_description::well_known::Rfc3339,
    )
    .ok()
    .map(|expiry| expiry - time::Duration::seconds(30));

    loop {
        if let Some(deadline) = deadline {
            if time::OffsetDateTime::now_utc() >= deadline {
                eprintln!("error: pairing expired — run `allternit-factory pane fabric pair` again");
                return 1;
            }
        }
        match cloud.exchange(&identity, &start).await {
            Ok(ExchangeOutcome::Issued(session)) => {
                identity.apply_session(&session);
                if let Err(err) = identity.save() {
                    eprintln!("error: paired but cannot write identity: {err}");
                    return 1;
                }
                println!();
                println!(
                    "Paired as {} ({}).",
                    identity.user_email.as_deref().unwrap_or("?"),
                    identity.runtime_id.as_deref().unwrap_or("?")
                );
                println!("Token expires: {}", session.expires_at);
                println!();
                println!(
                    "Pick up sessions on your phone in Allternit Mobile: https://m.allternit.com/?runtime={}",
                    session.runtime_id
                );
                return 0;
            }
            Ok(ExchangeOutcome::Pending) => {
                tokio::time::sleep(poll_interval).await;
            }
            Ok(ExchangeOutcome::RetryAfter(seconds)) => {
                tokio::time::sleep(std::time::Duration::from_secs(seconds.max(1))).await;
            }
            Ok(ExchangeOutcome::Expired) => {
                eprintln!("error: pairing expired — run `allternit-factory pane fabric pair` again");
                return 1;
            }
            Ok(ExchangeOutcome::Denied) => {
                eprintln!("error: pairing was denied");
                return 1;
            }
            Err(err) => {
                eprintln!("error: {err}");
                return 1;
            }
        }
    }
}

fn open_browser_best_effort(url: &str) {
    // Best-effort like gizzi pairing.ts:317-325, but headless-aware: no
    // browser open when there is no display (SSH'd box) or --no-browser.
    let has_display = if cfg!(target_os = "macos") {
        std::env::var_os("SSH_CONNECTION").is_none()
    } else {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    };
    if !has_display {
        return;
    }
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "linux") {
        "xdg-open"
    } else {
        return;
    };
    let _ = std::process::Command::new(program)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let _ = program;
}

// ---------------------------------------------------------------------------
// allternit-factory pane fabric serve
// ---------------------------------------------------------------------------

fn serve(args: &[String]) -> std::io::Result<i32> {
    let mut port = super::fabric_env("PORT")
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(DEFAULT_AO_PORT);
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--port" {
            index += 1;
            let Some(raw) = args.get(index) else {
                eprintln!("--port requires a value\n{USAGE}");
                return Ok(2);
            };
            port = match raw.parse::<u16>() {
                Ok(value) => value,
                Err(_) => {
                    eprintln!("invalid port: {raw}");
                    return Ok(2);
                }
            };
        } else {
            eprintln!("unknown flag: {}\n{USAGE}", args[index]);
            return Ok(2);
        }
        index += 1;
    }

    // Start the engine if it is not running yet — the shim translates to the
    // engine socket API, so a missing engine means empty responses.
    if let Err(err) = ensure_engine_running() {
        eprintln!("warn: engine autostart failed: {err}");
    }

    let identity = match NodeIdentity::load() {
        Some(identity) => identity,
        None => {
            eprintln!("error: no identity — run `allternit-factory pane fabric pair` first");
            return Ok(1);
        }
    };
    if let Err(err) = identity.require_paired() {
        eprintln!("error: {err}");
        return Ok(1);
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("error: cannot start async runtime: {err}");
            return Ok(1);
        }
    };

    Ok(runtime.block_on(async_serve(identity, port)))
}

fn ensure_engine_running() -> std::io::Result<()> {
    let client = crate::api::client::ApiClient::local();
    let request = crate::api::schema::Request {
        id: "ao-fabric:ping".into(),
        method: crate::api::schema::Method::Ping(Default::default()),
    };
    if client.request_value(&request).is_ok() {
        return Ok(());
    }
    crate::server::autodetect::spawn_server_daemon()?;
    crate::server::autodetect::wait_for_server_socket(
        &client.socket_path(),
        std::time::Duration::from_secs(15),
    )
}

async fn async_serve(identity: NodeIdentity, port: u16) -> i32 {
    let cloud = match CloudClient::new() {
        Ok(cloud) => cloud,
        Err(err) => {
            eprintln!("error: {err}");
            return 1;
        }
    };

    // Startup lifecycle, mirroring agent-daemon main() (index.ts:425-441):
    // rotate if inside the skew, then heartbeat (revoked → stop).
    let mut identity = identity;
    if identity.needs_rotation() {
        match cloud.rotate(&identity).await {
            Ok(session) => {
                identity.apply_session(&session);
                let _ = identity.save();
                println!("[factory-fabric] credential rotated (new expiry {})", session.expires_at);
            }
            Err(err) => eprintln!("[factory-fabric] warn: rotation failed: {err}"),
        }
    }
    match cloud.heartbeat(&identity).await {
        Ok(()) => {}
        Err(HeartbeatError::Revoked) => {
            eprintln!("error: this Factory node was revoked — run `allternit-factory pane fabric pair --re-pair`");
            return 1;
        }
        Err(HeartbeatError::Transport(err)) => {
            eprintln!("[factory-fabric] warn: initial heartbeat failed: {err}");
        }
    }

    let identity = Arc::new(tokio::sync::Mutex::new(identity));
    let (reconnect_tx, reconnect_rx) = tokio::sync::watch::channel(0u64);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let relay_label = Arc::new(tokio::sync::RwLock::new("connecting".to_string()));
    let shim_state = Arc::new(super::shim::ShimState {
        relay_label: Arc::clone(&relay_label),
    });

    // The relay must forward to THIS serve's shim port — not blindly to the
    // default port, which may be held by an unrelated service (Desktop's
    // connector sidecar owns 8014 and 401s foreign `/v1/*` traffic).
    let local_gateway = super::resolve_local_gateway(
        port,
        super::fabric_env("GATEWAY_URL"),
    );
    println!("[factory-fabric] relay forwarding to {local_gateway}");

    let relay_task = tokio::spawn(super::relay::run(
        Arc::clone(&identity),
        CloudClient::new().expect("http client"),
        reconnect_rx,
        shutdown_rx,
        Arc::clone(&relay_label),
        local_gateway,
    ));

    let lifecycle_shutdown = shutdown_tx.clone();
    let lifecycle_identity = Arc::clone(&identity);
    let lifecycle = tokio::spawn(async move {
        let cloud = match CloudClient::new() {
            Ok(cloud) => cloud,
            Err(err) => {
                eprintln!("[factory-fabric] lifecycle: {err}");
                return;
            }
        };
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.tick().await; // first tick immediate; startup already did one
        loop {
            interval.tick().await;

            let rotated = {
                let mut identity = lifecycle_identity.lock().await;
                if !identity.needs_rotation() {
                    false
                } else {
                    match cloud.rotate(&identity).await {
                        Ok(session) => {
                            identity.apply_session(&session);
                            let _ = identity.save();
                            eprintln!(
                                "[factory-fabric] credential rotated (new expiry {}); relay reconnecting",
                                session.expires_at
                            );
                            true
                        }
                        Err(err) => {
                            eprintln!("[factory-fabric] warn: rotation failed: {err}");
                            false
                        }
                    }
                }
            };
            if rotated {
                // Rotation forces relay reconnect on the new token (index.ts:236).
                let _ = reconnect_tx.send_modify(|epoch| *epoch += 1);
            }

            let revoked = {
                let identity = lifecycle_identity.lock().await;
                match cloud.heartbeat(&identity).await {
                    Ok(()) => false,
                    Err(HeartbeatError::Revoked) => {
                        eprintln!("[factory-fabric] node was revoked — unpairing and stopping");
                        cloud.revoke_self(&identity).await;
                        true
                    }
                    Err(HeartbeatError::Transport(err)) => {
                        eprintln!("[factory-fabric] warn: heartbeat failed: {err}");
                        false
                    }
                }
            };
            if revoked {
                let _ = lifecycle_shutdown.send(true);
                return;
            }
        }
    });

    // Ctrl-C: std channel because ctrlc handlers are not async.
    let (ctrl_tx, ctrl_rx) = std::sync::mpsc::channel::<()>();
    let _ = ctrlc::set_handler(move || {
        let _ = ctrl_tx.send(());
    });

    let shim_handle = tokio::spawn(super::shim::serve(port, shim_state));
    let ctrl_wait = tokio::task::spawn_blocking(move || {
        let _ = ctrl_rx.recv();
    });

    tokio::select! {
        result = shim_handle => {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => eprintln!("[factory-fabric] shim exited: {err}"),
                Err(err) => eprintln!("[factory-fabric] shim task failed: {err}"),
            }
        }
        _ = ctrl_wait => {
            println!("\n[factory-fabric] shutting down");
        }
    }

    let _ = shutdown_tx.send(true);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), relay_task).await;
    lifecycle.abort();
    let _ = std::io::stdout().flush();
    0
}

// ---------------------------------------------------------------------------
// allternit-factory pane fabric status
// ---------------------------------------------------------------------------

fn status(_args: &[String]) -> std::io::Result<i32> {
    let identity = match NodeIdentity::load() {
        Some(identity) => identity,
        None => {
            println!("identity: none (run `allternit-factory pane fabric pair`)");
            return Ok(0);
        }
    };

    println!("identity: {}", NodeIdentity::identity_path().display());
    println!("public key: {}", identity.public_key);
    println!("fingerprint (sha256): {}", identity.fingerprint());

    if !identity.is_paired() {
        println!("pairing: not paired (run `allternit-factory pane fabric pair`)");
    } else {
        println!("pairing: paired");
        println!("  runtime id: {}", identity.runtime_id.as_deref().unwrap_or("?"));
        println!("  user: {}", identity.user_email.as_deref().unwrap_or("?"));
        if let Some(org) = identity.organization_id.as_deref() {
            println!("  organization: {org}");
        }
        match identity.seconds_to_expiry() {
            Some(seconds) if seconds > 0 => {
                println!(
                    "  token expires: {} ({} days remaining){}",
                    identity.expires_at.as_deref().unwrap_or("?"),
                    seconds / 86_400,
                    if identity.needs_rotation() { " — within rotation window" } else { "" }
                );
            }
            _ => println!(
                "  token expires: {} (EXPIRED — run `allternit-factory pane fabric pair --re-pair`)",
                identity.expires_at.as_deref().unwrap_or("?")
            ),
        }
        println!("  capabilities: {}", identity.capabilities.join(", "));
    }

    // Probe the loopback shim to report local serve state.
    let gateway = super::local_gateway_url();
    let probe = std::process::Command::new("curl")
        .args(["-s", "-m", "2", &format!("{gateway}/status")])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    match probe {
        Ok(output) if output.status.success() => {
            let body = String::from_utf8_lossy(&output.stdout);
            let parsed: Option<serde_json::Value> = serde_json::from_str(&body).ok();
            let relay = parsed
                .as_ref()
                .and_then(|value| value["relay"].as_str())
                .unwrap_or("?");
            let sessions: Vec<&str> = parsed
                .as_ref()
                .and_then(|value| value["sessions"].as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect()
                })
                .unwrap_or_default();
            println!("relay: {relay} (via shim at {gateway})");
            if sessions.is_empty() {
                println!("live Factory sessions: none");
            } else {
                println!("live Factory sessions: {}", sessions.join(", "));
            }
        }
        _ => println!("relay: not serving (run `allternit-factory pane fabric serve`)"),
    }

    Ok(0)
}
