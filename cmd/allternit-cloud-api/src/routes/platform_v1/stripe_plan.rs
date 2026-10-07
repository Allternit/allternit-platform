//! Platform API prices in Stripe Billing (spec §7): the plan and the meter-event
//! reporter. **Both are off by default; nothing here writes to Stripe unless a
//! flag is explicitly turned on.**
//!
//! * [`plan`] maps every usage meter (`usage_events::METERS`, documented in
//!   `api/platform/usage.mdx`) to a Stripe Billing Meter plus a Pay-as-you-go and a
//!   Growth price, and adds the Growth base fee. `cargo run -p allternit-cloud-api
//!   --features stripe-plan-cli --bin platform-stripe-plan` prints the exact requests (a dry run). It creates
//!   them only with `--apply` **and** `ALLTERNIT_PLATFORM_STRIPE_APPLY=1` **and**
//!   `STRIPE_SECRET_KEY` set; each request carries an idempotency key.
//! * [`report_pending`] sends usage rows of live Pay-as-you-go / Growth projects
//!   with a Stripe customer to Stripe as meter events (`identifier` = the usage
//!   row id, so Stripe drops repeats). The worker runs it only when
//!   `ALLTERNIT_PLATFORM_STRIPE_METERING=1` and `STRIPE_SECRET_KEY` are set.
//!
//! Units in Stripe are whole numbers: voice is metered in **seconds**, text
//! tokens as the **billed amount in micro-dollars** (list price + 15% already
//! applied per row, 0 on the project's own key; one price of $0.000001 per unit),
//! everything else in its own unit.
//!
//! Growth (spec §7: $249/mo, $300 usage credit, 10% off usage) uses its own
//! metered prices at 90% of Pay-as-you-go, except carrier registration (passed
//! through at cost). The $300 monthly credit is a per-customer Stripe credit grant
//! made each period, so it is not part of this one-time plan.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;

use crate::routes::billing_checkout::StripeCheckout;

pub const ENV_APPLY: &str = "ALLTERNIT_PLATFORM_STRIPE_APPLY";
pub const ENV_METERING: &str = "ALLTERNIT_PLATFORM_STRIPE_METERING";
/// Version tag in idempotency keys and metadata; bump to create a new set.
pub const PLAN_VERSION: &str = "platform-v1-2026-10";

/// How a usage row becomes a Stripe meter value.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeterValue {
    /// The row's quantity (a whole count).
    Quantity,
    /// Minutes × 60, rounded.
    Seconds,
    /// The quantity rounded up.
    CeilQuantity,
    /// The row's billed amount in micro-dollars.
    AmountMicrousd,
}

/// One Stripe meter and its prices.
#[derive(Debug, Clone, Serialize)]
pub struct MeterPlan {
    /// Our meters that feed it.
    pub meters: &'static [&'static str],
    pub event_name: &'static str,
    pub display_name: &'static str,
    pub unit: &'static str,
    pub value: MeterValue,
    /// Pay as you go, cents per unit (decimal string, Stripe `unit_amount_decimal`).
    pub payg_cents: &'static str,
    /// Growth, cents per unit.
    pub growth_cents: &'static str,
    /// Units free each month before the price applies (graduated tiers).
    pub free_units: u32,
}

