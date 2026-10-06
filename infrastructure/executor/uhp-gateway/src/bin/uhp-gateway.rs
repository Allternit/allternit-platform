//! uhp-gateway — thin CLI entry point.

use std::net::SocketAddr;
use std::path::PathBuf;

use uhp_gateway::ServeConfig;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "uhp_gateway=info,tower_http=info".into()),
        )
        .init();

    let mut addr: SocketAddr = "127.0.0.1:8410".parse()?;
    let mut token: Option<String> = std::env::var("UHP_TOKEN").ok().filter(|value| !value.is_empty());
    let mut data_dir: Option<PathBuf> = None;
    // $ALLTERNIT_FACTORY_PANE_SOCKET, else the socket the pane engine exports
    // into every pane (its own env contract, kept from upstream).
    let mut engine_socket: Option<PathBuf> = std::env::var_os("ALLTERNIT_FACTORY_PANE_SOCKET")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("HERDR_SOCKET_PATH")) // old-names: keep (the pane engine's upstream env contract)
        .map(PathBuf::from);

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut flag = arg.as_str();
        // Support both `--flag value` and `--flag=value`.
        let inline = flag.split_once('=').map(|(name, value)| {
            flag = name;
            value.to_string()
        });
        let mut value = || -> Result<String, String> {
            inline
                .clone()
                .or_else(|| args.next())
                .ok_or_else(|| format!("missing value for {flag}"))
        };
        match flag {
            "--addr" => {
                addr = value()?
                    .parse()
                    .map_err(|err| format!("invalid --addr: {err}"))?;
            }
            "--token" => token = Some(value()?),
            "--data-dir" => data_dir = Some(PathBuf::from(value()?)),
            "--engine-socket" => engine_socket = Some(PathBuf::from(value()?)),
            "--help" | "-h" => {
                println!(
                    "uhp-gateway — UHP 2026-08-11 full-class gateway over the ao engine\n\
                     \n\
                     usage: uhp-gateway [--addr 127.0.0.1:8410] [--token TOKEN]\n\
                     \x20                    [--data-dir DIR] [--engine-socket PATH]\n\
                     \n\
                     --addr           bind address (default 127.0.0.1:8410)\n\
                     --token          bearer token (env UHP_TOKEN; random if unset)\n\
                     --data-dir       state directory (default $HOME/.ao/uhp)\n\
                     --engine-socket  pane engine socket (env ALLTERNIT_FACTORY_PANE_SOCKET; required)"
                );
                return Ok(());
            }
            other => return Err(format!("unknown flag {other} (see --help)").into()),
        }
    }

    let token = token.unwrap_or_else(|| {
        let generated = format!("uhp-{}", uuid::Uuid::new_v4());
        eprintln!("uhp-gateway: no --token/UHP_TOKEN given; generated one-use token: {generated}");
        generated
    });
    let data_dir = data_dir.unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".ao/uhp")
    });
    let engine_socket = engine_socket.ok_or(
        "no engine socket: pass --engine-socket or set ALLTERNIT_FACTORY_PANE_SOCKET",
    )?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(uhp_gateway::serve(ServeConfig {
        bind: addr,
        token,
        data_dir,
        engine_socket,
    }))
}
