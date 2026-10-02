//! Customer lifecycle emails, sent from our own mail server.
//!
//! Three emails, wording approved by Eoj (2026-10-01):
//! - **welcome** when someone signs up (Clerk `user.created` webhook),
//! - **plan_started** when a Stripe subscription first turns active,
//! - **computer_ready** when their paid cloud computer first pairs.
//!
//! cloud-api runs on the mail host, so mail goes through the local Postfix
//! (`/usr/sbin/sendmail`), whose amavis DKIM-signs `news.allternit.com`.
//! Every email is claimed first in `customer_emails` (UNIQUE user, kind,
//! ref), so webhook redeliveries never send twice; the row also feeds the
//! admin Customers page and the daily summary.
//!
//! Sending is on when `ALLTERNIT_CUSTOMER_EMAILS=1`. The team notes (new
//! sign-up, new paying customer) go out through [`super::ops_alert`] either way.

use std::process::Stdio;

use sqlx::PgPool;
use tokio::io::AsyncWriteExt;

const ENV_ENABLED: &str = "ALLTERNIT_CUSTOMER_EMAILS";
const ENV_FROM: &str = "ALLTERNIT_EMAIL_FROM";
const ENV_REPLY_TO: &str = "ALLTERNIT_EMAIL_REPLY_TO";
const ENV_SENDMAIL: &str = "ALLTERNIT_SENDMAIL";
const DEFAULT_FROM_NAME: &str = "Allternit";
const DEFAULT_FROM_ADDRESS: &str = "hello@news.allternit.com";
const DEFAULT_REPLY_TO: &str = "hello@allternit.com";
const DEFAULT_SENDMAIL: &str = "/usr/sbin/sendmail";

pub fn enabled() -> bool {
    matches!(std::env::var(ENV_ENABLED).as_deref(), Ok("1") | Ok("true"))
}

/// Who an email goes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    pub email: String,
    pub first_name: Option<String>,
}

/// A rendered email: plain text plus the branded HTML version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email {
    pub subject: String,
    pub body: String,
    pub html: String,
}

const ENV_ASSETS: &str = "ALLTERNIT_EMAIL_ASSETS";
const DEFAULT_ASSETS: &str = "https://allternit.com/email";
const INK: &str = "#1A1A1A";
const CORAL: &str = "#E07A5F";
const CREAM: &str = "#FDF8F3";
const CARD: &str = "#F3ECE3";
const MUTED: &str = "#6B6560";
const FONT: &str = "-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif";

fn assets() -> String {
    std::env::var(ENV_ASSETS).unwrap_or_else(|_| DEFAULT_ASSETS.to_string()).trim_end_matches('/').to_string()
}

fn first_name(recipient: &Recipient) -> Option<&str> {
    recipient.first_name.as_deref().map(str::trim).filter(|name| !name.is_empty())
}