pub const METER_PLANS: &[MeterPlan] = &[
    MeterPlan { meters: &["voice_min_allternit"], event_name: "allternit_voice_seconds_allternit", display_name: "Voice agent seconds (Allternit model)", unit: "second", value: MeterValue::Seconds, payg_cents: "0.15", growth_cents: "0.135", free_units: 0 },
    MeterPlan { meters: &["voice_min_byok"], event_name: "allternit_voice_seconds_byok", display_name: "Voice agent seconds (own model key)", unit: "second", value: MeterValue::Seconds, payg_cents: "0.1", growth_cents: "0.09", free_units: 0 },
    MeterPlan { meters: &["agent_month"], event_name: "allternit_hosted_agent_months", display_name: "Hosted agents", unit: "agent-month", value: MeterValue::Quantity, payg_cents: "400", growth_cents: "360", free_units: 3 },
    MeterPlan { meters: &["tokens_in", "tokens_out"], event_name: "allternit_text_tokens_microusd", display_name: "Text conversation tokens (list price + 15%)", unit: "micro-dollar", value: MeterValue::AmountMicrousd, payg_cents: "0.0001", growth_cents: "0.00009", free_units: 0 },
    MeterPlan { meters: &["number_local_month"], event_name: "allternit_numbers_local", display_name: "Local numbers", unit: "number-month", value: MeterValue::Quantity, payg_cents: "200", growth_cents: "180", free_units: 0 },
    MeterPlan { meters: &["number_tollfree_month"], event_name: "allternit_numbers_tollfree", display_name: "Toll-free numbers", unit: "number-month", value: MeterValue::Quantity, payg_cents: "300", growth_cents: "270", free_units: 0 },
    MeterPlan { meters: &["sms_segment"], event_name: "allternit_sms_segments", display_name: "SMS segments", unit: "segment", value: MeterValue::Quantity, payg_cents: "1.2", growth_cents: "1.08", free_units: 0 },
    MeterPlan { meters: &["mms"], event_name: "allternit_mms", display_name: "MMS messages", unit: "message", value: MeterValue::Quantity, payg_cents: "3", growth_cents: "2.7", free_units: 0 },
    MeterPlan { meters: &["registration_passthrough_cents"], event_name: "allternit_registration_passthrough_cents", display_name: "Carrier registration (at cost)", unit: "cent", value: MeterValue::Quantity, payg_cents: "1", growth_cents: "1", free_units: 0 },
    MeterPlan { meters: &["recording_min_month"], event_name: "allternit_recording_minute_months", display_name: "Recording storage after 90 days", unit: "minute-month", value: MeterValue::CeilQuantity, payg_cents: "0.2", growth_cents: "0.18", free_units: 0 },
];

/// Growth base fee, cents per month.
pub const GROWTH_BASE_CENTS: i64 = 24_900;

/// The Stripe meter a usage meter reports to.
pub fn meter_plan(meter: &str) -> Option<&'static MeterPlan> {
    METER_PLANS.iter().find(|p| p.meters.contains(&meter))
}

/// One Stripe request of the plan. `{meter:<event_name>}` and `{product:<key>}`
/// in form values are the ids of objects created earlier in the plan.
#[derive(Debug, Clone, Serialize)]
pub struct PlanStep {
    pub key: String,
    pub method: &'static str,
    pub path: &'static str,
    pub form: Vec<(String, String)>,
}

fn f(k: &str, v: impl Into<String>) -> (String, String) {
    (k.to_string(), v.into())
}

fn price_step(key: String, product: &str, m: &MeterPlan, plan: &str, cents: &str) -> PlanStep {
    let mut form = vec![
        f("currency", "usd"),
        f("product", format!("{{product:{product}}}")),
        f("nickname", format!("{} ({plan})", m.display_name)),
        f("lookup_key", format!("platform_{plan}_{}", m.event_name.trim_start_matches("allternit_"))),
        f("recurring[interval]", "month"),
        f("recurring[usage_type]", "metered"),
        f("recurring[meter]", format!("{{meter:{}}}", m.event_name)),
        f("metadata[allternit_plan]", plan),
        f("metadata[allternit_version]", PLAN_VERSION),
    ];
    if m.free_units > 0 {
        form.extend([
            f("billing_scheme", "tiered"),
            f("tiers_mode", "graduated"),
            f("tiers[0][up_to]", m.free_units.to_string()),
            f("tiers[0][unit_amount]", "0"),
            f("tiers[1][up_to]", "inf"),
            f("tiers[1][unit_amount_decimal]", cents),
        ]);
    } else {
        form.extend([f("billing_scheme", "per_unit"), f("unit_amount_decimal", cents)]);
    }
    PlanStep { key, method: "POST", path: "/v1/prices", form }
}

