//! Wallet math for Allternit Open (CONTRACT-open-tier-2026-11-26.md D1-D5):
//! bought credits survive the rollover cap, the open allowance resets once
//! per plan grant, and open spend draws plan allowance first, then bought.

use super::*;
use sqlx::PgPool;

/// The wallet tables with production's column types (migrations 001/002/169).
async fn wallet_db() -> PgPool {
    let db = crate::routes::test_support::test_pool().await;
    sqlx::raw_sql(
        "CREATE TABLE user_credits (user_id TEXT PRIMARY KEY, balance_usd DOUBLE PRECISION DEFAULT 0,
             bought_usd DOUBLE PRECISION NOT NULL DEFAULT 0, created_at TIMESTAMPTZ DEFAULT NOW(), updated_at TIMESTAMPTZ DEFAULT NOW());
         CREATE TABLE credit_transactions (id TEXT PRIMARY KEY, user_id TEXT, amount_usd DOUBLE PRECISION,
             transaction_id TEXT UNIQUE, source TEXT, created_at TIMESTAMPTZ DEFAULT NOW());
         CREATE TABLE user_open_allowance (user_id TEXT PRIMARY KEY, allowance_usd DOUBLE PRECISION NOT NULL DEFAULT 0,
             used_usd DOUBLE PRECISION NOT NULL DEFAULT 0, period_start TIMESTAMPTZ, grant_key TEXT,
             updated_at TIMESTAMPTZ NOT NULL DEFAULT now());",
    )
    .execute(&db)
    .await
    .unwrap();
    db
}

/// (balance, bought as read per D1).
async fn wallet(db: &PgPool, user: &str) -> (f64, f64) {
    sqlx::query_as("SELECT balance_usd, LEAST(bought_usd, GREATEST(balance_usd, 0)) FROM user_credits WHERE user_id = $1")
        .bind(user)
        .fetch_one(db)
        .await
        .unwrap()
}

