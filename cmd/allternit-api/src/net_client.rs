//! Shared outbound HTTP client with default timeouts.
//!
//! `reqwest::Client::new()` sets no timeout, so a hung upstream holds the
//! task and its connection forever. Use `client()` instead.
//!
//! - `connect_timeout` bounds the TCP and TLS setup.
//! - `read_timeout` bounds each read. It does not cap a whole response, so
//!   long SSE and LLM streams still work while bytes keep arriving.
//!
//! A call site that needs a hard total limit adds `.timeout(..)` on the request.

use std::sync::OnceLock;
use std::time::Duration;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const READ_TIMEOUT: Duration = Duration::from_secs(120);

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// The shared client. Cloning is cheap: clones share one connection pool.
pub fn client() -> reqwest::Client {
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .read_timeout(READ_TIMEOUT)
                .build()
                .unwrap_or_default()
        })
        .clone()
}
