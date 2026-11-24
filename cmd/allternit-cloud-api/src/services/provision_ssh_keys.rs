//! Computers console SSH keys (contract
//! `surfaces/docs/dev/computers-console-contract.mdx`, "SSH key model",
//! step 2.7; table `computer_ssh_keys`, migration 156).
//!
//! Each cloud computer keeps its own list of SSH public keys: people's keys
//! (`kind: "computer"`, no expiry) and short-lived agent keys (`kind:
//! "agent"`, `expires_at` set). Only public keys are stored; a private key is
//! refused before anything is written.
//!
//! Cloud computers use Orgo's Direct SSH: `POST /computers/{id}/ssh` with
//! every key, one per line (the keys sent replace the ones on the computer),
//! `DELETE /computers/{id}/ssh` when the last key goes, `GET
//! /computers/{id}/ssh` for the host and port. So every change sends the
//! whole current set, under a row lock on the instance so two changes cannot
//! drop each other's key. Expired agent keys leave the set on the next
//! change, and the sweep below removes them every 5 minutes; a row is deleted
//! only after Orgo took the set without it, so the list never hides a key
//! that still works.
//!
//! Paired and desktop computers have no key route yet: the node has no
//! action that writes the marked `authorized_keys` block, so their ids are
//! 404 `not_a_cloud_computer` like the other cloud-only console routes.

use super::*;
use base64::{
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
    Engine as _,
};
use sha2::{Digest, Sha256};

/// Keys on one computer, people's and agents' together.
pub const MAX_KEYS_PER_COMPUTER: usize = 20;
/// Orgo refuses a key set over 16 KB (`public_key too large (max 16 KB)`).
pub const MAX_KEY_SET_BYTES: usize = 16 * 1024;
/// Agent key lifetime when none is asked for, and the allowed range.
pub const AGENT_KEY_DEFAULT_MINUTES: i64 = 60;
pub const AGENT_KEY_MIN_MINUTES: i64 = 5;
pub const AGENT_KEY_MAX_MINUTES: i64 = 24 * 60;
const LABEL_MAX_CHARS: usize = 80;
const COMMENT_MAX_CHARS: usize = 200;
const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// The key types Orgo's Direct SSH accepts.
const KEY_TYPES: &[&str] = &[
    "ssh-ed25519",
    "ssh-rsa",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "sk-ssh-ed25519@openssh.com",
    "sk-ecdsa-sha2-nistp256@openssh.com",
];

fn refused(code: &'static str, message: &str) -> CloudActionError {
    CloudActionError::SshKeyRefused {
        code,
        message: message.to_string(),
    }
}

/// A checked OpenSSH public key line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPublicKey {
    pub key_type: String,
    /// `<type> <base64>[ <comment>]`, the line sent to the computer.
    pub line: String,
    pub comment: Option<String>,
    /// OpenSSH form: `SHA256:` + base64 (no padding) of sha256(key blob).
    pub fingerprint: String,
}

/// Checks one OpenSSH public key (the contents of a `.pub` file). Private
/// keys, several keys at once, `authorized_keys` options and anything whose
/// base64 blob does not name its own type are refused with a plain message.
pub fn parse_public_key(text: &str) -> Result<ParsedPublicKey, CloudActionError> {
    let text = text.trim();
    if text.contains("PRIVATE KEY") || text.starts_with("PuTTY-User-Key-File") {
        return Err(refused(
            "private_key",
            "That is a private key. Never share it: paste the public key (the .pub file, for example ~/.ssh/id_ed25519.pub) instead. Nothing was saved.",
        ));
    }
    if text.is_empty() {
        return Err(refused(
            "public_key_required",
            "Paste a public key (the contents of a .pub file).",
        ));
    }
    if text.lines().filter(|l| !l.trim().is_empty()).count() > 1 {
        return Err(refused(
            "one_key_only",
            "Add one key at a time. Nothing was saved.",
        ));
    }
    let mut words = text.split_whitespace();
    let key_type = words.next().unwrap_or_default();
    if !KEY_TYPES.contains(&key_type) {
        return Err(refused(
            "invalid_public_key",
            "That does not look like an SSH public key. It should start with ssh-ed25519, ssh-rsa, ecdsa-sha2-… or sk-…, with nothing in front. Nothing was saved.",
        ));
    }
    let encoded = words.next().unwrap_or_default();
    let blob = STANDARD
        .decode(encoded)
        .ok()
        .filter(|blob| blob_names(blob, key_type));
    let Some(blob) = blob else {
        return Err(refused(
            "invalid_public_key",
            "That key is cut off or damaged. Copy the whole line from the .pub file. Nothing was saved.",
        ));
    };
    let comment: String = words
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .take(COMMENT_MAX_CHARS)
        .collect();
    let comment = Some(comment.trim().to_string()).filter(|c| !c.is_empty());
    let line = match &comment {
        Some(comment) => format!("{key_type} {encoded} {comment}"),
        None => format!("{key_type} {encoded}"),
    };
    Ok(ParsedPublicKey {
        key_type: key_type.to_string(),
        line,
        comment,
        fingerprint: format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(&blob))),
    })
}

