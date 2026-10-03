//! Minimal typed client for the vendored mailflare service (a per-installation
//! Cloudflare Worker email system).
//!
//! Configuration is env-driven: `ALLTERNIT_MAILFLARE_URL` (worker base URL),
//! `ALLTERNIT_MAILFLARE_ADMIN_KEY` (admin-scope `ep_...` key, used for mailbox
//! and scoped-key management), `ALLTERNIT_BOT_EMAIL_DOMAIN` (the agent email
//! domain, e.g. `agents.example.com`) and `ALLTERNIT_MAILFLARE_WEBHOOK_SECRET`
//! (HMAC secret for inbound webhook verification). When the URL or admin key is
//! unset, mailflare is disabled and callers fall back to the previous
//! mint-only behavior.

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone)]
pub struct MailflareConfig {
    /// Worker base URL, trailing slash trimmed.
    pub base_url: String,
    /// Admin-scope API key (`ep_...`).
    pub admin_key: String,
    /// Agent email domain (`ALLTERNIT_BOT_EMAIL_DOMAIN`).
    pub domain: String,
    /// HMAC-SHA256 secret shared with mailflare webhook deliveries.
    pub webhook_secret: Option<String>,
}

impl MailflareConfig {
    /// Load from the environment. Returns `None` (mailflare disabled) when the
    /// base URL or admin key is missing/empty.
    pub fn from_env() -> Option<Self> {
        let base_url = env_non_empty("ALLTERNIT_MAILFLARE_URL")?;
        let admin_key = env_non_empty("ALLTERNIT_MAILFLARE_ADMIN_KEY")?;
        let domain = env_non_empty("ALLTERNIT_BOT_EMAIL_DOMAIN")?;
        Some(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            admin_key,
            domain,
            webhook_secret: env_non_empty("ALLTERNIT_MAILFLARE_WEBHOOK_SECRET"),
        })
    }
}

fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

#[derive(Debug)]
pub struct MailflareError {
    pub status: Option<StatusCode>,
    pub message: String,
}

impl std::fmt::Display for MailflareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(status) => write!(f, "mailflare HTTP {}: {}", status, self.message),
            None => write!(f, "mailflare: {}", self.message),
        }
    }
}

impl std::error::Error for MailflareError {}

#[derive(Debug, Deserialize)]
pub struct MailflareDomain {
    pub id: String,
    pub hostname: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateMailboxResponse {
    pub id: String,
    pub address: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxSummary {
    pub id: String,
    pub local_part: String,
    pub hostname: String,
}

impl MailboxSummary {
    pub fn address(&self) -> String {
        format!("{}@{}", self.local_part, self.hostname)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateKeyResponse {
    pub id: String,
    /// The full `ep_...` key — returned only once, at creation.
    pub key: String,
}

#[derive(Debug, Serialize)]
pub struct SendEmailRequest<'a> {
    pub from: &'a str,
    pub to: &'a str,
    pub subject: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<&'a str>,
    #[serde(rename = "mailboxId")]
    pub mailbox_id: &'a str,
    /// Reply threading / auto-reply markers. mailflare only accepts the
    /// allow-listed set (In-Reply-To, References, Auto-Submitted,
    /// List-Unsubscribe); anything else fails the send with a 400.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<&'a std::collections::HashMap<String, String>>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Serialize)]
struct SendBody<'a> {
    #[serde(flatten)]
    request: &'a SendEmailRequest<'a>,
    /// Admin-scope keys only: deliver this one message without the approval
    /// gate. mailflare ignores it for mailbox-scoped keys.
    #[serde(rename = "skipApproval", skip_serializing_if = "is_false")]
    skip_approval: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendEmailResponse {
    pub message_id: String,
    #[serde(default)]
    pub job_id: Option<String>,
    /// `pending_approval` when REQUIRE_SEND_APPROVAL is on (the default),
    /// `queued` otherwise.
    pub status: String,
}

#[derive(Clone)]
pub struct MailflareClient {
    config: MailflareConfig,
    http: Client,
}

impl MailflareClient {
    pub fn new(config: MailflareConfig) -> Self {
        Self {
            config,
            http: Client::new(),
        }
    }

    pub fn from_env() -> Option<Self> {
        MailflareConfig::from_env().map(Self::new)
    }

    pub fn config(&self) -> &MailflareConfig {
        &self.config
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.config.base_url, path)
    }

    /// Extract mailflare's `{"error": ...}` body when present.
    async fn check(response: reqwest::Response) -> Result<reqwest::Response, MailflareError> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                v.get("error")
                    .map(|e| e.as_str().map(str::to_string).unwrap_or_else(|| e.to_string()))
            })
            .unwrap_or(body);
        Err(MailflareError {
            status: Some(status),
            message,
        })
    }