/// Every Stripe object the Platform API prices need, in creation order.
pub fn plan() -> Vec<PlanStep> {
    let mut steps = Vec::new();
    for m in METER_PLANS {
        steps.push(PlanStep {
            key: format!("meter:{}", m.event_name),
            method: "POST",
            path: "/v1/billing/meters",
            form: vec![
                f("display_name", m.display_name),
                f("event_name", m.event_name),
                f("default_aggregation[formula]", "sum"),
                f("customer_mapping[type]", "by_id"),
                f("customer_mapping[event_payload_key]", "stripe_customer_id"),
                f("value_settings[event_payload_key]", "value"),
            ],
        });
    }
    for m in METER_PLANS {
        let product = m.event_name.trim_start_matches("allternit_");
        steps.push(PlanStep {
            key: format!("product:{product}"),
            method: "POST",
            path: "/v1/products",
            form: vec![
                f("name", format!("Allternit Platform API: {}", m.display_name)),
                f("unit_label", m.unit),
                f("metadata[allternit_meters]", m.meters.join(",")),
                f("metadata[allternit_version]", PLAN_VERSION),
            ],
        });
        steps.push(price_step(format!("price:payg:{product}"), product, m, "payg", m.payg_cents));
        steps.push(price_step(format!("price:growth:{product}"), product, m, "growth", m.growth_cents));
    }
    steps.push(PlanStep {
        key: "product:growth_base".into(),
        method: "POST",
        path: "/v1/products",
        form: vec![f("name", "Allternit Platform API: Growth plan"), f("metadata[allternit_version]", PLAN_VERSION)],
    });
    steps.push(PlanStep {
        key: "price:growth:base".into(),
        method: "POST",
        path: "/v1/prices",
        form: vec![
            f("currency", "usd"),
            f("product", "{product:growth_base}"),
            f("nickname", "Growth plan, monthly"),
            f("lookup_key", "platform_growth_base"),
            f("unit_amount", GROWTH_BASE_CENTS.to_string()),
            f("recurring[interval]", "month"),
            f("recurring[usage_type]", "licensed"),
            f("metadata[allternit_plan]", "growth"),
            f("metadata[allternit_version]", PLAN_VERSION),
        ],
    });
    steps
}

/// The plan as text: one block per request, exactly what `--apply` would send.
pub fn render(steps: &[PlanStep]) -> String {
    let mut out = format!(
        "Allternit Platform API: Stripe plan {PLAN_VERSION} (DRY RUN: nothing was sent to Stripe)\n\
         {} requests: {} meters, {} products, {} prices.\n\
         {{meter:X}} / {{product:X}} = the id of the object created by an earlier step.\n\n",
        steps.len(),
        steps.iter().filter(|s| s.path == "/v1/billing/meters").count(),
        steps.iter().filter(|s| s.path == "/v1/products").count(),
        steps.iter().filter(|s| s.path == "/v1/prices").count(),
    );
    for (i, s) in steps.iter().enumerate() {
        out.push_str(&format!("{:>2}. {} {}   [{}]\n", i + 1, s.method, s.path, s.key));
        for (k, v) in &s.form {
            out.push_str(&format!("      {k} = {v}\n"));
        }
    }
    out.push_str(
        "\nNot in this plan (made per customer, later): the Growth $300 monthly credit grant \
         (POST /v1/billing/credit_grants each period) and each project's subscription.\n",
    );
    out
}

fn resolve(value: &str, ids: &HashMap<String, String>) -> Result<String, String> {
    match value.strip_prefix('{').and_then(|v| v.strip_suffix('}')) {
        Some(reference) if reference.starts_with("meter:") || reference.starts_with("product:") => {
            ids.get(reference).cloned().ok_or_else(|| format!("{reference} was not created first"))
        }
        _ => Ok(value.to_string()),
    }
}

/// Is applying allowed by the environment? (`ALLTERNIT_PLATFORM_STRIPE_APPLY=1`)
pub fn apply_allowed() -> bool {
    std::env::var(ENV_APPLY).is_ok_and(|v| v == "1")
}

/// Create the plan's objects in Stripe. Only call after `--apply` with
/// [`apply_allowed`]; every request has an idempotency key.
pub async fn apply(stripe: &dyn StripeCheckout, secret: &str, steps: &[PlanStep]) -> Result<HashMap<String, String>, String> {
    let mut ids = HashMap::new();
    for s in steps {
        let form = s.form.iter().map(|(k, v)| resolve(v, &ids).map(|v| (k.clone(), v))).collect::<Result<Vec<_>, _>>()?;
        let idem = format!("allternit-{PLAN_VERSION}-{}", s.key.replace(':', "-"));
        let created = stripe.post_object(secret, s.path, Some(&idem), &form).await.map_err(|e| format!("{}: {e}", s.key))?;
        let id = created["id"].as_str().ok_or_else(|| format!("{}: Stripe returned no id", s.key))?;
        ids.insert(s.key.clone(), id.to_string());
    }
    Ok(ids)
}

