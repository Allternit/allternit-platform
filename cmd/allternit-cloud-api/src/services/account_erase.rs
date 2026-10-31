//! Account erase, run when Clerk reports `user.deleted`.
//!
//! In order: stop every charge, take back what costs money (computers, cloud
//! phones, numbers, mailboxes), delete stored objects, then erase the rows in
//! one transaction. Every step can run again: Clerk retries a failed delivery,
//! so a failed step answers non-2xx and the rows stay until the retry succeeds.
//!
//! The `users` row stays as an empty tombstone. The kept finance tables have
//! `ON DELETE CASCADE` foreign keys to it, and a late Stripe webhook still
//! writes against it. Clerk never reuses an id, and the email is cleared, so
//! signing up again starts a new account.
//!
//! [`ERASE`] and [`RETAIN`] name every table that holds a user or owner column.
//! The test below reads `migrations_pg` and fails on a table in neither list.

use sqlx::{PgPool, Row};

use crate::{
    error::ApiError,
    routes::billing_checkout::{ReqwestStripeCheckout, StripeCheckout},
    services::{
        cloud_phones::CloudPhoneService,
        r2::{ObjectStore, R2Client},
    },
    ApiState,
};

const USER: &str = "user_id = $1";
const OWNER: &str = "owner_id = $1";
const OWNER_USER: &str = "owner_user_id = $1";
const PROJECT: &str = "project_id IN (SELECT id FROM platform_projects WHERE owner_user_id = $1)";

/// Kept after an account is erased, with the reason. Holds no contact details:
/// see [`SCRUB`]. Only finance, dispute and audit records belong here.
pub const RETAIN: &[(&str, &str)] = &[
    ("users", "empty tombstone: the id only, see the module comment"),
    ("audit_log", "audit trail, contact fields cleared"),
    ("audit_logs", "audit trail, address cleared"),
    ("credit_transactions", "finance: credit ledger"),
    ("billing_entitlement_events", "finance: what a payment granted"),
    ("billing_subscriptions", "finance: subscription mirror, marked canceled"),
    ("user_billing_accounts", "finance: links the id to the Stripe customer"),
    ("billing_disputes", "finance: chargebacks and disputes"),
    ("billing_auto_topup_attempts", "finance: charge attempts"),
    ("voice_billing_periods", "finance: voice invoices, unbilled ones closed"),
    ("inference_usage", "finance: token counts and cost, no content"),
    ("artifact_versions", "versions of other people's artifacts, author cleared"),
];

