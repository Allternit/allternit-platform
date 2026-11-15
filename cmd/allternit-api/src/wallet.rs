//! Wallet client — moves money between the Stripe-backed user wallet
//! (allternit-cloud-api) and this server's org fabric credits ledger.
//!
//! The cloud API exposes `POST /api/v1/internal/billing/credits/transfer`
//! (shared-secret gated). This module is the allternit-api side of the
//! credits consolidation bridge: `direction=debit` pulls balance out of the
//! user's wallet so the caller can credit the org ledger locally;
//! `direction=credit` refunds it (compensation path).

use serde::Serialize;

/// Result of one wallet transfer call.
#[derive(Debug, Clone)]
pub struct WalletTransferResult {
    pub balance_usd: f64,
    pub idempotent_replay: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum WalletError {
    #[error("wallet transfer is not configured (no cloud API URL)")]
    NotConfigured,
    #[error("wallet credentials rejected: {0}")]
    Unauthorized(String),
    #[error("insufficient wallet balance: {0}")]
    Insufficient(String),
    #[error("wallet upstream error {0}: {1}")]
    Upstream(u16, String),
    #[error("wallet request failed: {0}")]
    Transport(#[from] reqwest::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferDirection {
    Debit,
    Credit,
}

impl TransferDirection {
    fn as_str(&self) -> &'static str {
        match self {
            TransferDirection::Debit => "debit",
            TransferDirection::Credit => "credit",
        }
    }
}

/// Server-to-server wallet operations. Trait-sealed so route tests can use a
/// fake (the real client talks to the cloud API over HTTP).
pub trait WalletClient: Send + Sync {
    async fn transfer(
        &self,
        user_id: &str,
        amount_cents: i64,
        idempotency_key: &str,
        direction: TransferDirection,
    ) -> Result<WalletTransferResult, WalletError>;
}

/// Live wallet client speaking to the cloud API's internal transfer route.
pub struct ReqwestWalletClient {
    base_url: String,
    secret: String,
    client: reqwest::Client,
}

impl ReqwestWalletClient {
    pub fn new(base_url: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            secret: secret.into(),
            client: crate::net_client::client(),
        }
    }

    /// Build a client from the environment, or `None` when hosted billing is
    /// not configured on this deployment.
    pub fn from_env(config: &crate::config::AppConfig) -> Option<Self> {
        let base_url = config.cloud_api_url()?;
        let secret = std::env::var("ALLTERNIT_BILLING_SYNC_SECRET")
            .ok()
            .filter(|s| !s.is_empty())?;
        Some(Self::new(base_url, secret))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TransferRequest<'a> {
    user_id: &'a str,
    amount_cents: i64,
    idempotency_key: &'a str,
    direction: &'a str,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransferResponse {
    balance_usd: f64,
    #[serde(default)]
    idempotent_replay: bool,
}

impl WalletClient for ReqwestWalletClient {
    async fn transfer(
        &self,
        user_id: &str,
        amount_cents: i64,
        idempotency_key: &str,
        direction: TransferDirection,
    ) -> Result<WalletTransferResult, WalletError> {
        let response = self
            .client
            .post(format!(
                "{}/api/v1/internal/billing/credits/transfer",
                self.base_url
            ))
            .header("x-allternit-billing-secret", &self.secret)
            .json(&TransferRequest {
                user_id,
                amount_cents,
                idempotency_key,
                direction: direction.as_str(),
            })
            .send()
            .await?;

        let status = response.status().as_u16();
        if status == 401 || status == 403 {
            return Err(WalletError::Unauthorized(format!("HTTP {status}")));
        }
        if status == 402 {
            return Err(WalletError::Insufficient(format!("HTTP {status}")));
        }
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(WalletError::Upstream(status, body));
        }
        let payload: TransferResponse = response.json().await?;
        Ok(WalletTransferResult {
            balance_usd: payload.balance_usd,
            idempotent_replay: payload.idempotent_replay,
        })
    }
}
