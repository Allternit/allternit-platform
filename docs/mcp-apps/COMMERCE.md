# MCP App commerce

Lets an MCP App sell something through a payment sheet the Allternit host renders. **Stripe test mode only.** It is
not enabled in production and the migration (V199) has not been applied there. Code: `commerce.rs`,
`commerce_routes.rs`, `migrations/V199__commerce_orders.sql`. Env vars are in [OPERATIONS.md](OPERATIONS.md).

## Test-mode rule

Keys come only from `ALLTERNIT_COMMERCE_*` variables; there is no fallback to another Stripe key. Any key containing
`_live_`, a secret key not starting `sk_test_`/`rk_test_`, or a publishable key not starting `pk_test_` is refused.
On startup a refused config only logs `commerce disabled`; the rest of the API keeps running and every commerce route
answers 503. With no secret key set, commerce is off (`GET /commerce/config` returns `enabled: false`).

## Flow

1. **Onboard.** `POST /commerce/connect/accounts` creates a Stripe Connect Express account and an onboarding link for an app.
   The `account.updated` webhook sets `charges_enabled`.
2. **Open a session.** The app's server returns a checkout session (line items, totals in minor units, currency, merchant
   `acct_…`, https terms/refund/support links). The host client posts it to `POST /commerce/checkout/sessions`. The server
   recomputes line totals, subtotal, tax and total. A mismatch is a 422 and nothing is corrected. Limits: 1–100 line items,
   quantity 1–10,000, total at most 99,999,999 minor units. The merchant must be the app's connected account with charges enabled.
   The stored copy is the only source of the charge amount; the View never supplies one.
3. **Pay.** The sheet shows the server's copy. `POST /commerce/checkout/sessions/:id/pay` takes only the payment method, creates
   and confirms a PaymentIntent for the stored total with `application_fee_amount` and `transfer_data.destination`
   (idempotency key `allternit-commerce-pi-<session>`). `POST .../complete` finishes 3DS and retries fulfilment.
4. **Fulfil.** The host calls the app's `<app_id>.complete_checkout` tool once with
   `{checkout_session_id, payment_intent_id}` (atomic claim on the order row) and records the order. If fulfilment fails the
   order is `fulfillment_failed` and `/complete` can retry. A claim stuck in `fulfilling` for over 5 minutes can be re-taken.
5. **Refund.** `POST /commerce/orders/:id/refund` is a full refund that reverses the transfer and the app fee, idempotent. Only
   user ids in `ALLTERNIT_COMMERCE_OPERATOR_USER_IDS` may call it; others get 403.

The platform fee is `total × BPS / 10,000 + fixed`, capped at the total; both settings default to 0.

## Webhook

`POST /webhooks/stripe-commerce` is public and verified by Stripe signature with a 300 s tolerance, so replays outside that
window fail. Each Stripe event id is processed once (`commerce_webhook_events`). `account.updated` updates the account.
Disputes are logged in `commerce_disputes`; nothing acts on them automatically.

## Tables (V199)

`commerce_connected_accounts`, `commerce_checkout_sessions`, `commerce_orders`, `commerce_disputes`, `commerce_webhook_events`.

## Not done

- One-time charges only. No subscriptions, partial refunds, or tax calculation (the app's `tax_minor` is checked for arithmetic only).
- No route-level HTTP tests; logic is tested with a fake Stripe and a fake app. The real Stripe test API was never called.
- No background reconciler for stuck orders.
- Legal documents (marketplace terms, developer agreement, refund policy) are outlines only, kept outside this repo. A lawyer must write the real ones.
- The host UI (checkout sheet, `requestCheckout` handler) lives in the web repo, and wiring it to the app bridge's
  `x-allternit/requestCheckout` method is not done here. `appId` must come from the host's own knowledge of which server the
  iframe is, never from the View, and must not contain `.`.