fn greeting(recipient: &Recipient) -> String {
    match first_name(recipient) {
        Some(name) => format!("Hi {name},"),
        None => "Hi,".to_string(),
    }
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// HTML building blocks. All text passed in is plain text and gets escaped.
mod html {
    use super::*;

    pub fn heading(text: &str) -> String {
        format!(
            r#"<tr><td style="padding:8px 0 4px;font-family:{FONT};font-size:26px;line-height:32px;font-weight:700;color:{INK};">{}</td></tr>"#,
            escape(text)
        )
    }

    pub fn paragraph(text: &str) -> String {
        format!(
            r#"<tr><td style="padding:10px 0;font-family:{FONT};font-size:16px;line-height:24px;color:{INK};">{}</td></tr>"#,
            escape(text)
        )
    }

    pub fn bullets(items: &[&str]) -> String {
        let rows: String = items
            .iter()
            .map(|item| {
                format!(
                    r#"<tr><td width="18" valign="top" style="padding:4px 0;"><div style="width:8px;height:8px;margin-top:8px;background:{CORAL};border-radius:2px;"></div></td><td style="padding:4px 0;font-family:{FONT};font-size:16px;line-height:24px;color:{INK};">{}</td></tr>"#,
                    escape(item)
                )
            })
            .collect();
        format!(r#"<tr><td style="padding:6px 0;"><table role="presentation" cellpadding="0" cellspacing="0" border="0">{rows}</table></td></tr>"#)
    }

    pub fn button(label: &str, href: &str) -> String {
        format!(
            r#"<tr><td style="padding:18px 0 8px;"><a href="{}" style="display:inline-block;background:{INK};color:{CREAM};font-family:{FONT};font-size:15px;font-weight:600;text-decoration:none;padding:13px 22px;border-radius:8px;">{}</a></td></tr>"#,
            escape(href),
            escape(label)
        )
    }

    /// A beige card with rows of label/value.
    pub fn facts_card(title: &str, rows: &[(&str, String)]) -> String {
        let body: String = rows
            .iter()
            .map(|(label, value)| {
                format!(
                    r#"<tr><td style="padding:7px 0;font-family:{FONT};font-size:14px;color:{MUTED};">{}</td><td align="right" style="padding:7px 0;font-family:{FONT};font-size:14px;font-weight:600;color:{INK};">{}</td></tr>"#,
                    escape(label),
                    escape(value)
                )
            })
            .collect();
        format!(
            r#"<tr><td style="padding:14px 0;"><table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="background:{CARD};border-radius:12px;"><tr><td style="padding:18px 20px;"><div style="font-family:{FONT};font-size:11px;letter-spacing:1.6px;font-weight:700;color:{CORAL};text-transform:uppercase;padding-bottom:6px;">{}</div><table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0">{body}</table></td></tr></table></td></tr>"#,
            escape(title)
        )
    }

    /// A clickable image card with a caption.
    pub fn image_card(image: &str, alt: &str, href: &str, title: &str, caption: &str) -> String {
        format!(
            r#"<tr><td style="padding:14px 0;"><table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="background:{CARD};border-radius:12px;overflow:hidden;"><tr><td><a href="{href}"><img src="{image}" width="520" alt="{alt}" style="display:block;width:100%;max-width:520px;height:auto;border:0;border-radius:12px 12px 0 0;"></a></td></tr><tr><td style="padding:14px 20px 18px;"><div style="font-family:{FONT};font-size:16px;font-weight:700;color:{INK};">{title}</div><div style="padding-top:4px;font-family:{FONT};font-size:14px;line-height:21px;color:{MUTED};">{caption}</div></td></tr></table></td></tr>"#,
            href = escape(href),
            image = escape(image),
            alt = escape(alt),
            title = escape(title),
            caption = escape(caption),
        )
    }

    pub fn layout(preheader: &str, rows: &[String]) -> String {
        let assets = assets();
        format!(
            r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="light"><title>Allternit</title></head><body style="margin:0;padding:0;background:{CREAM};"><div style="display:none;max-height:0;overflow:hidden;opacity:0;">{preheader}</div><table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="background:{CREAM};"><tr><td align="center" style="padding:28px 16px;"><table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" style="max-width:520px;"><tr><td style="padding:4px 0 22px;"><a href="https://allternit.com"><img src="{assets}/wordmark.png" width="200" alt="A://TERNIT" style="display:block;width:200px;height:auto;border:0;"></a></td></tr>{rows}<tr><td style="padding:28px 0 0;border-top:1px solid #E6DDD2;font-family:{FONT};font-size:13px;line-height:20px;color:{MUTED};">Questions? Reply to this email and a person will answer.<br><a href="https://allternit.com" style="color:{MUTED};">allternit.com</a> &middot; <a href="https://ai.allternit.com" style="color:{MUTED};">ai.allternit.com</a> &middot; <a href="https://allternit.com/download" style="color:{MUTED};">Download</a></td></tr></table></td></tr></table></body></html>"#,
            preheader = escape(preheader),
            rows = rows.concat(),
        )
    }

    /// This month's featured film (see [`feature_for`]).
    pub fn feature_card(now: chrono::DateTime<chrono::Utc>) -> String {
        let feature = feature_for(now);
        image_card(
            &format!("{}/feature-{}.jpg", assets(), feature.id),
            &format!("Play: {}", feature.title),
            "https://allternit.com",
            feature.title,
            feature.caption,
        )
    }

    /// m.allternit.com on an iPhone; welcome email only.
    pub fn phone_card() -> String {
        image_card(
            &format!("{}/phone.jpg", assets()),
            "Allternit running on an iPhone, and Add to Home Screen",
            "https://m.allternit.com",
            "Use Allternit on your phone",
            "Open m.allternit.com in Safari, tap Share, then Add to Home Screen. On a computer, get the Desktop app at allternit.com/download.",
        )
    }
}

/// A film featured in the emails; one per month, in this order.
pub struct Feature {
    pub id: &'static str,
    pub title: &'static str,
    pub caption: &'static str,
}

/// Captions are the allternit.com film blurbs. Images are
/// `{assets}/feature-{id}.jpg` (the film poster with a play button).
pub const FEATURES: &[Feature] = &[
    Feature { id: "montage", title: "See what Allternit does", caption: "A short look at chat, agents, artifacts and code in Allternit." },
    Feature { id: "artifacts", title: "Artifacts", caption: "Ask for a site or a deck and open what comes back." },
    Feature { id: "gizzi-code", title: "Gizzi Code", caption: "The coding agent in your terminal." },
    Feature { id: "mcp-apps", title: "MCP Apps", caption: "Connect the services your agents work in." },
    Feature { id: "bots-threads", title: "Bots and Threads", caption: "Bot Mode became threads. Here is what changed." },
    Feature { id: "agency-kernel", title: "Agency Kernel", caption: "Rules, contracts and decisions for a team of agents." },
];

/// The feature for `now`'s month: October 2026 is the first, then one a month.
pub fn feature_for(now: chrono::DateTime<chrono::Utc>) -> &'static Feature {
    use chrono::Datelike;
    let months = i64::from(now.year()) * 12 + i64::from(now.month0()) - (2026 * 12 + 9);
    &FEATURES[months.rem_euclid(FEATURES.len() as i64) as usize]
}

pub fn welcome_email(recipient: &Recipient) -> Email {
    let body = format!(
        "{}\n\n\
         Your Allternit account is set up. You can sign in at ai.allternit.com or in the Allternit Desktop app.\n\n\
         - Use your own models and computers for free.\n\
         - Plans and credit packs are in Settings → Billing if you want Allternit Cloud.\n\n\
         On your phone: open m.allternit.com in Safari, tap Share, then Add to Home Screen.\n\
         On a computer: download the Desktop app at allternit.com/download.\n\n\
         Questions? Reply to this email.\n\n\
         — Allternit\n",
        greeting(recipient)
    );
    let html = html::layout(
        "Your Allternit account is set up.",
        &[
            html::heading("Welcome to Allternit"),
            html::paragraph(&greeting(recipient)),
            html::paragraph("Your Allternit account is set up. You can sign in at ai.allternit.com or in the Allternit Desktop app."),
            html::bullets(&[
                "Use your own models and computers for free.",
                "Plans and credit packs are in Settings → Billing if you want Allternit Cloud.",
            ]),
            html::button("Open Allternit", "https://ai.allternit.com"),
            html::feature_card(chrono::Utc::now()),
            html::phone_card(),
        ],
    );
    Email { subject: "Welcome to Allternit".to_string(), body, html }
}

/// `plan_label` e.g. "Plus"; `monthly_credits_usd` e.g. 22.0; size from the plan.
pub fn plan_started_email(
    recipient: &Recipient,
    plan_label: &str,
    monthly_credits_usd: f64,
    cpu_cores: i64,
    memory_mb: i64,
) -> Email {
    let credits = format_usd(monthly_credits_usd);
    let memory_gb = memory_mb / 1024;
    let body = format!(
        "{}\n\n\
         Thanks for subscribing. Your plan includes ${credits} in Allternit Cloud credits each month and a cloud computer ({cpu_cores} vCPU, {memory_gb} GB).\n\n\
         We're setting up your cloud computer now. We'll email you when it's ready.\n\n\
         Manage your plan, card and invoices any time in Settings → Billing → Manage billing.\n\n\
         — Allternit\n",
        greeting(recipient),
    );
    let html = html::layout(
        &format!("Your {plan_label} plan is active. Your cloud computer is being set up."),
        &[
            html::heading(&format!("Your {plan_label} plan is active")),
            html::paragraph(&greeting(recipient)),
            html::paragraph("Thanks for subscribing. Here's what your plan includes."),
            html::facts_card(
                &format!("Allternit Cloud {plan_label}"),
                &[
                    ("Credits", format!("${credits} each month")),
                    ("Cloud computer", format!("{cpu_cores} vCPU · {memory_gb} GB")),
                    ("Status", "Setting up".to_string()),
                ],
            ),
            html::paragraph("We're setting up your cloud computer now. We'll email you when it's ready."),
            html::paragraph("Manage your plan, card and invoices any time in Settings → Billing → Manage billing."),
            html::button("Manage billing", "https://ai.allternit.com/settings/billing"),
            html::feature_card(chrono::Utc::now()),
        ],
    );
    Email { subject: format!("Your Allternit Cloud {plan_label} plan is active"), body, html }
}

pub fn computer_ready_email(recipient: &Recipient, cpu_cores: i64, memory_mb: i64) -> Email {
    let body = format!(
        "{}\n\n\
         Your Allternit cloud computer is on and signed in to your account. It shows under Computers in the app.\n\n\
         It stays on, so your agents and scheduled tasks keep running when your own computer is off.\n\n\
         — Allternit\n",
        greeting(recipient)
    );
    let html = html::layout(
        "Your Allternit cloud computer is on and signed in.",
        &[
            html::heading("Your cloud computer is ready"),
            html::paragraph(&greeting(recipient)),
            html::paragraph("Your Allternit cloud computer is on and signed in to your account. It shows under Computers in the app."),
            html::facts_card(
                "Your cloud computer",
                &[
                    ("Size", format!("{cpu_cores} vCPU · {} GB", memory_mb / 1024)),
                    ("Status", "On".to_string()),
                ],
            ),
            html::paragraph("It stays on, so your agents and scheduled tasks keep running when your own computer is off."),
            html::button("Open Allternit", "https://ai.allternit.com"),
            html::feature_card(chrono::Utc::now()),
        ],
    );
    Email { subject: "Your cloud computer is ready".to_string(), body, html }
}

fn format_usd(value: f64) -> String {
    if (value - value.round()).abs() < 0.005 {
        format!("{}", value.round() as i64)
    } else {
        format!("{value:.2}")
    }
}

/// Header-safe: one line, no CR/LF.
fn one_line(value: &str) -> String {
    value.replace(['\r', '\n'], " ").trim().to_string()
}

/// The RFC 5322 message handed to sendmail: multipart/alternative, plain
/// text first, branded HTML second.
pub fn render_message(recipient: &Recipient, email: &Email, now: chrono::DateTime<chrono::Utc>) -> String {
    let from_address = std::env::var(ENV_FROM).unwrap_or_else(|_| DEFAULT_FROM_ADDRESS.to_string());
    let reply_to = std::env::var(ENV_REPLY_TO).unwrap_or_else(|_| DEFAULT_REPLY_TO.to_string());
    let domain = from_address.rsplit('@').next().unwrap_or("news.allternit.com").to_string();
    let boundary = format!("allternit-{}", uuid::Uuid::new_v4().simple());
    [
        format!("From: {DEFAULT_FROM_NAME} <{}>", one_line(&from_address)),
        format!("Reply-To: {}", one_line(&reply_to)),
        format!("To: {}", one_line(&recipient.email)),
        format!("Subject: {}", encode_subject(&one_line(&email.subject))),
        format!("Date: {}", now.to_rfc2822()),
        format!("Message-ID: <{}@{}>", uuid::Uuid::new_v4().simple(), one_line(&domain)),
        "MIME-Version: 1.0".to_string(),
        format!("Content-Type: multipart/alternative; boundary=\"{boundary}\""),
        String::new(),
        format!("--{boundary}"),
        "Content-Type: text/plain; charset=utf-8".to_string(),
        "Content-Transfer-Encoding: base64".to_string(),
        String::new(),
        base64_lines(&email.body.replace('\n', "\r\n")),
        format!("--{boundary}"),
        "Content-Type: text/html; charset=utf-8".to_string(),
        "Content-Transfer-Encoding: base64".to_string(),
        String::new(),
        base64_lines(&email.html),
        format!("--{boundary}--"),
        String::new(),
    ]
    .join("\r\n")
}

/// RFC 2047 for non-ASCII subjects (e.g. "→").
fn encode_subject(subject: &str) -> String {
    if subject.is_ascii() {
        subject.to_string()
    } else {
        use base64::Engine as _;
        format!("=?UTF-8?B?{}?=", base64::engine::general_purpose::STANDARD.encode(subject))
    }
}

/// Base64 body wrapped at 76 characters (RFC 2045).
fn base64_lines(text: &str) -> String {
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    encoded
        .as_bytes()
        .chunks(76)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join("\r\n")
}

async fn sendmail(recipient: &Recipient, email: &Email) -> Result<(), String> {
    if !recipient.email.contains('@') || recipient.email.contains(['\r', '\n', ' ', '<', '>']) {
        return Err(format!("not a sendable address: {:?}", recipient.email));
    }
    let program = std::env::var(ENV_SENDMAIL).unwrap_or_else(|_| DEFAULT_SENDMAIL.to_string());
    let from_address = std::env::var(ENV_FROM).unwrap_or_else(|_| DEFAULT_FROM_ADDRESS.to_string());
    let message = render_message(recipient, email, chrono::Utc::now());
    // -i: a lone "." line is not end of input; envelope recipient given
    // explicitly instead of trusting headers (-t).
    let mut child = tokio::process::Command::new(&program)
        .args(["-i", "-f", one_line(&from_address).as_str(), "--", recipient.email.as_str()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{program}: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(message.as_bytes())
            .await
            .map_err(|error| format!("{program} stdin: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .await
        .map_err(|error| format!("{program}: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{program} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Claim the (user, kind, ref) slot. `false` = already handled.
async fn claim(db: &PgPool, user_id: &str, kind: &str, reference: &str, email: &str) -> Result<bool, sqlx::Error> {
    let claimed = sqlx::query(
        r#"
        INSERT INTO customer_emails (user_id, kind, ref, email, status)
        VALUES ($1, $2, $3, $4, 'sent')
        ON CONFLICT (user_id, kind, ref) DO NOTHING
        "#,
    )
    .bind(user_id)
    .bind(kind)
    .bind(reference)
    .bind(email)
    .execute(db)
    .await?;
    Ok(claimed.rows_affected() == 1)
}

/// Claim, then send. Returns whether this call sent (or tried to send) it.
pub async fn deliver(
    db: &PgPool,
    user_id: &str,
    kind: &str,
    reference: &str,
    recipient: &Recipient,
    email: &Email,
) -> bool {
    match claim(db, user_id, kind, reference, &recipient.email).await {
        Ok(true) => {}
        Ok(false) => return false,
        Err(error) => {
            tracing::error!(%user_id, kind, %error, "customer email: could not record; not sending");
            return false;
        }
    }
    if let Err(error) = sendmail(recipient, email).await {
        tracing::error!(%user_id, kind, %error, "customer email failed");
        let _ = sqlx::query(
            "UPDATE customer_emails SET status = 'failed', error = $4 WHERE user_id = $1 AND kind = $2 AND ref = $3",
        )
        .bind(user_id)
        .bind(kind)
        .bind(reference)
        .bind(&error)
        .execute(db)
        .await;
    } else {
        tracing::info!(%user_id, kind, "customer email sent");
    }
    true
}

/// The user's primary email and first name from Clerk.
pub async fn fetch_recipient(user_id: &str) -> Option<Recipient> {
    let config = super::user_trust::TrustConfig::from_env();
    let secret = config.clerk_secret_key.as_deref()?;
    let url = format!("{}/v1/users/{}", config.clerk_api_base.trim_end_matches('/'), user_id);
    let user: serde_json::Value = reqwest::Client::new()
        .get(url)
        .bearer_auth(secret)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    recipient_from_clerk_user(&user)
}

/// Primary email + first name out of a Clerk user object (API or webhook `data`).
pub fn recipient_from_clerk_user(user: &serde_json::Value) -> Option<Recipient> {
    let addresses = user["email_addresses"].as_array()?;
    let primary_id = user["primary_email_address_id"].as_str();
    let email = addresses
        .iter()
        .find(|address| primary_id.is_some() && address["id"].as_str() == primary_id)
        .or_else(|| addresses.first())?["email_address"]
        .as_str()?
        .trim()
        .to_string();
    Some(Recipient {
        email,
        first_name: user["first_name"].as_str().map(str::to_string),
    })
}

/// Clerk `user.created`: note to the team, welcome email to the person.
pub async fn on_user_created(db: &PgPool, user: &serde_json::Value) {
    let Some(user_id) = user["id"].as_str() else { return };
    let Some(recipient) = recipient_from_clerk_user(user) else {
        tracing::warn!(%user_id, "user.created without an email address; no welcome email");
        return;
    };
    super::ops_alert::send_once(
        &format!("signup:{user_id}"),
        format!("New sign-up: {}", recipient.email),
        format!(
            "{} signed up for Allternit.\n\nUser: {user_id}\nName: {}\n\nAll customers: https://platform.allternit.com/admin/customers",
            recipient.email,
            recipient.first_name.as_deref().unwrap_or("(none)"),
        ),
    );
    if enabled() {
        deliver(db, user_id, "welcome", "", &recipient, &welcome_email(&recipient)).await;
    }
}

/// A subscription's first active grant: note to the team, plan email to the
/// customer. Idempotent per subscription.
pub fn spawn_plan_started(
    db: PgPool,
    user_id: String,
    subscription_id: String,
    plan: &'static crate::routes::billing_subscriptions::SubscriptionPlan,
) {
    tokio::spawn(async move {
        // Already handled (redelivery, renewal, or existed before go-live)?
        let handled: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM customer_emails WHERE user_id = $1 AND kind = 'plan_started' AND ref = $2",
        )
        .bind(&user_id)
        .bind(&subscription_id)
        .fetch_optional(&db)
        .await
        .ok()
        .flatten();
        if handled.is_some() {
            return;
        }
        let recipient = fetch_recipient(&user_id).await;
        let email = recipient.as_ref().map(|r| r.email.clone()).unwrap_or_else(|| "(unknown email)".to_string());
        super::ops_alert::send_once(
            &format!("paid:{subscription_id}"),
            format!("New paying customer: {email} ({} ${}/mo)", plan.label, format_usd(plan.price_usd)),
            format!(
                "{email} subscribed to Allternit Cloud {}.\n\nUser: {user_id}\nSubscription: https://dashboard.stripe.com/subscriptions/{subscription_id}\n\nAll customers: https://platform.allternit.com/admin/customers",
                plan.label
            ),
        );
        let Some(recipient) = recipient else {
            tracing::warn!(%user_id, "plan started: no email from Clerk; no plan email");
            return;
        };
        if !enabled() {
            return;
        }
        let size: Option<(Option<i32>, Option<i64>)> = sqlx::query_as(
            "SELECT computer_base_vcpu, computer_base_memory_mb FROM plan_tiers WHERE id = $1",
        )
        .bind(plan.id)
        .fetch_optional(&db)
        .await
        .ok()
        .flatten();
        let Some((Some(cpu_cores), Some(memory_mb))) = size else {
            tracing::warn!(plan = plan.id, "plan started: plan has no computer size; no plan email");
            return;
        };
        let message = plan_started_email(&recipient, plan.label, plan.monthly_credits_usd, i64::from(cpu_cores), memory_mb);
        deliver(&db, &user_id, "plan_started", &subscription_id, &recipient, &message).await;
    });
}

/// A paid cloud computer paired: "ready" email, once per computer.
pub fn spawn_computer_ready(db: PgPool, user_id: String, instance_id: String) {
    if !enabled() {
        return;
    }
    tokio::spawn(async move {
        let row: Option<(String, i32, i64)> =
            sqlx::query_as("SELECT tier, cpu_cores, memory_mb FROM provisioned_instances WHERE id = $1")
                .bind(&instance_id)
                .fetch_optional(&db)
                .await
                .ok()
                .flatten();
        let Some((tier, cpu_cores, memory_mb)) = row else { return };
        if tier != "paid" {
            return;
        }
        let Some(recipient) = fetch_recipient(&user_id).await else {
            tracing::warn!(%user_id, "computer ready: no email from Clerk; no email");
            return;
        };
        deliver(&db, &user_id, "computer_ready", &instance_id, &recipient, &computer_ready_email(&recipient, i64::from(cpu_cores), memory_mb)).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eoj() -> Recipient {
        Recipient { email: "a@example.com".into(), first_name: Some("Sam".into()) }
    }

    #[test]
    fn emails_use_the_approved_wording() {
        let welcome = welcome_email(&eoj());
        assert_eq!(welcome.subject, "Welcome to Allternit");
        assert!(welcome.body.starts_with("Hi Sam,\n\nYour Allternit account is set up."));
        let plan = plan_started_email(&eoj(), "Plus", 22.0, 2, 4096);
        assert_eq!(plan.subject, "Your Allternit Cloud Plus plan is active");
        assert!(plan.body.contains("includes $22 in Allternit Cloud credits each month and a cloud computer (2 vCPU, 4 GB)."));
        let ready = computer_ready_email(&Recipient { email: "a@example.com".into(), first_name: None }, 2, 4096);
        assert_eq!(ready.subject, "Your cloud computer is ready");
        assert!(ready.body.starts_with("Hi,\n\n"));
    }

    #[test]
    fn html_is_branded_and_escaped() {
        let sneaky = Recipient { email: "a@example.com".into(), first_name: Some("<b>Sam</b>".into()) };
        let email = welcome_email(&sneaky);
        assert!(email.html.contains("https://allternit.com/email/wordmark.png"));
        assert!(email.html.contains("https://allternit.com/email/phone.jpg"));
        assert!(email.html.contains("https://allternit.com/email/feature-"));
        // The phone card is in the welcome email only.
        assert!(!plan_started_email(&sneaky, "Plus", 22.0, 2, 4096).html.contains("/email/phone.jpg"));
        assert!(!computer_ready_email(&sneaky, 2, 4096).html.contains("/email/phone.jpg"));
        assert!(email.html.contains("Hi &lt;b&gt;Sam&lt;/b&gt;,"));
        assert!(!email.html.contains("<b>Sam</b>"));
        let message = render_message(&sneaky, &email, chrono::Utc::now());
        assert!(message.contains("Content-Type: multipart/alternative;"));
        assert!(message.contains("Content-Type: text/html; charset=utf-8"));
    }

    /// `EMAIL_PREVIEW_DIR=/tmp/x cargo test -p allternit-cloud-api write_email_previews -- --ignored`
    #[test]
    #[ignore]
    fn write_email_previews() {
        let dir = std::env::var("EMAIL_PREVIEW_DIR").expect("EMAIL_PREVIEW_DIR");
        let sam = eoj();
        for (name, email) in [
            ("welcome", welcome_email(&sam)),
            ("plan_started", plan_started_email(&sam, "Plus", 22.0, 2, 4096)),
            ("computer_ready", computer_ready_email(&sam, 2, 4096)),
        ] {
            std::fs::write(format!("{dir}/{name}.html"), &email.html).unwrap();
            std::fs::write(format!("{dir}/{name}.eml"), render_message(&Recipient { email: "allternitpbc@gmail.com".into(), first_name: Some("Eoj".into()) }, &email, chrono::Utc::now())).unwrap();
        }
    }

    #[test]
    fn the_featured_film_rotates_monthly() {
        use chrono::TimeZone;
        let at = |y, m| chrono::Utc.with_ymd_and_hms(y, m, 15, 12, 0, 0).unwrap();
        assert_eq!(feature_for(at(2026, 10)).id, "montage");
        assert_eq!(feature_for(at(2026, 11)).id, "artifacts");
        assert_eq!(feature_for(at(2027, 3)).id, "agency-kernel");
        assert_eq!(feature_for(at(2027, 4)).id, "montage");
        assert_eq!(feature_for(at(2026, 9)).id, "agency-kernel");
    }

    #[test]
    fn headers_cannot_be_injected() {
        let sneaky = Recipient { email: "a@example.com\r\nBcc: x@evil.test".into(), first_name: None };
        let message = render_message(&sneaky, &welcome_email(&sneaky), chrono::Utc::now());
        assert!(!message.contains("\r\nBcc:"));
    }

    #[test]
    fn primary_email_is_picked_from_a_clerk_user() {
        let user = serde_json::json!({
            "id": "user_1",
            "first_name": "Sam",
            "primary_email_address_id": "idn_2",
            "email_addresses": [
                { "id": "idn_1", "email_address": "old@example.com" },
                { "id": "idn_2", "email_address": "main@example.com" }
            ]
        });
        assert_eq!(
            recipient_from_clerk_user(&user),
            Some(Recipient { email: "main@example.com".into(), first_name: Some("Sam".into()) })
        );
    }
}
