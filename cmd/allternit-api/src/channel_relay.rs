//! Shared names for the channels hybrid relay (cloud-api
//! `routes/channel_inbound.rs`): inbound platform requests reach this runtime
//! through cloud-api's queue and the runtime relay.

/// Set by cloud-api on queued deliveries: unix seconds when it received the
/// platform's request. Platforms that sign a timestamp (Slack) are checked
/// against this instead of the delivery time.
pub const QUEUED_AT_HEADER: &str = "x-allternit-channel-queued-at";
