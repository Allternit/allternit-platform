//! `ao serve` — run the UHP (Unified Harness Protocol) HTTP surface in-process
//! under the ao binary (P6a core + P6b full; plan §2.6). Operator doc: `docs/programs/ao/UHP.md`.
//!
//! Not a sidecar: the gateway lives in the `uhp-gateway` workspace crate and
//! is mounted here so the `ao serve` process IS the UHP server. The gateway
//! drives the engine over its Unix-socket JSON-RPC API — the same substrate
//! `ao spawn/send/watch` use — so a missing engine is auto-started, exactly
//! like the ao contract commands.
//!
//! Surface: `ao serve` (foreground; Ctrl-C/SIGTERM stops) and
//! `ao serve health` (probe the discovery endpoint of a running surface).

use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use rand::Rng as _;

use crate::api::client::ApiClient;
use crate::api::schema::{Method, PingParams, Request};

const USAGE: &str = concat!(
    "usage: ao serve [--addr 127.0.0.1:8410] [--token TOKEN]\n",
    "                [--data-dir DIR] [--engine-socket PATH]\n",
    "       ao serve health [--addr 127.0.0.1:8410] [--token TOKEN]\n",
    "\n",
    "Run the UHP 2026-08-11 HTTP surface in-process (full class).\n",
    "\n",
    "  --addr           bind address (default 127.0.0.1:8410)\n",
    "  --token          bearer token (env UHP_TOKEN; random if unset)\n",
    "  --data-dir       state directory (default $HOME/.ao/uhp)\n",
    "  --engine-socket  herdr engine socket (default: the running engine's;\n",
    "                   the engine is auto-started when absent)\n",
    "\n",
    "health             GET /v1/uhp on a running surface; exit 0 on HTTP 200",
);

pub(crate) fn run(args: &[String]) -> io::Result<i32> {
    if args
        .first()
        .map(|arg| matches!(arg.as_str(), "help" | "--help" | "-h"))
        .unwrap_or(false)
    {
        eprintln!("{USAGE}");
        return Ok(0);
    }

    let health = args.first().map(|arg| arg.as_str()) == Some("health");
    let flag_args = if health { &args[1..] } else { args };
    let flags = match parse_flags(flag_args) {
        Ok(flags) => flags,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return Ok(2);
        }
    };

    if health {
        return serve_health(&flags);
    }

    let engine_socket = match flags.engine_socket {
        Some(path) => path,
        None => ensure_engine_running()?,
    };
    let token = flags.token.unwrap_or_else(generate_token);
    let data_dir = flags.data_dir.unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".ao/uhp")
    });

    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "uhp_gateway=info".into()),
        )
        .try_init();

    eprintln!("ao serve: UHP surface on http://{}", flags.addr);
    eprintln!("ao serve: bearer token: {token}");
    eprintln!("ao serve: engine socket: {}", engine_socket.display());
    eprintln!("ao serve: data dir: {}", data_dir.display());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(io::Error::other)?;
    runtime
        .block_on(uhp_gateway::serve(uhp_gateway::ServeConfig {
            bind: flags.addr,
            token,
            data_dir,
            engine_socket,
        }))
        .map_err(io::Error::other)?;
    Ok(0)
}

struct ServeFlags {
    addr: SocketAddr,
    token: Option<String>,
    data_dir: Option<PathBuf>,
    engine_socket: Option<PathBuf>,
}

fn parse_flags(args: &[String]) -> Result<ServeFlags, String> {
    let mut flags = ServeFlags {
        addr: "127.0.0.1:8410".parse().expect("valid default addr"),
        token: std::env::var("UHP_TOKEN").ok().filter(|value| !value.is_empty()),
        data_dir: None,
        engine_socket: None,
    };
    let mut index = 0;
    while index < args.len() {
        let mut flag = args[index].as_str();
        let inline = flag.split_once('=').map(|(name, value)| {
            flag = name;
            value.to_string()
        });
        let mut value = |index: &mut usize| -> Result<String, String> {
            if let Some(inline) = inline.clone() {
                return Ok(inline);
            }
            *index += 1;
            args.get(*index)
                .cloned()
                .ok_or_else(|| format!("missing value for {flag}"))
        };
        match flag {
            "--addr" => {
                flags.addr = value(&mut index)?
                    .parse()
                    .map_err(|err| format!("invalid --addr: {err}"))?;
            }
            "--token" => flags.token = Some(value(&mut index)?),
            "--data-dir" => flags.data_dir = Some(PathBuf::from(value(&mut index)?)),
            "--engine-socket" => flags.engine_socket = Some(PathBuf::from(value(&mut index)?)),
            other => return Err(format!("unknown flag {other}")),
        }
        index += 1;
    }
    Ok(flags)
}

/// Ping the engine; auto-start the server daemon when it is absent, exactly
/// like the ao contract commands (`cli/ao.rs::ensure_engine_running`).
fn ensure_engine_running() -> io::Result<PathBuf> {
    let client = ApiClient::local();
    let request = Request {
        id: "ao:ping".into(),
        method: Method::Ping(PingParams::default()),
    };
    if client.request_value(&request).is_err() {
        crate::server::autodetect::spawn_server_daemon()?;
        crate::server::autodetect::wait_for_server_socket(
            &client.socket_path(),
            Duration::from_secs(15),
        )?;
    }
    Ok(client.socket_path())
}

fn serve_health(flags: &ServeFlags) -> io::Result<i32> {
    let url = format!("http://{}/v1/uhp", flags.addr);
    let mut request = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(io::Error::other)?
        .get(&url);
    if let Some(token) = &flags.token {
        request = request.bearer_auth(token);
    }
    match request.send() {
        Ok(response) => {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            println!("{status} {body}");
            Ok(if status.is_success() { 0 } else { 1 })
        }
        Err(err) => {
            eprintln!("ao serve health: {err}");
            Ok(1)
        }
    }
}

fn generate_token() -> String {
    let mut rng = rand::thread_rng();
    (0..32).map(|_| format!("{:02x}", rng.gen::<u8>())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_defaults() {
        let flags = parse_flags(&[]).expect("defaults parse");
        assert_eq!(flags.addr.to_string(), "127.0.0.1:8410");
        assert!(flags.data_dir.is_none());
        assert!(flags.engine_socket.is_none());
    }

    #[test]
    fn parse_inline_and_spaced_values() {
        let args = vec![
            "--addr=0.0.0.0:9000".to_string(),
            "--token".to_string(),
            "secret".to_string(),
            "--data-dir=/tmp/uhp".to_string(),
        ];
        let flags = parse_flags(&args).expect("flags parse");
        assert_eq!(flags.addr.to_string(), "0.0.0.0:9000");
        assert_eq!(flags.token.as_deref(), Some("secret"));
        assert_eq!(
            flags.data_dir.as_deref(),
            Some(std::path::Path::new("/tmp/uhp"))
        );
    }

    #[test]
    fn unknown_flag_rejected() {
        assert!(parse_flags(&["--bogus".to_string()]).is_err());
    }

    #[test]
    fn token_is_64_hex_chars() {
        let token = generate_token();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