/// True when the SSH wire blob starts with a length-prefixed `key_type`
/// and carries key material after it.
fn blob_names(blob: &[u8], key_type: &str) -> bool {
    let Some(len) = blob
        .get(..4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
    else {
        return false;
    };
    blob.get(4..4 + len) == Some(key_type.as_bytes()) && blob.len() > 4 + len + 4
}

/// `POST /api/v1/computers/:id/ssh-keys` body.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshKeyInput {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub public_key: Option<String>,
    /// `computer` (default) or `agent`.
    #[serde(default)]
    pub kind: Option<String>,
    /// Agent keys only: lifetime in minutes (default 60, 5 to 1440).
    #[serde(default)]
    pub expires_in_minutes: Option<i64>,
}

/// One key as the console shows it. The key itself is not sent back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshKeyView {
    pub id: String,
    pub label: String,
    pub fingerprint: String,
    pub kind: String,
    pub key_type: String,
    pub created_at: String,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
struct KeyRow {
    id: String,
    label: String,
    public_key: String,
    fingerprint: String,
    kind: String,
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
}

impl KeyRow {
    fn view(&self) -> SshKeyView {
        SshKeyView {
            id: self.id.clone(),
            label: self.label.clone(),
            fingerprint: self.fingerprint.clone(),
            kind: self.kind.clone(),
            key_type: self
                .public_key
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string(),
            created_at: self.created_at.to_rfc3339(),
            expires_at: self.expires_at.map(|at| at.to_rfc3339()),
        }
    }

    fn active(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_none_or(|at| at > now)
    }
}

/// `GET /api/v1/computers/:id/ssh-keys`.
#[derive(Debug, Clone)]
pub struct SshKeyList {
    pub keys: Vec<SshKeyView>,
    /// Where to connect, read from Orgo while the computer runs and has keys.
    pub connect: Option<SshEndpoint>,
}

/// What one locked change does to the key set.
enum KeyChange {
    Add(KeyRow),
    Remove(String),
    /// Drop expired agent keys only (the sweep).
    Prune,
}

const KEY_COLUMNS: &str = "id, label, public_key, fingerprint, kind, created_at, expires_at";

/// Deletes every key row of a cloud computer whose disk was replaced
/// (rebuild, move): the fresh computer has none. Best effort, logged.
pub(super) async fn forget_ssh_keys(db: &PgPool, instance_id: &str) {
    if let Err(error) = sqlx::query(
        "DELETE FROM computer_ssh_keys WHERE computer_kind = 'cloud' AND computer_id = $1",
    )
    .bind(instance_id)
    .execute(db)
    .await
    {
        tracing::warn!(id = %instance_id, %error, "ssh keys: could not clear the list of a replaced computer");
    }
}

/// Removes expired agent keys from every cloud computer every 5 minutes.
pub fn start_ssh_key_sweeper(service: Arc<ProvisioningService>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(SWEEP_EVERY).await;
            service.sweep_expired_ssh_keys(Utc::now()).await;
        }
    });
}

/// Maps a provider error of a key change to the console error.
fn provider_error(error: ProvisionError, row_status: &str) -> CloudActionError {
    match error {
        ProvisionError::Api { status: 409, .. } if row_status == "running" => {
            CloudActionError::Busy {
                status: row_status.to_string(),
            }
        }
        ProvisionError::Api { status: 409, .. } => CloudActionError::Asleep {
            status: row_status.to_string(),
        },
        ProvisionError::Api { status: 501, .. } => CloudActionError::NotSupported("ssh_keys"),
        ProvisionError::Api {
            status: 400,
            message,
        } if message.contains("too large") => refused(
            "too_many_keys",
            "This computer can hold 16 KB of keys. Remove one first. Nothing was changed.",
        ),
        other => CloudActionError::Api(other.to_api_error()),
    }
}