/// `platform-stripe-plan [--json] [--apply]`. Prints the plan; creates it only
/// with `--apply` and `ALLTERNIT_PLATFORM_STRIPE_APPLY=1` and `STRIPE_SECRET_KEY`.
pub async fn cli_main(args: &[String]) -> i32 {
    let steps = plan();
    if args.iter().any(|a| a == "--json") {
        println!("{}", serde_json::to_string_pretty(&json!({ "version": PLAN_VERSION, "dry_run": true, "steps": steps })).unwrap_or_default());
    } else {
        print!("{}", render(&steps));
    }
    if !args.iter().any(|a| a == "--apply") {
        return 0;
    }
    if !apply_allowed() {
        eprintln!("--apply ignored: set {ENV_APPLY}=1 as well (and STRIPE_SECRET_KEY) to create these in Stripe.");
        return 2;
    }
    let Ok(secret) = std::env::var("STRIPE_SECRET_KEY") else {
        eprintln!("--apply needs STRIPE_SECRET_KEY.");
        return 2;
    };
    let stripe = crate::routes::billing_checkout::ReqwestStripeCheckout::new();
    match apply(&stripe, &secret, &steps).await {
        Ok(ids) => {
            println!("Created {} objects:", ids.len());
            let mut ids: Vec<_> = ids.into_iter().collect();
            ids.sort();
            for (k, id) in ids {
                println!("  {k} = {id}");
            }
            0
        }
        Err(e) => {
            eprintln!("Stopped: {e}");
            1
        }
    }
}

// ─── Meter-event reporter ───────────────────────────────────────────────────

/// Is the reporter switched on? (`ALLTERNIT_PLATFORM_STRIPE_METERING=1` and a Stripe key.)
pub fn metering_enabled() -> bool {
    std::env::var(ENV_METERING).is_ok_and(|v| v == "1") && std::env::var("STRIPE_SECRET_KEY").is_ok_and(|v| !v.is_empty())
}

/// A usage row the reporter may send.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PendingRow {
    pub id: String,
    pub meter: String,
    pub quantity: f64,
    pub amount_microusd: Option<i64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub stripe_customer_id: String,
}

/// The Stripe meter value of a row (`None` = nothing to bill, e.g. own-key tokens).
pub fn meter_value(meter: &str, quantity: f64, amount_microusd: Option<i64>) -> Option<(&'static str, i64)> {
    let m = meter_plan(meter)?;
    let value = match m.value {
        MeterValue::Quantity => quantity.round() as i64,
        MeterValue::Seconds => (quantity * 60.0).round() as i64,
        MeterValue::CeilQuantity => quantity.ceil() as i64,
        MeterValue::AmountMicrousd => amount_microusd.unwrap_or(0),
    };
    (value > 0).then_some((m.event_name, value))
}

/// Form fields of `POST /v1/billing/meter_events` for a row.
pub fn meter_event_form(row: &PendingRow) -> Option<Vec<(String, String)>> {
    let (event, value) = meter_value(&row.meter, row.quantity, row.amount_microusd)?;
    Some(vec![
        f("event_name", event),
        f("identifier", row.id.clone()),
        f("timestamp", row.created_at.timestamp().to_string()),
        f("payload[stripe_customer_id]", row.stripe_customer_id.clone()),
        f("payload[value]", value.to_string()),
    ])
}