    /// `GET /api/domains` with the admin key — also serves as the cheap
    /// reachability probe for the status endpoint.
    pub async fn list_domains(&self) -> Result<Vec<MailflareDomain>, MailflareError> {
        let response = Self::check(
            self.http
                .get(self.url("/api/domains"))
                .bearer_auth(&self.config.admin_key)
                .send()
                .await
                .map_err(|e| MailflareError {
                    status: None,
                    message: e.to_string(),
                })?,
        )
        .await?;
        let body: serde_json::Value = response.json().await.map_err(|e| MailflareError {
            status: None,
            message: e.to_string(),
        })?;
        let domains = body
            .get("domains")
            .cloned()
            .unwrap_or(serde_json::Value::Array(vec![]));
        serde_json::from_value(domains).map_err(|e| MailflareError {
            status: None,
            message: format!("unexpected /api/domains shape: {e}"),
        })
    }

    /// Resolve the mailflare domain id for `ALLTERNIT_BOT_EMAIL_DOMAIN`,
    /// cached per (base_url, domain) for the process lifetime.
    pub async fn resolve_domain_id(&self) -> Result<String, MailflareError> {
        let cache_key = format!("{}|{}", self.config.base_url, self.config.domain);
        let cache = domain_cache();
        if let Some(id) = cache.lock().unwrap().get(&cache_key) {
            return Ok(id.clone());
        }
        let domains = self.list_domains().await?;
        let found = domains
            .iter()
            .find(|d| d.hostname.eq_ignore_ascii_case(&self.config.domain))
            .ok_or_else(|| MailflareError {
                status: None,
                message: format!(
                    "domain '{}' not found in mailflare (add it via the mailflare dashboard first)",
                    self.config.domain
                ),
            })?;
        cache
            .lock()
            .unwrap()
            .insert(cache_key, found.id.clone());
        Ok(found.id.clone())
    }

    /// `POST /api/mailboxes` (admin key). Creates the mailbox and the
    /// Cloudflare Email Routing rule for the address.
    pub async fn create_mailbox(
        &self,
        domain_id: &str,
        local_part: &str,
        display_name: Option<&str>,
    ) -> Result<CreateMailboxResponse, MailflareError> {
        let response = Self::check(
            self.http
                .post(self.url("/api/mailboxes"))
                .bearer_auth(&self.config.admin_key)
                .json(&serde_json::json!({
                    "domainId": domain_id,
                    "localPart": local_part,
                    "displayName": display_name,
                }))
                .send()
                .await
                .map_err(|e| MailflareError {
                    status: None,
                    message: e.to_string(),
                })?,
        )
        .await?;
        response.json().await.map_err(|e| MailflareError {
            status: None,
            message: e.to_string(),
        })
    }

    /// `GET /api/mailboxes` (admin key) — used to adopt an existing mailbox
    /// after a create conflict.
    pub async fn list_mailboxes(&self) -> Result<Vec<MailboxSummary>, MailflareError> {
        let response = Self::check(
            self.http
                .get(self.url("/api/mailboxes"))
                .bearer_auth(&self.config.admin_key)
                .send()
                .await
                .map_err(|e| MailflareError {
                    status: None,
                    message: e.to_string(),
                })?,
        )
        .await?;
        let body: serde_json::Value = response.json().await.map_err(|e| MailflareError {
            status: None,
            message: e.to_string(),
        })?;
        let mailboxes = body
            .get("mailboxes")
            .cloned()
            .unwrap_or(serde_json::Value::Array(vec![]));
        serde_json::from_value(mailboxes).map_err(|e| MailflareError {
            status: None,
            message: format!("unexpected /api/mailboxes shape: {e}"),
        })
    }

    /// `DELETE /api/mailboxes/[id]` (admin key). Removes the routing rule and
    /// disables the mailbox. 404 is treated as success (already gone).
    pub async fn delete_mailbox(&self, mailbox_id: &str) -> Result<(), MailflareError> {
        let response = self
            .http
            .delete(self.url(&format!("/api/mailboxes/{mailbox_id}")))
            .bearer_auth(&self.config.admin_key)
            .send()
            .await
            .map_err(|e| MailflareError {
                status: None,
                message: e.to_string(),
            })?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Self::check(response).await?;
        Ok(())
    }