impl ProvisioningService {
    /// The caller's Orgo computer and its backend, for the key routes.
    /// Standby (Incus) and free computers are 501 `not_supported`.
    async fn ssh_key_row(
        &self,
        instance_id: &str,
        user_id: &str,
    ) -> Result<(InstanceRow, Arc<dyn ProvisionBackend>), CloudActionError> {
        let row = self.console_cloud_row(instance_id, user_id).await?;
        if row.is_free() {
            return Err(CloudActionError::NotSupported("ssh_keys"));
        }
        let backend = self.backend_for_row(&row).await?;
        if backend.provider() != super::orgo::ORGO_SCHEME {
            return Err(CloudActionError::NotSupported("ssh_keys"));
        }
        Ok((row, backend))
    }

    /// `GET /api/v1/computers/:id/ssh-keys`: the stored keys, oldest first
    /// (expired agent keys stay listed until the computer has dropped them),
    /// and the connect details while the computer runs. Never wakes it.
    pub async fn ssh_keys(
        &self,
        instance_id: &str,
        user_id: &str,
    ) -> Result<SshKeyList, CloudActionError> {
        let (row, backend) = self.ssh_key_row(instance_id, user_id).await?;
        let rows: Vec<KeyRow> = sqlx::query_as(&format!(
            "SELECT {KEY_COLUMNS} FROM computer_ssh_keys WHERE computer_kind = 'cloud' AND computer_id = $1 ORDER BY created_at, id"
        ))
        .bind(&row.id)
        .fetch_all(&self.db)
        .await
        .map_err(ApiError::from)?;
        let connect = if !rows.is_empty() && row.status == "running" {
            match backend.ssh_endpoint(&row.incus_name).await {
                Ok(endpoint) => endpoint,
                Err(error) => {
                    tracing::warn!(id = %row.id, %error, "ssh keys: connect details unreadable");
                    None
                }
            }
        } else {
            None
        };
        Ok(SshKeyList {
            keys: rows.iter().map(KeyRow::view).collect(),
            connect,
        })
    }

    /// `POST /api/v1/computers/:id/ssh-keys`: check the key, then put the
    /// whole set on the computer, then store it. Returns the new key and
    /// whether Orgo needs one restart before it works.
    pub async fn add_ssh_key(
        &self,
        instance_id: &str,
        user_id: &str,
        input: SshKeyInput,
    ) -> Result<(SshKeyView, bool), CloudActionError> {
        let agent = match input.kind.as_deref().map(str::trim) {
            None | Some("") | Some("computer") => false,
            Some("agent") => true,
            Some(_) => {
                return Err(refused(
                    "invalid_kind",
                    "A key is either a computer key or an agent key.",
                ))
            }
        };
        let now = Utc::now();
        let expires_at = match (agent, input.expires_in_minutes) {
            (false, None) => None,
            (false, Some(_)) => {
                return Err(refused(
                    "invalid_expiry",
                    "Only agent keys expire. Leave out expiresInMinutes for your own key.",
                ))
            }
            (true, minutes) => {
                let minutes = minutes.unwrap_or(AGENT_KEY_DEFAULT_MINUTES);
                if !(AGENT_KEY_MIN_MINUTES..=AGENT_KEY_MAX_MINUTES).contains(&minutes) {
                    return Err(refused(
                        "invalid_expiry",
                        "An agent key lasts from 5 minutes to 24 hours.",
                    ));
                }
                Some(now + Duration::minutes(minutes))
            }
        };
        let key = parse_public_key(input.public_key.as_deref().unwrap_or_default())?;
        let label = input
            .label
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .or(key.comment.as_deref())
            .unwrap_or(if agent { "Agent key" } else { "SSH key" });
        let label: String = label
            .chars()
            .filter(|c| !c.is_control())
            .take(LABEL_MAX_CHARS)
            .collect();
        let (row, backend) = self.ssh_key_row(instance_id, user_id).await?;
        let new = KeyRow {
            id: format!("sshk_{}", Uuid::new_v4().simple()),
            label,
            public_key: key.line,
            fingerprint: key.fingerprint,
            kind: if agent { "agent" } else { "computer" }.to_string(),
            created_at: now,
            expires_at,
        };
        let (added, restart) = self
            .change_ssh_keys(&row, backend.as_ref(), KeyChange::Add(new))
            .await?;
        Ok((added.expect("an add returns its key"), restart))
    }

