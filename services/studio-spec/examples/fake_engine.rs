//! Run: cargo run -p allternit-studio-spec --example fake_engine -- [port] [token]
//! Prints the base URL and token, then serves engine API v1 until killed.
use allternit_studio_spec::fake_engine::router;
use std::time::Duration;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let token = args.next().unwrap_or_else(|| "fake-token".into());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.expect("bind");
    println!("fake engine on http://{} token={token}", listener.local_addr().unwrap());
    axum::serve(listener, router(&token, Duration::from_millis(500))).await.unwrap();
}