/// `(table, condition)`, run in this order with the user id as `$1`. A table
/// that another table references without a cascade comes after that table.
pub const ERASE: &[(&str, &str)] = &[
    // Platform API projects, children first.
    ("platform_webhook_deliveries", "webhook_id IN (SELECT id FROM platform_webhooks WHERE user_id = $1 OR project_id IN (SELECT id FROM platform_projects WHERE owner_user_id = $1)) OR event_id IN (SELECT id FROM platform_events WHERE user_id = $1 OR project_id IN (SELECT id FROM platform_projects WHERE owner_user_id = $1))"),
    ("platform_webhooks", "user_id = $1 OR project_id IN (SELECT id FROM platform_projects WHERE owner_user_id = $1)"),
    ("platform_events", "user_id = $1 OR project_id IN (SELECT id FROM platform_projects WHERE owner_user_id = $1)"),
    ("platform_call_transcript", "call_id IN (SELECT id FROM platform_calls WHERE project_id IN (SELECT id FROM platform_projects WHERE owner_user_id = $1))"),
    ("platform_calls", PROJECT),
    ("platform_call_slots", PROJECT),
    ("platform_computer_events", PROJECT),
    ("platform_computers", "owner_user_id = $1 OR project_id IN (SELECT id FROM platform_projects WHERE owner_user_id = $1)"),
    ("platform_conversation_messages", "conversation_id IN (SELECT id FROM platform_conversations WHERE project_id IN (SELECT id FROM platform_projects WHERE owner_user_id = $1))"),
    ("platform_conversations", PROJECT),
    ("platform_channel_connections", PROJECT),
    ("platform_knowledge_files", PROJECT),
    ("platform_mail_messages", "mailbox_id IN (SELECT id FROM platform_mailboxes WHERE project_id IN (SELECT id FROM platform_projects WHERE owner_user_id = $1))"),
    ("platform_identities", PROJECT),
    ("platform_mailboxes", PROJECT),
    ("platform_memory", PROJECT),
    ("platform_messages", PROJECT),
    ("platform_model_keys", PROJECT),
    ("platform_project_tracing", PROJECT),
    ("platform_spend_alerts", PROJECT),
    ("platform_credit_grants", PROJECT),
    ("platform_usage_events", PROJECT),
    ("platform_phone_fleet_runs", PROJECT),
    ("platform_phone_runs", OWNER_USER),
    ("platform_idempotency", "key_id IN (SELECT id FROM api_keys WHERE user_id = $1 OR project_id IN (SELECT id FROM platform_projects WHERE owner_user_id = $1))"),
    ("api_keys", "user_id = $1 OR project_id IN (SELECT id FROM platform_projects WHERE owner_user_id = $1)"),
    ("platform_agents", PROJECT),
    ("platform_accounts", PROJECT),
    ("platform_projects", OWNER_USER),
    // Artifacts. Comments and suggestions on other people's artifacts go too.
    ("artifact_comments", "author_id = $1"),
    ("artifact_suggestions", "author_id = $1"),
    ("artifact_shares", "principal_type = 'user' AND principal_id = $1"),
    ("artifact_consents", USER),
    ("artifact_presence", USER),
    ("artifact_storage", USER),
    ("artifact_library_archives", USER),
    ("motion_render_jobs", USER),
    ("artifacts", OWNER),
    // Runs, schedules, tasks.
    ("mirror_sessions", USER),
    ("attachments", USER),
    ("runs", OWNER),
    ("schedules", OWNER),
    ("task_comments", "task_id IN (SELECT id FROM tasks WHERE owner_id = $1)"),
    ("task_queue", "task_id IN (SELECT id FROM tasks WHERE owner_id = $1)"),
    ("task_events", "task_id IN (SELECT id FROM tasks WHERE owner_id = $1)"),
    ("tasks", OWNER),
    // Computers and runtimes.
    ("provisioned_instance_wakes", USER),
    ("provisioned_instance_usage_sessions", USER),
    ("provisioned_instances", USER),
    ("hosted_runtime_usage_sessions", USER),
    ("hosted_runtime_instances", USER),
    ("cloud_phones", USER),
    ("computer_pairing_codes", USER),
    ("paired_computers", USER),
    ("code_environments", USER),
    ("gizzi_instances", USER),
    ("byo_bootstrap_tokens", USER),
    ("dispatch_handoff_tokens", USER),
    ("runtime_pairings", USER),
    ("runtime_presence_alerts", USER),
    ("runtime_failovers", USER),
    ("user_node_preferences", USER),
    ("registered_phones", "account_id = $1"),
    ("app_phones", "account = $1"),
    ("runtime_devices", USER),
    // Phone, voice and in-app calls.
    ("invite_contacts", "owner_user_id = $1 OR contact_user_id = $1"),
    ("inapp_calls", "caller_user_id = $1 OR callee_user_id = $1"),
    ("inapp_thread_events", "sender_user_id = $1"),
    ("inapp_threads", "owner_user_id = $1 OR contact_user_id = $1"),
    ("phone_invites", USER),
    ("call_consents", USER),
    ("phone_numbers", USER),
    ("phone_presence", USER),
    ("phone_call_prefs", USER),
    ("voice_calls", USER),
    ("voice_linked_call_rooms", OWNER),
    ("voice_bot_config", USER),
    ("voice_custom_voices", USER),
    ("voice_usage", USER),
    ("voice_tickets_used", USER),
    ("photo_avatar_usage", USER),
    // Channels. Each queue is keyed by an install only this user holds.
    ("teams_app_queue", "tenant_id IN (SELECT tenant_id FROM teams_installs WHERE user_id = $1)"),
    ("teams_conversation_refs", "tenant_id IN (SELECT tenant_id FROM teams_installs WHERE user_id = $1)"),
    ("teams_installs", USER),
    ("teams_app_states", USER),
    ("slack_event_queue", "team_id IN (SELECT team_id FROM slack_installs WHERE user_id = $1)"),
    ("slack_installs", USER),
    ("discord_webhooks", "guild_id IN (SELECT guild_id FROM discord_installs WHERE user_id = $1)"),
    ("discord_installs", USER),
    ("whatsapp_windows", "phone_number_id IN (SELECT phone_number_id FROM whatsapp_es_accounts WHERE user_id = $1)"),
    ("whatsapp_es_accounts", USER),
    ("whatsapp_signup_states", USER),
    ("telegram_onboarding", USER),
    ("mcp_event_client_subscriptions", USER),
    ("channel_inbound_routes", USER),
    ("bot_email_mailboxes", USER),
    ("bot_email_domains", USER),
    ("agent_contacts", OWNER_USER),
    // Notifications.
    ("push_log", USER),
    ("push_subscriptions", USER),
    ("apns_device_tokens", USER),
    ("notification_preferences", USER),
    ("notification_push_log", USER),
    ("notification_quiet_hours", USER),
    ("notification_held", USER),
    ("notification_repeat", USER),
    ("customer_emails", USER),
    // Access, sessions and account settings.
    ("api_tokens", USER),
    ("vendor_bot_keys", USER),
    ("provider_tokens", USER),
    ("user_inference_keys", USER),
    ("mcp_oauth_approvals", USER),
    ("user_sessions", USER),
    ("wizard_sessions", USER),
    ("team_sessions", USER),
    ("team_members_seen", USER),
    ("floor_shares", USER),
    ("user_files", USER),
    ("user_credits", USER),
    ("user_cost_budgets", USER),
    ("cost_alerts", USER),
    ("billing_auto_topup", USER),
    ("billing_purchase_trust", USER),
    ("user_trust", USER),
    ("user_pairing_usage", USER),
    ("user_relay_usage", USER),
    ("user_runtime_quotas", USER),
    ("user_open_lane", USER),
    ("open_lane_blocks", USER),
];