    /// `DELETE /api/v1/computers/:id/ssh-keys/:keyId`: put the set without
    /// that key on the computer (SSH off when it was the last), then forget
    /// it. Returns whether Orgo needs one restart.
    pub async fn remove_ssh_key(
        &self,
        instance_id: &str,
        user_id: &str,
        key_id: &str,
    ) -> Result<bool, CloudActionError> {
        let (row, backend) = self.ssh_key_row(instance_id, user_id).await?;
        let (_, restart) = self
            .change_ssh_keys(
                &row,
                backend.as_ref(),
                KeyChange::Remove(key_id.to_string()),
            )
            .await?;
        Ok(restart)
    }

    /// One key change under a lock on the instance row: read the set, apply
    /// `change`, drop expired agent keys, send the set to the computer, and
    /// write the rows only after the computer took it.
    async fn change_ssh_keys(
        &self,
        row: &InstanceRow,
        backend: &dyn ProvisionBackend,
        change: KeyChange,
    ) -> Result<(Option<SshKeyView>, bool), CloudActionError> {
        if !matches!(
            row.status.as_str(),
            "running" | "stopped" | "sleeping" | "suspended"
        ) {
            return Err(CloudActionError::Busy {
                status: row.status.clone(),
            });
        }
        let now = Utc::now();
        let mut tx = self.db.begin().await.map_err(ApiError::from)?;
        sqlx::query("SELECT id FROM provisioned_instances WHERE id = $1 FOR UPDATE")
            .bind(&row.id)
            .execute(&mut *tx)
            .await
            .map_err(ApiError::from)?;
        let current: Vec<KeyRow> = sqlx::query_as(&format!(
            "SELECT {KEY_COLUMNS} FROM computer_ssh_keys WHERE computer_kind = 'cloud' AND computer_id = $1 ORDER BY created_at, id"
        ))
        .bind(&row.id)
        .fetch_all(&mut *tx)
        .await
        .map_err(ApiError::from)?;
        let expired: Vec<String> = current
            .iter()
            .filter(|k| !k.active(now))
            .map(|k| k.id.clone())
            .collect();
        let mut keep: Vec<&KeyRow> = current.iter().filter(|k| k.active(now)).collect();
        let mut removed: Option<String> = None;
        match &change {
            KeyChange::Add(new) => {
                if current.iter().any(|k| k.fingerprint == new.fingerprint) {
                    return Err(CloudActionError::SshKeyExists);
                }
                if keep.len() >= MAX_KEYS_PER_COMPUTER {
                    return Err(refused(
                        "too_many_keys",
                        "This computer already has 20 keys. Remove one first. Nothing was changed.",
                    ));
                }
                keep.push(new);
            }
            KeyChange::Remove(key_id) => {
                if !current.iter().any(|k| &k.id == key_id) {
                    return Err(CloudActionError::SshKeyNotFound);
                }
                keep.retain(|k| &k.id != key_id);
                removed = Some(key_id.clone());
            }
            KeyChange::Prune => {
                if expired.is_empty() {
                    return Ok((None, false));
                }
            }
        }
        let lines: Vec<String> = keep.iter().map(|k| k.public_key.clone()).collect();
        if lines.iter().map(|l| l.len() + 1).sum::<usize>() > MAX_KEY_SET_BYTES {
            return Err(refused(
                "too_many_keys",
                "This computer can hold 16 KB of keys. Remove one first. Nothing was changed.",
            ));
        }
        let restart = if lines.is_empty() {
            match backend.disable_ssh(&row.incus_name).await {
                // Already gone on the provider: nothing left to remove.
                Ok(()) | Err(ProvisionError::NotFound(_)) => false,
                Err(error) => return Err(provider_error(error, &row.status)),
            }
        } else {
            backend
                .set_ssh_keys(&row.incus_name, &lines)
                .await
                .map_err(|error| provider_error(error, &row.status))?
        };
        let gone: Vec<String> = expired.into_iter().chain(removed).collect();
        if !gone.is_empty() {
            sqlx::query("DELETE FROM computer_ssh_keys WHERE computer_kind = 'cloud' AND computer_id = $1 AND id = ANY($2)")
                .bind(&row.id)
                .bind(&gone)
                .execute(&mut *tx)
                .await
                .map_err(ApiError::from)?;
        }
        let mut added = None;
        if let KeyChange::Add(new) = &change {
            sqlx::query(
                r#"
                INSERT INTO computer_ssh_keys
                    (id, user_id, computer_kind, computer_id, kind, label, public_key, fingerprint, created_at, expires_at)
                VALUES ($1, $2, 'cloud', $3, $4, $5, $6, $7, $8, $9)
                "#,
            )
            .bind(&new.id)
            .bind(&row.user_id)
            .bind(&row.id)
            .bind(&new.kind)
            .bind(&new.label)
            .bind(&new.public_key)
            .bind(&new.fingerprint)
            .bind(new.created_at)
            .bind(new.expires_at)
            .execute(&mut *tx)
            .await
            .map_err(ApiError::from)?;
            added = Some(new.view());
        }
        if let Err(error) = tx.commit().await {
            // The computer already has the new set; the next change or
            // sweep sends the stored set again.
            tracing::error!(id = %row.id, %error, "ssh keys: computer updated but the list was not saved");
            return Err(CloudActionError::Api(ApiError::from(error)));
        }
        Ok((added, restart))
    }

