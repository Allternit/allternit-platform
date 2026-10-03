//! The call's "brain": where a finished caller turn goes for a reply.
//!
//! The worker holds the turn loop on calls, but never an LLM: each turn goes to
//! the bot's own runtime (its gizzi-code agent loop, tools, memory, thread)
//! through cloud-api's relay. [`RelayBrain`] is the only runtime impl. If the
//! relay fails, the call speaks [`RELAY_UNAVAILABLE_LINE`] (honest, fixed) and
//! logs the error; there is no scripted or fake reply path outside tests.

use std::time::Duration;

use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};

use super::cloud_client::{CloudClient, TurnRequest};

/// Spoken when the bot's runtime can't be reached for a turn.
pub const RELAY_UNAVAILABLE_LINE: &str =
    "I can't reach my tools right now; I'll have someone follow up.";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct BrainError(pub String);

/// Reply text, streamed as it is produced.
pub type ReplyStream = BoxStream<'static, Result<String, BrainError>>;

pub trait CallBrain: Send + Sync + 'static {
    fn reply(&self, turn: TurnRequest) -> BoxFuture<'static, Result<ReplyStream, BrainError>>;
}

/// cloud-api `POST /api/v1/voice/calls/{callId}/turns` → relay → runtime.
pub struct RelayBrain {
    client: CloudClient,
    first_byte_timeout: Duration,
}

impl RelayBrain {
    /// `first_byte_timeout` bounds how long a caller waits in silence for the
    /// runtime (including a sleeping cloud computer waking) before the honest
    /// fallback line plays.
    pub fn new(client: CloudClient, first_byte_timeout: Duration) -> Self {
        Self { client, first_byte_timeout }
    }
}

impl CallBrain for RelayBrain {
    fn reply(&self, turn: TurnRequest) -> BoxFuture<'static, Result<ReplyStream, BrainError>> {
        let client = self.client.clone();
        let timeout = self.first_byte_timeout;
        Box::pin(async move {
            let s = client.turn(&turn, timeout).await.map_err(|e| BrainError(e.to_string()))?;
            Ok(s.map_err(|e| BrainError(e.to_string())).boxed())
        })
    }
}

/// Test-only brain with canned replies, one per turn, in order.
#[cfg(test)]
pub struct ScriptedBrain {
    replies: std::sync::Mutex<std::collections::VecDeque<Result<Vec<String>, String>>>,
    pub seen: std::sync::Mutex<Vec<TurnRequest>>,
}

#[cfg(test)]
impl ScriptedBrain {
    pub fn new(replies: Vec<Result<Vec<&str>, &str>>) -> Self {
        Self {
            replies: std::sync::Mutex::new(
                replies
                    .into_iter()
                    .map(|r| r.map(|v| v.into_iter().map(String::from).collect()).map_err(String::from))
                    .collect(),
            ),
            seen: Default::default(),
        }
    }
}

#[cfg(test)]
impl CallBrain for ScriptedBrain {
    fn reply(&self, turn: TurnRequest) -> BoxFuture<'static, Result<ReplyStream, BrainError>> {
        self.seen.lock().unwrap().push(turn);
        let next = self.replies.lock().unwrap().pop_front();
        Box::pin(async move {
            match next {
                Some(Ok(parts)) => Ok(futures::stream::iter(parts.into_iter().map(Ok)).boxed()),
                Some(Err(e)) => Err(BrainError(e)),
                None => Err(BrainError("no scripted reply".into())),
            }
        })
    }
}