    /// `POST /api/api-keys` with the admin key (admin-scope keys act as their
    /// owning user). Mint a mailbox-scoped key, e.g. `scopes = ["send", "read"]`.
    pub async fn create_scoped_key(
        &self,
        name: &str,
        scopes: &[&str],
        mailbox_ids: &[String],
    ) -> Result<CreateKeyResponse, MailflareError> {
        let response = Self::check(
            self.http
                .post(self.url("/api/api-keys"))
                .bearer_auth(&self.config.admin_key)
                .json(&serde_json::json!({
                    "name": name,
                    "scopes": scopes,
                    "mailboxIds": mailbox_ids,
                }))
                .send()
                .await
                .map_err(|e| MailflareError {
                    status: None,
                    message: e.to_string(),
                })?,
        )
        .await?;
        response.json().await.map_err(|e| MailflareError {
            status: None,
            message: e.to_string(),
        })
    }

    /// `POST /api/v1/send` with a per-agent send-scope key. With
    /// REQUIRE_SEND_APPROVAL=true (the mailflare default) this does NOT send —
    /// it returns `status: "pending_approval"` plus a `job_id` to approve.
    pub async fn send(
        &self,
        api_key: &str,
        request: &SendEmailRequest<'_>,
        idempotency_key: &str,
    ) -> Result<SendEmailResponse, MailflareError> {
        self.send_with_options(api_key, request, idempotency_key, false)
            .await
    }

    /// Like [`send`], with the admin-only approval bypass. Only allternit-api's
    /// admin key may set `skip_approval`; mailflare ignores the flag for
    /// mailbox-scoped keys, so a wrong key here degrades to approval-gated.
    pub async fn send_with_options(
        &self,
        api_key: &str,
        request: &SendEmailRequest<'_>,
        idempotency_key: &str,
        skip_approval: bool,
    ) -> Result<SendEmailResponse, MailflareError> {
        let body = SendBody {
            request,
            skip_approval,
        };
        let response = Self::check(
            self.http
                .post(self.url("/api/v1/send"))
                .bearer_auth(api_key)
                .header("Idempotency-Key", idempotency_key)
                .json(&body)
                .send()
                .await
                .map_err(|e| MailflareError {
                    status: None,
                    message: e.to_string(),
                })?,
        )
        .await?;
        response.json().await.map_err(|e| MailflareError {
            status: None,
            message: e.to_string(),
        })
    }

    /// `POST /api/v1/outbound/[jobId]/approve` (send-scope, mailbox-scoped key).
    /// Returns the provider message id on success.
    pub async fn approve(&self, api_key: &str, job_id: &str) -> Result<String, MailflareError> {
        let response = Self::check(
            self.http
                .post(self.url(&format!("/api/v1/outbound/{job_id}/approve")))
                .bearer_auth(api_key)
                .send()
                .await
                .map_err(|e| MailflareError {
                    status: None,
                    message: e.to_string(),
                })?,
        )
        .await?;
        let body: serde_json::Value = response.json().await.map_err(|e| MailflareError {
            status: None,
            message: e.to_string(),
        })?;
        Ok(body
            .get("messageId")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string())
    }

    /// `POST /api/v1/outbound/[jobId]/reject` (send-scope, mailbox-scoped key).
    pub async fn reject(&self, api_key: &str, job_id: &str) -> Result<(), MailflareError> {
        Self::check(
            self.http
                .post(self.url(&format!("/api/v1/outbound/{job_id}/reject")))
                .bearer_auth(api_key)
                .send()
                .await
                .map_err(|e| MailflareError {
                    status: None,
                    message: e.to_string(),
                })?,
        )
        .await?;
        Ok(())
    }

    /// `GET /api/v1/messages/[id]` with a read-scope key — full bodies when the
    /// webhook snippet is not enough.
    pub async fn get_message(
        &self,
        api_key: &str,
        message_id: &str,
    ) -> Result<serde_json::Value, MailflareError> {
        let response = Self::check(
            self.http
                .get(self.url(&format!("/api/v1/messages/{message_id}")))
                .bearer_auth(api_key)
                .send()
                .await
                .map_err(|e| MailflareError {
                    status: None,
                    message: e.to_string(),
                })?,
        )
        .await?;
        response.json().await.map_err(|e| MailflareError {
            status: None,
            message: e.to_string(),
        })
    }
}

/// Process-local cache of resolved domain ids, keyed by `base_url|domain`.
fn domain_cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}