/// (allowance, used, grant_key).
async fn allowance(db: &PgPool, user: &str) -> (f64, f64, Option<String>) {
    sqlx::query_as("SELECT allowance_usd, used_usd, grant_key FROM user_open_allowance WHERE user_id = $1")
        .bind(user)
        .fetch_one(db)
        .await
        .unwrap()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// Grant one plan the way the Stripe invoice path does.
async fn grant(service: &CostServiceImpl, user: &str, key: &str, plan_id: &str, amount: f64, cap: f64) -> f64 {
    service
        .grant_plan_credits_keyed(user, key, "stripe_subscription", amount, cap, open_allowance_for(plan_id, amount))
        .await
        .unwrap()
}

#[tokio::test]
async fn max_grant_sets_half_as_open_allowance_and_plus_gets_none() {
    assert!(close(open_allowance_for("super", 110.0), 55.0));
    assert!(close(open_allowance_for("ultra", 220.0), 110.0));
    assert!(close(open_allowance_for("plus", 22.0), 0.0));
    assert!(close(open_allowance_for("free", 22.0), 0.0));

    let db = wallet_db().await;
    let service = CostServiceImpl::new(db.clone());
    let balance = grant(&service, "max_user", "stripe-invoice-in_1", "super", 110.0, 50.0).await;
    assert!(close(balance, 110.0));
    let (allow, used, key) = allowance(&db, "max_user").await;
    assert!(close(allow, 55.0) && close(used, 0.0), "allowance {allow} used {used}");
    assert_eq!(key.as_deref(), Some("stripe-invoice-in_1"));

    grant(&service, "plus_user", "stripe-invoice-in_2", "plus", 22.0, 10.0).await;
    let (allow, _, _) = allowance(&db, "plus_user").await;
    assert!(close(allow, 0.0));
}

#[tokio::test]
async fn replayed_grant_does_not_reset_used_allowance() {
    let db = wallet_db().await;
    let service = CostServiceImpl::new(db.clone());
    grant(&service, "u1", "stripe-invoice-in_1", "super", 110.0, 50.0).await;
    let debit = service.debit_open_usage("u1", 10.0, "inference-req_1").await.unwrap();
    assert!(close(debit.plan_usd, 10.0) && close(debit.bought_usd, 0.0));

    // Stripe retries the same invoice: nothing moves.
    let balance = grant(&service, "u1", "stripe-invoice-in_1", "super", 110.0, 50.0).await;
    assert!(close(balance, 100.0), "replay must not grant again: {balance}");
    let (allow, used, _) = allowance(&db, "u1").await;
    assert!(close(allow, 55.0) && close(used, 10.0), "replay must not reset used: {used}");

    // The next real grant does reset it: 100 rolls, clipped to the 50 cap, + 110.
    let balance = grant(&service, "u1", "stripe-invoice-in_2", "super", 110.0, 50.0).await;
    assert!(close(balance, 160.0));
    let (_, used, key) = allowance(&db, "u1").await;
    assert!(close(used, 0.0));
    assert_eq!(key.as_deref(), Some("stripe-invoice-in_2"));
}

#[tokio::test]
async fn rollover_clips_plan_credits_but_keeps_bought_credits() {
    let db = wallet_db().await;
    let service = CostServiceImpl::new(db.clone());
    // Plus: $15 of plan credits left, then a $100 pack.
    grant(&service, "u1", "stripe-invoice-in_1", "plus", 15.0, 10.0).await;
    service.add_credits("u1", 100.0, "stripe-evt_pack", "stripe").await.unwrap();
    assert_eq!(wallet(&db, "u1").await, (115.0, 100.0));

    // Renewal: bought 100 kept whole, plan 15 clipped to the $10 cap, + $22.
    let balance = grant(&service, "u1", "stripe-invoice-in_2", "plus", 22.0, 10.0).await;
    assert!(close(balance, 132.0), "{balance}");
    let (balance, bought) = wallet(&db, "u1").await;
    assert!(close(balance, 132.0) && close(bought, 100.0), "bought {bought}");
}

#[tokio::test]
async fn bought_reads_clamped_after_an_ordinary_debit_then_a_pack() {
    let db = wallet_db().await;
    let service = CostServiceImpl::new(db.clone());
    service.add_credits("u1", 30.0, "stripe-evt_1", "stripe").await.unwrap();
    // An ordinary (non-open) debit does not touch bought_usd; the read clamps it.
    service.deduct_credits_for_usage("u1", "inference-req_1", 20.0, "inference").await.unwrap();
    let (balance, bought) = wallet(&db, "u1").await;
    assert!(close(balance, 10.0) && close(bought, 10.0), "bought {bought} over balance {balance}");

    // A $25 pack lands on top of the clamped $10, not on the stale $30.
    service.add_credits("u1", 25.0, "stripe-evt_2", "stripe").await.unwrap();
    let (balance, bought) = wallet(&db, "u1").await;
    assert!(close(balance, 35.0) && close(bought, 35.0));

    // A plan grant now adds plan credits beside the $35 bought.
    grant(&service, "u1", "stripe-invoice-in_1", "plus", 22.0, 10.0).await;
    let (balance, bought) = wallet(&db, "u1").await;
    assert!(close(balance, 57.0) && close(bought, 35.0));
}

#[tokio::test]
async fn one_turn_overrun_comes_from_plan_credits_and_counts_as_used() {
    let db = wallet_db().await;
    let service = CostServiceImpl::new(db.clone());
    // Max: $110 plan credits, $55 open allowance, then $2 bought.
    grant(&service, "u1", "stripe-invoice-in_1", "super", 110.0, 50.0).await;
    service.add_credits("u1", 2.0, "stripe-evt_1", "stripe").await.unwrap();

    let first = service.debit_open_usage("u1", 50.0, "inference-req_1").await.unwrap();
    assert!(close(first.plan_usd, 50.0) && close(first.bought_usd, 0.0));

    // $10 turn: $5 of allowance left, then the $2 bought, then $3 overrun from plan credits.
    let second = service.debit_open_usage("u1", 10.0, "inference-req_2").await.unwrap();
    assert!(close(second.plan_usd, 8.0) && close(second.bought_usd, 2.0), "{second:?}");
    let (balance, bought) = wallet(&db, "u1").await;
    assert!(close(balance, 52.0) && close(bought, 0.0));
    let (_, used, _) = allowance(&db, "u1").await;
    assert!(close(used, 58.0), "overrun counts in used: {used}");

    // A retried settlement of the same request takes nothing.
    let replay = service.debit_open_usage("u1", 10.0, "inference-req_2").await.unwrap();
    assert!(replay.replayed && close(replay.plan_usd, 0.0) && close(replay.bought_usd, 0.0));
    assert!(close(wallet(&db, "u1").await.0, 52.0));
    let (amount, source): (f64, String) =
        sqlx::query_as("SELECT amount_usd, source FROM credit_transactions WHERE transaction_id = 'inference-req_2'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert!(close(amount, -10.0));
    assert_eq!(source, "inference");
}