/// Clears the user from rows that stay: kept records, and work that belongs to
/// someone else. Runs after [`ERASE`], in the same transaction.
const SCRUB: &[&str] = &[
    "UPDATE artifact_versions SET author_id = 'deleted' WHERE author_id = $1",
    "UPDATE artifact_shares SET invited_by = 'deleted' WHERE invited_by = $1",
    "UPDATE phone_invites SET joined_user_id = NULL WHERE joined_user_id = $1",
    "UPDATE audit_log SET user_email = NULL, ip_address = NULL, user_agent = NULL WHERE user_id = $1",
    "UPDATE audit_logs SET ip_address = NULL WHERE user_id = $1",
    "UPDATE users SET email = NULL, name = NULL, avatar_url = NULL, tenant_id = NULL, status = 'inactive' WHERE id = $1",
];

/// Revoked at once, before anything that can fail and leave the account waiting
/// for a retry: nothing the user holds keeps working.
const REVOKE: &[&str] = &[
    "UPDATE api_tokens SET is_revoked = TRUE WHERE user_id = $1",
    "UPDATE api_keys SET revoked_at = NOW() WHERE revoked_at IS NULL AND (user_id = $1 OR project_id IN (SELECT id FROM platform_projects WHERE owner_user_id = $1))",
    "DELETE FROM vendor_bot_keys WHERE user_id = $1",
    "DELETE FROM floor_shares WHERE user_id = $1",
    "UPDATE runtime_devices SET status = 'revoked', revoked_at = CURRENT_TIMESTAMP WHERE user_id = $1 AND revoked_at IS NULL",
];

/// Stops the charges that run without a subscription: the auto top-up, and
/// voice overage that an admin has not billed yet. Nothing is refunded.
const STOP_CHARGES: &[&str] = &[
    "UPDATE billing_auto_topup SET enabled = FALSE WHERE user_id = $1",
    "UPDATE voice_billing_periods SET state = 'failed', error = 'account deleted', updated_at = now() WHERE user_id = $1 AND state IN ('pending', 'dry_run')",
];

/// Erases the account. `Err` means a step failed and Clerk should retry; rows
/// are erased only after every external step succeeded.
pub async fn erase_account(state: &ApiState, user_id: &str) -> Result<(), ApiError> {
    let db = &state.db;
    run_all(db, &["UPDATE users SET status = 'inactive' WHERE id = $1"], user_id).await?;
    run_all(db, STOP_CHARGES, user_id).await?;
    run_all(db, REVOKE, user_id).await?;

    let mut failures: Vec<String> = Vec::new();
    let secret = std::env::var("STRIPE_SECRET_KEY").ok().filter(|k| !k.is_empty());
    if let Err(e) = cancel_stripe(db, &ReqwestStripeCheckout::new(), secret.as_deref(), user_id).await {
        failures.push(format!("stripe: {e}"));
    }
    if let Err(e) = release_computers(state, user_id).await {
        failures.push(format!("computers: {e}"));
    }
    if let Err(e) = release_numbers(db, user_id).await {
        failures.push(format!("numbers: {e}"));
    }
    if let Err(e) = crate::routes::bot_email::remove_all_for_user(db, user_id).await {
        failures.push(format!("mailboxes: {e}"));
    }
    remove_platform_mailboxes(db, user_id).await.unwrap_or_else(|e| failures.push(format!("mailboxes: {e}")));
    let r2 = R2Client::from_env().ok();
    if let Err(e) = delete_objects(db, r2.as_ref().map(|c| c as &dyn ObjectStore), user_id).await {
        failures.push(format!("objects: {e}"));
    }
    if !failures.is_empty() {
        return Err(ApiError::ServiceUnavailable(failures.join("; ")));
    }
    erase_rows(db, user_id).await
}

async fn run_all(db: &PgPool, statements: &[&str], user_id: &str) -> Result<(), ApiError> {
    for statement in statements {
        sqlx::query(statement).bind(user_id).execute(db).await?;
    }
    Ok(())
}

