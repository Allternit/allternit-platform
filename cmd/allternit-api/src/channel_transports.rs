//! Production [`ChannelTransport`]s per provider. Slack lives in
//! `channel_gateway`; Teams, Discord and WhatsApp are added below.

use std::sync::Arc;

use crate::channel_gateway::{ChannelTransport, SlackTransport};
use crate::AppState;

/// The transport for `provider`, or `None` when it is unknown.
pub fn transport_for(_state: &Arc<AppState>, provider: &str) -> Option<Arc<dyn ChannelTransport>> {
    match provider {
        "slack" => Some(Arc::new(SlackTransport::from_env())),
        _ => None,
    }
}