/// Rows of live Pay-as-you-go / Growth projects with a Stripe customer, not sent yet
/// (Stripe takes meter events up to 35 days old; older ones are left alone).
pub async fn pending_rows(db: &PgPool, limit: i64) -> Result<Vec<PendingRow>, sqlx::Error> {
    sqlx::query_as::<_, PendingRow>(
        "SELECT e.id, e.meter, e.quantity::float8 AS quantity, e.amount_microusd, e.created_at, p.stripe_customer_id \
         FROM platform_usage_events e JOIN platform_projects p ON p.id = e.project_id \
         WHERE e.stripe_reported_at IS NULL AND p.env = 'live' AND p.plan IN ('payg', 'growth') \
           AND p.stripe_customer_id IS NOT NULL AND e.created_at > NOW() - INTERVAL '30 days' \
         ORDER BY e.created_at LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Send pending rows to Stripe. Callers check [`metering_enabled`] first.
/// Returns how many rows were marked sent (rows worth 0 are marked without a request).
pub async fn report_pending(db: &PgPool, stripe: &dyn StripeCheckout, secret: &str) -> Result<usize, sqlx::Error> {
    let mut sent = 0;
    for row in pending_rows(db, 200).await? {
        if let Some(form) = meter_event_form(&row) {
            if let Err(e) = stripe.post_object(secret, "/v1/billing/meter_events", Some(&format!("allternit-usage-{}", row.id)), &form).await {
                tracing::warn!(row = %row.id, "platform stripe meter event not sent: {e}");
                continue;
            }
        }
        sqlx::query("UPDATE platform_usage_events SET stripe_reported_at = NOW() WHERE id = $1").bind(&row.id).execute(db).await?;
        sent += 1;
    }
    Ok(sent)
}

/// One reporter pass for the worker: a no-op unless [`metering_enabled`].
pub async fn report_if_enabled(db: &PgPool) {
    if !metering_enabled() {
        return;
    }
    let Ok(secret) = std::env::var("STRIPE_SECRET_KEY") else { return };
    let stripe = crate::routes::billing_checkout::ReqwestStripeCheckout::new();
    match report_pending(db, &stripe, &secret).await {
        Ok(n) if n > 0 => tracing::info!(rows = n, "platform usage sent to Stripe"),
        Ok(_) => {}
        Err(e) => tracing::warn!("platform stripe metering: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::platform_v1::usage_events::METERS;

    #[test]
    fn every_usage_meter_has_a_stripe_meter_and_prices_match_spec() {
        for meter in METERS {
            assert!(meter_plan(meter).is_some(), "{meter} has no Stripe meter");
        }
        // Stripe prices agree with the spend-cap prices (per unit, in micro-dollars).
        for m in METER_PLANS {
            for meter in m.meters {
                let Some(ours) = super::super::billing::unit_price_microusd(meter) else { continue };
                let cents: f64 = m.payg_cents.parse().unwrap();
                let per_unit = match m.value {
                    MeterValue::Seconds => cents * 60.0 * 10_000.0,
                    _ => cents * 10_000.0,
                };
                assert!((per_unit - ours as f64).abs() < 0.5, "{meter}: stripe {per_unit} vs ours {ours}");
                let growth: f64 = m.growth_cents.parse().unwrap();
                let expected = if *meter == "registration_passthrough_cents" { cents } else { cents * 0.9 };
                assert!((growth - expected).abs() < 1e-9, "{meter}: growth {growth} vs {expected}");
            }
        }
    }

    #[test]
    fn plan_creates_meters_before_the_prices_that_use_them() {
        let steps = plan();
        let mut made = std::collections::HashSet::new();
        for s in &steps {
            for (_, v) in &s.form {
                if let Some(r) = v.strip_prefix('{').and_then(|v| v.strip_suffix('}')) {
                    assert!(made.contains(r), "{} uses {r} before it exists", s.key);
                }
            }
            made.insert(s.key.clone());
        }
        assert_eq!(steps.iter().filter(|s| s.path == "/v1/billing/meters").count(), METER_PLANS.len());
        let text = render(&steps);
        assert!(text.contains("DRY RUN") && text.contains("unit_amount = 24900"));
        let agents = steps.iter().find(|s| s.key == "price:payg:hosted_agent_months").unwrap();
        assert!(agents.form.contains(&("tiers[0][up_to]".into(), "3".into())) && agents.form.contains(&("tiers[1][unit_amount_decimal]".into(), "400".into())));
    }

    #[test]
    fn meter_values_are_whole_numbers_and_own_key_tokens_send_nothing() {
        assert_eq!(meter_value("voice_min_allternit", 2.5, None), Some(("allternit_voice_seconds_allternit", 150)));
        assert_eq!(meter_value("tokens_out", 2000.0, Some(115)), Some(("allternit_text_tokens_microusd", 115)));
        assert_eq!(meter_value("tokens_in", 2000.0, Some(0)), None);
        assert_eq!(meter_value("tokens_in", 2000.0, None), None);
        assert_eq!(meter_value("recording_min_month", 0.2, None), Some(("allternit_recording_minute_months", 1)));
        let row = PendingRow { id: "use_1".into(), meter: "sms_segment".into(), quantity: 3.0, amount_microusd: None, created_at: chrono::Utc::now(), stripe_customer_id: "cus_1".into() };
        let form = meter_event_form(&row).unwrap();
        assert!(form.contains(&("identifier".into(), "use_1".into())) && form.contains(&("payload[value]".into(), "3".into())));
    }

    #[test]
    fn metering_and_apply_are_off_by_default() {
        // The test environment never sets these.
        assert!(!apply_allowed());
        assert!(std::env::var(ENV_METERING).is_err());
    }
}
