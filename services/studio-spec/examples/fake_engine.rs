//! Run: cargo run -p allternit-studio-spec --example fake_engine -- [port] [token] [hardware]
//! `hardware` is a hardware id (default `apple-silicon`, or the launch env
//! `ALLTERNIT_STUDIO_HARDWARE`). Output paths point under the launch env
//! `ALLTERNIT_STUDIO_OUTPUT_DIR` when set. Prints the base URL and token, then
//! serves engine API v1 until killed.
use allternit_studio_spec::fake_engine::{router_with, FakeConfig};
use allternit_studio_spec::{Hardware, HostHardware};
use std::time::Duration;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let token = args.next().unwrap_or_else(|| "fake-token".into());
    let hw_id = args
        .next()
        .or_else(|| std::env::var("ALLTERNIT_STUDIO_HARDWARE").ok())
        .unwrap_or_else(|| "apple-silicon".into());
    let Some(hardware) = Hardware::from_id(&hw_id) else {
        let ids: Vec<&str> = Hardware::ALL.iter().map(|h| h.id()).collect();
        eprintln!("unknown hardware '{hw_id}'; one of {}", ids.join(", "));
        std::process::exit(2);
    };
    let cfg = FakeConfig {
        hardware: HostHardware::plain(hardware),
        output_dir: std::env::var_os("ALLTERNIT_STUDIO_OUTPUT_DIR").map(Into::into),
    };
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.expect("bind");
    println!("fake engine on http://{} token={token} hardware={hw_id}", listener.local_addr().unwrap());
    axum::serve(listener, router_with(&token, Duration::from_millis(500), cfg)).await.unwrap();
}