    /// Removes expired agent keys from each computer that has one. A
    /// computer that refuses (asleep, busy) keeps its rows for the next
    /// sweep; a deleted computer's rows are dropped.
    pub async fn sweep_expired_ssh_keys(&self, now: DateTime<Utc>) {
        let ids: Vec<String> = match sqlx::query_scalar(
            "SELECT DISTINCT computer_id FROM computer_ssh_keys WHERE computer_kind = 'cloud' AND expires_at <= $1 LIMIT 200",
        )
        .bind(now)
        .fetch_all(&self.db)
        .await
        {
            Ok(ids) => ids,
            Err(error) => {
                tracing::warn!(%error, "ssh key sweep: list failed");
                return;
            }
        };
        for id in ids {
            let row = match self.fetch_row(&id, None).await {
                Ok(row) if row.status != "deleted" => row,
                Ok(_) | Err(ApiError::NotFound(_)) => {
                    forget_ssh_keys(&self.db, &id).await;
                    continue;
                }
                Err(error) => {
                    tracing::warn!(%id, %error, "ssh key sweep: computer unreadable");
                    continue;
                }
            };
            let backend = match self.backend_for_row(&row).await {
                Ok(backend) => backend,
                Err(error) => {
                    tracing::warn!(%id, %error, "ssh key sweep: no backend");
                    continue;
                }
            };
            if let Err(error) = self
                .change_ssh_keys(&row, backend.as_ref(), KeyChange::Prune)
                .await
            {
                tracing::info!(%id, error = ?error.parts().1["error"], "ssh key sweep: kept expired keys for the next sweep");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ED25519: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl eoj@laptop";

    #[test]
    fn a_public_key_is_checked_and_fingerprinted_like_openssh() {
        let key = parse_public_key(&format!("  {ED25519}\n")).unwrap();
        assert_eq!(key.key_type, "ssh-ed25519");
        assert_eq!(key.comment.as_deref(), Some("eoj@laptop"));
        assert_eq!(key.line, ED25519);
        // What `ssh-keygen -lf` prints for this key.
        assert_eq!(
            key.fingerprint,
            "SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU"
        );
        assert_eq!(
            key.fingerprint,
            parse_public_key(ED25519.trim_end_matches(" eoj@laptop"))
                .unwrap()
                .fingerprint
        );
    }

    #[test]
    fn private_keys_and_broken_keys_are_refused_with_a_plain_message() {
        let code = |text: &str| match parse_public_key(text).unwrap_err() {
            CloudActionError::SshKeyRefused { code, message } => {
                assert!(!message.is_empty());
                code
            }
            other => panic!("{other:?}"),
        };
        assert_eq!(code("-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----"), "private_key");
        assert_eq!(code("-----BEGIN RSA PRIVATE KEY-----\nMIIE"), "private_key");
        assert_eq!(code("PuTTY-User-Key-File-3: ssh-ed25519"), "private_key");
        assert_eq!(code(""), "public_key_required");
        assert_eq!(code(&format!("{ED25519}\n{ED25519}")), "one_key_only");
        assert_eq!(
            code(&format!("command=\"ls\" {ED25519}")),
            "invalid_public_key"
        );
        assert_eq!(code("ssh-dss AAAAB3NzaC1kc3MAAACBAP"), "invalid_public_key");
        // The blob names a different type than the line.
        assert_eq!(
            code("ssh-rsa AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl"),
            "invalid_public_key"
        );
        // Cut off: the type is there, the key material is not.
        assert_eq!(
            code("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5"),
            "invalid_public_key"
        );
        assert_eq!(code("ssh-ed25519 not-base64!"), "invalid_public_key");
    }
}