/// Cancels the user's Stripe subscriptions at once, with no final invoice. The
/// mirror can miss one, so each known customer is also asked what it has.
pub(crate) async fn cancel_stripe(
    db: &PgPool,
    stripe: &dyn StripeCheckout,
    secret: Option<&str>,
    user_id: &str,
) -> Result<(), ApiError> {
    let mut subscriptions: Vec<String> = sqlx::query_scalar(
        "SELECT stripe_subscription_id FROM billing_subscriptions WHERE user_id = $1 AND status <> 'canceled' \
         UNION SELECT stripe_subscription_id FROM platform_projects WHERE owner_user_id = $1 AND stripe_subscription_id IS NOT NULL",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    let customers: Vec<String> = sqlx::query_scalar(
        "SELECT stripe_customer_id FROM user_billing_accounts WHERE user_id = $1 \
         UNION SELECT stripe_customer_id FROM billing_subscriptions WHERE user_id = $1 AND stripe_customer_id IS NOT NULL \
         UNION SELECT stripe_customer_id FROM billing_auto_topup WHERE user_id = $1 AND stripe_customer_id IS NOT NULL \
         UNION SELECT stripe_customer_id FROM voice_billing_periods WHERE user_id = $1 AND stripe_customer_id IS NOT NULL \
         UNION SELECT stripe_customer_id FROM platform_projects WHERE owner_user_id = $1 AND stripe_customer_id IS NOT NULL",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    if subscriptions.is_empty() && customers.is_empty() {
        return Ok(());
    }
    let secret = secret.ok_or_else(|| ApiError::ServiceUnavailable("STRIPE_SECRET_KEY is not set".to_string()))?;

    for customer in &customers {
        let query = [
            ("customer".to_string(), customer.clone()),
            ("status".to_string(), "all".to_string()),
            ("limit".to_string(), "100".to_string()),
        ];
        match stripe.get_object(secret, "/v1/subscriptions", &query).await {
            Ok(list) => {
                for sub in list["data"].as_array().into_iter().flatten() {
                    if let (Some(id), false) = (sub["id"].as_str(), is_finished(sub["status"].as_str())) {
                        subscriptions.push(id.to_string());
                    }
                }
            }
            Err(e) if is_missing(&e) => {}
            Err(e) => return Err(e),
        }
    }
    subscriptions.sort();
    subscriptions.dedup();

    for id in &subscriptions {
        let path = format!("/v1/subscriptions/{id}");
        match stripe.get_object(secret, &path, &[]).await {
            Ok(sub) if is_finished(sub["status"].as_str()) => {}
            Ok(_) => match stripe.delete_object(secret, &path, &[]).await {
                Ok(_) => {}
                Err(e) if is_missing(&e) => {}
                Err(e) => return Err(e),
            },
            Err(e) if is_missing(&e) => {}
            Err(e) => return Err(e),
        }
        crate::routes::billing_subscriptions::mark_billing_subscription_canceled(db, id).await?;
    }
    Ok(())
}

fn is_finished(status: Option<&str>) -> bool {
    matches!(status, Some("canceled" | "incomplete_expired"))
}

/// Stripe's answer for an object that is already gone.
fn is_missing(error: &ApiError) -> bool {
    error.to_string().contains("No such ")
}

async fn release_computers(state: &ApiState, user_id: &str) -> Result<(), ApiError> {
    state.provisioning_service.delete_all_for_user(user_id).await?;
    crate::routes::hosted_runtimes::destroy_all_for_user(state, user_id).await?;
    let phones: Vec<String> =
        sqlx::query_scalar("SELECT id FROM cloud_phones WHERE user_id = $1 AND status <> 'deleted'")
            .bind(user_id)
            .fetch_all(&state.db)
            .await?;
    let service = CloudPhoneService::new(state.db.clone());
    for id in phones {
        service.delete(&id, user_id).await?;
    }
    Ok(())
}

/// Gives numbers back to the carrier, which stops their monthly charge.
async fn release_numbers(db: &PgPool, user_id: &str) -> Result<(), String> {
    use crate::routes::{phone, platform_v1::calls};
    let rows = sqlx::query("SELECT id, simulated FROM phone_numbers WHERE user_id = $1 AND released_at IS NULL")
        .bind(user_id)
        .fetch_all(db)
        .await
        .map_err(|e| e.to_string())?;
    for row in rows {
        let id: String = row.get("id");
        let simulated: bool = row.get("simulated");
        calls::unprovision(db, &id, None).await;
        let number = phone::number_for_user(db, user_id, &id).await.map_err(|e| format!("{e:?}"))?;
        let carrier = if simulated { None } else { Some(phone::carrier().map_err(|e| format!("{e:?}"))?) };
        phone::release_platform_number(db, carrier.as_deref(), &number).await.map_err(|e| format!("{e:?}"))?;
    }
    Ok(())
}

async fn remove_platform_mailboxes(db: &PgPool, user_id: &str) -> Result<(), String> {
    use crate::routes::platform_v1::mailboxes::{MailHost, ProdMailHost};
    let rows = sqlx::query(&format!(
        "SELECT mail_mailbox_id, webhook_id FROM platform_mailboxes WHERE deleted_at IS NULL AND {PROJECT}"
    ))
    .bind(user_id)
    .fetch_all(db)
    .await
    .map_err(|e| e.to_string())?;
    for row in rows {
        let mailbox: String = row.get("mail_mailbox_id");
        let webhook: Option<String> = row.get("webhook_id");
        ProdMailHost.delete(&mailbox, webhook.as_deref()).await;
    }
    Ok(())
}

/// Every stored object of this user, as `(bucket, key)`.
async fn object_keys(db: &PgPool, user_id: &str) -> Result<Vec<(String, String)>, sqlx::Error> {
    use crate::routes::voice_recordings;
    use crate::services::user_files::BUCKET as FILES;
    let mut keys = Vec::new();
    let mut add = |bucket: &str, found: Vec<String>| {
        keys.extend(found.into_iter().map(|key| (bucket.to_string(), key)));
    };
    add(FILES, sqlx::query_scalar("SELECT key FROM user_files WHERE user_id = $1 AND deleted_at IS NULL").bind(user_id).fetch_all(db).await?);
    add(FILES, sqlx::query_scalar("SELECT clip_key FROM voice_custom_voices WHERE user_id = $1 AND clip_key IS NOT NULL").bind(user_id).fetch_all(db).await?);
    add(FILES, sqlx::query_scalar(&format!("SELECT storage_key FROM platform_knowledge_files WHERE deleted_at IS NULL AND {PROJECT}")).bind(user_id).fetch_all(db).await?);
    add(
        FILES,
        sqlx::query_scalar(
            "SELECT output_key FROM motion_render_jobs WHERE storage = 'r2' AND output_key IS NOT NULL \
             AND (user_id = $1 OR artifact_id IN (SELECT id FROM artifacts WHERE owner_id = $1))",
        )
        .bind(user_id)
        .fetch_all(db)
        .await?,
    );
    let calls: Vec<String> = sqlx::query_scalar("SELECT call_id FROM voice_calls WHERE user_id = $1").bind(user_id).fetch_all(db).await?;
    add(voice_recordings::RECORDINGS_BUCKET, calls.iter().map(|id| voice_recordings::default_key(id)).collect());
    add(
        voice_recordings::RECORDINGS_BUCKET,
        sqlx::query_scalar(
            "SELECT payload->>'recordingRef' FROM voice_call_events WHERE payload ? 'recordingRef' \
             AND call_id IN (SELECT call_id FROM voice_calls WHERE user_id = $1)",
        )
        .bind(user_id)
        .fetch_all(db)
        .await?,
    );
    add(
        voice_recordings::RECORDINGS_BUCKET,
        sqlx::query_scalar(&format!("SELECT recording_ref FROM platform_calls WHERE recording_ref IS NOT NULL AND {PROJECT}")).bind(user_id).fetch_all(db).await?,
    );
    Ok(keys)
}

pub(crate) async fn delete_objects(db: &PgPool, store: Option<&dyn ObjectStore>, user_id: &str) -> Result<(), String> {
    let keys = object_keys(db, user_id).await.map_err(|e| e.to_string())?;
    if keys.is_empty() {
        return Ok(());
    }
    let store = store.ok_or("object storage is not configured")?;
    for (bucket, key) in keys {
        store.delete(&bucket, &key).await.map_err(|e| format!("{bucket}/{key}: {e}"))?;
    }
    Ok(())
}

/// Erases and scrubs in one transaction, so a failure leaves nothing half done.
pub(crate) async fn erase_rows(db: &PgPool, user_id: &str) -> Result<(), ApiError> {
    let mut tx = db.begin().await?;
    for (table, condition) in ERASE {
        sqlx::query(&format!("DELETE FROM {table} WHERE {condition}")).bind(user_id).execute(&mut *tx).await?;
    }
    for statement in SCRUB {
        sqlx::query(statement).bind(user_id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::r2::R2Error;
    use serde_json::{json, Value};
    use sqlx::Executor;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::Duration;

    fn migrations() -> Vec<PathBuf> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations_pg");
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
        files.retain(|p| p.extension().is_some_and(|e| e == "sql"));
        files.sort();
        files
    }

    fn holds_a_person(column: &str) -> bool {
        let c = column.to_ascii_lowercase();
        c.ends_with("user_id") || c.contains("owner") || c.starts_with("author") || c == "invited_by"
            || c == "account" || c == "account_id" || c == "project_id" || c.contains("email")
    }

    fn table_name(word: &str) -> String {
        word.trim_matches('"').rsplit('.').next().unwrap().trim_matches('"').to_ascii_lowercase()
    }

    /// Table name → its columns that can hold a user, from `CREATE TABLE` and
    /// `ALTER TABLE … ADD COLUMN` across all the migrations.
    fn person_columns() -> BTreeMap<String, BTreeSet<String>> {
        let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for file in migrations() {
            let sql = std::fs::read_to_string(file).unwrap();
            let mut current: Option<String> = None;
            for line in sql.lines() {
                let line = line.split("--").next().unwrap().trim();
                let upper = line.to_ascii_uppercase();
                let words: Vec<&str> = line.split_whitespace().collect();
                if upper.starts_with("CREATE TABLE") {
                    let at = if upper.contains("IF NOT EXISTS") { 5 } else { 2 };
                    let name = table_name(words[at].split('(').next().unwrap());
                    current = Some(name);
                } else if line.starts_with(");") || line.starts_with(')') {
                    current = None;
                } else if let Some(table) = &current {
                    if let Some(column) = words.first() {
                        let column = column.trim_matches('"');
                        if holds_a_person(column) {
                            found.entry(table.clone()).or_default().insert(column.to_string());
                        }
                    }
                }
                if upper.starts_with("ALTER TABLE") {
                    let at = if upper.starts_with("ALTER TABLE ONLY") || upper.starts_with("ALTER TABLE IF EXISTS") { 3 } else { 2 };
                    let at = if upper.starts_with("ALTER TABLE IF EXISTS") { 4 } else { at };
                    let table = words.get(at).map(|w| table_name(w));
                    let mut rest = upper.as_str();
                    while let Some(i) = rest.find("ADD COLUMN") {
                        let original = &line[line.len() - rest.len() + i + "ADD COLUMN".len()..];
                        let after = original.trim_start();
                        let after = if after.to_ascii_uppercase().starts_with("IF NOT EXISTS") { after["IF NOT EXISTS".len()..].trim_start() } else { after };
                        let column = after.split_whitespace().next().unwrap_or("").trim_matches('"');
                        if let (Some(table), true) = (&table, holds_a_person(column)) {
                            found.entry(table.clone()).or_default().insert(column.to_string());
                        }
                        rest = &rest[i + "ADD COLUMN".len()..];
                    }
                }
            }
        }
        found
    }

    #[test]
    fn every_table_with_a_person_is_classified() {
        let classified: BTreeSet<&str> = ERASE.iter().map(|(t, _)| *t).chain(RETAIN.iter().map(|(t, _)| *t)).collect();
        let found = person_columns();
        assert!(found.len() > 80, "the migration parser found only {} tables", found.len());
        let missing: Vec<String> = found
            .iter()
            .filter(|(table, _)| !classified.contains(table.as_str()))
            .map(|(table, columns)| format!("{table} {columns:?}"))
            .collect();
        assert!(missing.is_empty(), "add these to ERASE or RETAIN in account_erase.rs: {missing:#?}");
    }

    #[test]
    fn no_table_is_both_erased_and_retained() {
        for (table, _) in RETAIN {
            assert!(!ERASE.iter().any(|(t, _)| t == table), "{table} is in both lists");
        }
        let unique: BTreeSet<&str> = ERASE.iter().map(|(t, _)| *t).collect();
        assert_eq!(unique.len(), ERASE.len(), "ERASE lists a table twice");
    }

    #[derive(Default)]
    struct FakeStripe {
        status: Mutex<BTreeMap<String, String>>,
        deleted: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl StripeCheckout for FakeStripe {
        async fn create_checkout_session(&self, _: &str, _: &[(String, String)]) -> Result<String, ApiError> {
            unreachable!()
        }
        async fn create_billing_portal_session(&self, _: &str, _: &[(String, String)]) -> Result<String, ApiError> {
            unreachable!()
        }
        async fn get_object(&self, _: &str, path: &str, query: &[(String, String)]) -> Result<Value, ApiError> {
            let status = self.status.lock().unwrap();
            if path == "/v1/subscriptions" {
                assert_eq!(query[0].0, "customer");
                let data: Vec<Value> = status.iter().map(|(id, s)| json!({ "id": id, "status": s })).collect();
                return Ok(json!({ "data": data }));
            }
            let id = path.rsplit('/').next().unwrap();
            match status.get(id) {
                Some(s) => Ok(json!({ "id": id, "status": s })),
                None => Err(ApiError::Internal(format!("Stripe rejected the session: No such subscription: '{id}'"))),
            }
        }
        async fn delete_object(&self, _: &str, path: &str, _: &[(String, String)]) -> Result<Value, ApiError> {
            let id = path.rsplit('/').next().unwrap().to_string();
            self.status.lock().unwrap().insert(id.clone(), "canceled".to_string());
            self.deleted.lock().unwrap().push(id);
            Ok(json!({}))
        }
    }

    #[derive(Default)]
    struct FakeStore {
        deleted: Mutex<Vec<String>>,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl ObjectStore for FakeStore {
        fn presign_get(&self, _: &str, _: &str, _: Duration) -> Result<String, R2Error> {
            unreachable!()
        }
        async fn head(&self, _: &str, _: &str) -> Result<Option<u64>, R2Error> {
            unreachable!()
        }
        async fn delete(&self, bucket: &str, key: &str) -> Result<(), R2Error> {
            if self.fail {
                return Err(R2Error::Unavailable);
            }
            self.deleted.lock().unwrap().push(format!("{bucket}/{key}"));
            Ok(())
        }
    }

    /// A fresh database with the real migrations, or `None` without a server.
    async fn migrated_db() -> Option<(PgPool, String, String)> {
        let base = std::env::var("TEST_DATABASE_URL").ok()?;
        let admin = PgPool::connect(&base).await.ok()?;
        let name = format!("erase_{}", uuid::Uuid::new_v4().simple());
        admin.execute(format!("CREATE DATABASE {name}").as_str()).await.ok()?;
        let url = format!("{}/{name}", base.rsplit_once('/').unwrap().0);
        let pool = PgPool::connect(&url).await.unwrap();
        for file in migrations() {
            let sql = std::fs::read_to_string(&file).unwrap();
            pool.execute(sql.as_str()).await.unwrap_or_else(|e| panic!("{file:?}: {e}"));
        }
        Some((pool, base, name))
    }

    async fn drop_db(pool: PgPool, base: &str, name: &str) {
        pool.close().await;
        let admin = PgPool::connect(base).await.unwrap();
        admin.execute(format!("DROP DATABASE {name} WITH (FORCE)").as_str()).await.unwrap();
    }

    async fn count(db: &PgPool, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(db).await.unwrap()
    }

    #[tokio::test]
    async fn erase_on_the_real_schema() {
        let Some((db, base, name)) = migrated_db().await else {
            eprintln!("skipped: TEST_DATABASE_URL is not set or cannot create a database");
            return;
        };

        // Every statement is valid, and each table is emptied before a table it
        // references without a cascade.
        let fks = sqlx::query(
            "SELECT conrelid::regclass::text child, confrelid::regclass::text parent FROM pg_constraint \
             WHERE contype = 'f' AND confdeltype = 'a' AND conrelid <> confrelid",
        )
        .fetch_all(&db)
        .await
        .unwrap();
        let at = |table: &str| ERASE.iter().position(|(t, _)| *t == table);
        for fk in fks {
            let (child, parent): (String, String) = (fk.get("child"), fk.get("parent"));
            let (child, parent) = (child.replace("public.", ""), parent.replace("public.", ""));
            if let Some(p) = at(&parent) {
                let c = at(&child).unwrap_or_else(|| panic!("{child} references erased {parent} but is not erased"));
                assert!(c < p, "{child} must be erased before {parent}");
            }
        }

        // The list covers what the database itself says holds a person.
        let columns: Vec<(String, String)> = sqlx::query_as(
            "SELECT table_name::text, column_name::text FROM information_schema.columns c \
             JOIN information_schema.tables t USING (table_schema, table_name) \
             WHERE table_schema = 'public' AND table_type = 'BASE TABLE'",
        )
        .fetch_all(&db)
        .await
        .unwrap();
        for (table, column) in columns {
            if holds_a_person(&column) {
                assert!(
                    ERASE.iter().any(|(t, _)| *t == table) || RETAIN.iter().any(|(t, _)| *t == table),
                    "{table}.{column} is not classified"
                );
            }
        }

        for user in ["user_gone", "user_stays"] {
            db.execute(
                format!(
                    "INSERT INTO users (id, email, name) VALUES ('{user}', '{user}@example.com', 'Name');
                     INSERT INTO billing_subscriptions (stripe_subscription_id, user_id, plan_id, plan_tier, status, stripe_customer_id)
                       VALUES ('sub_{user}', '{user}', 'plus', 'plus', 'active', 'cus_{user}');
                     INSERT INTO credit_transactions (id, user_id, amount_usd, transaction_id, source)
                       VALUES ('ct_{user}', '{user}', 5, 'tx_{user}', 'test');
                     INSERT INTO user_credits (user_id, balance_usd) VALUES ('{user}', 5);
                     INSERT INTO billing_auto_topup (user_id, enabled, threshold_usd, amount_usd, monthly_cap_usd)
                       VALUES ('{user}', TRUE, 1, 5, 50);
                     INSERT INTO user_files (id, user_id, key, name, content_type, bytes)
                       VALUES (gen_random_uuid(), '{user}', 'files/{user}', 'a.txt', 'text/plain', 1);
                     INSERT INTO api_tokens (id, token_hash, name, user_id) VALUES ('t_{user}', 'h_{user}', 'n', '{user}');
                     INSERT INTO audit_log (id, action, resource_type, user_id, user_email, success)
                       VALUES ('a_{user}', 'x', 'y', '{user}', '{user}@example.com', TRUE);
                     INSERT INTO platform_projects (id, owner_user_id, name, env, stripe_subscription_id)
                       VALUES ('p_{user}', '{user}', 'n', 'live', 'sub_p_{user}');
                     INSERT INTO platform_accounts (id, project_id, name) VALUES ('ac_{user}', 'p_{user}', 'n');"
                )
                .as_str(),
            )
            .await
            .unwrap();
        }

        // Stripe: the mirrored one, the project's, and one only Stripe knows about.
        let stripe = FakeStripe::default();
        for id in ["sub_user_gone", "sub_p_user_gone", "sub_unmirrored"] {
            stripe.status.lock().unwrap().insert(id.to_string(), "active".to_string());
        }
        assert!(cancel_stripe(&db, &stripe, None, "user_gone").await.is_err(), "no key must fail so Clerk retries");
        cancel_stripe(&db, &stripe, Some("sk_test"), "user_gone").await.unwrap();
        let mut cancelled = stripe.deleted.lock().unwrap().clone();
        cancelled.sort();
        assert_eq!(cancelled, ["sub_p_user_gone", "sub_unmirrored", "sub_user_gone"]);
        // A retry cancels nothing twice, and a gone subscription is not an error.
        stripe.status.lock().unwrap().remove("sub_p_user_gone");
        cancel_stripe(&db, &stripe, Some("sk_test"), "user_gone").await.unwrap();
        assert_eq!(stripe.deleted.lock().unwrap().len(), 3);

        // Objects: a failure keeps the rows for the retry.
        let failing = FakeStore { fail: true, ..Default::default() };
        assert!(delete_objects(&db, Some(&failing), "user_gone").await.is_err());
        assert!(delete_objects(&db, None, "user_gone").await.is_err());
        let store = FakeStore::default();
        delete_objects(&db, Some(&store), "user_gone").await.unwrap();
        assert_eq!(store.deleted.lock().unwrap().clone(), ["allternit-user-files/files/user_gone"]);

        run_all(&db, STOP_CHARGES, "user_gone").await.unwrap();
        run_all(&db, REVOKE, "user_gone").await.unwrap();
        erase_rows(&db, "user_gone").await.unwrap();
        erase_rows(&db, "user_gone").await.unwrap(); // Clerk can deliver twice.

        for (sql, want) in [
            ("SELECT count(*) FROM user_credits WHERE user_id = 'user_gone'", 0),
            ("SELECT count(*) FROM user_files WHERE user_id = 'user_gone'", 0),
            ("SELECT count(*) FROM api_tokens WHERE user_id = 'user_gone'", 0),
            ("SELECT count(*) FROM billing_auto_topup WHERE user_id = 'user_gone'", 0),
            ("SELECT count(*) FROM platform_accounts WHERE project_id = 'p_user_gone'", 0),
            ("SELECT count(*) FROM platform_projects WHERE owner_user_id = 'user_gone'", 0),
            // Kept, with nothing that identifies a person.
            ("SELECT count(*) FROM credit_transactions WHERE user_id = 'user_gone'", 1),
            ("SELECT count(*) FROM billing_subscriptions WHERE user_id = 'user_gone' AND status = 'canceled'", 1),
            ("SELECT count(*) FROM audit_log WHERE user_id = 'user_gone' AND user_email IS NULL", 1),
            ("SELECT count(*) FROM users WHERE id = 'user_gone' AND email IS NULL AND name IS NULL", 1),
            // Someone else is untouched.
            ("SELECT count(*) FROM user_credits WHERE user_id = 'user_stays'", 1),
            ("SELECT count(*) FROM user_files WHERE user_id = 'user_stays'", 1),
            ("SELECT count(*) FROM api_tokens WHERE user_id = 'user_stays' AND is_revoked = FALSE", 1),
            ("SELECT count(*) FROM platform_accounts WHERE project_id = 'p_user_stays'", 1),
            ("SELECT count(*) FROM billing_auto_topup WHERE user_id = 'user_stays' AND enabled", 1),
            ("SELECT count(*) FROM users WHERE id = 'user_stays' AND email = 'user_stays@example.com'", 1),
        ] {
            assert_eq!(count(&db, sql).await, want, "{sql}");
        }

        drop_db(db, &base, &name).await;
    }
}
