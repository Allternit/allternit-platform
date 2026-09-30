# P5 — MCP App commerce (Stripe TEST MODE ONLY)

Branches: `p5-commerce` (this repo, platform/API) and `p5-commerce-web` (`../wt-p5-commerce-web`). Committed locally; nothing pushed, merged, deployed, or migrated.

## Files changed

Platform (`cmd/allternit-api`):
- `src/commerce.rs` — config + live-key refusal, fee math, session validation (server recomputes totals), Stripe API trait + reqwest client, Connect Express onboarding, PaymentIntent (application_fee_amount + transfer_data.destination, idempotency keys), `complete_checkout` call via `AppCaller` (prod: MCP dispatcher `<app_id>.complete_checkout`), orders ledger, full refund, Stripe webhook signature check, `account.updated` + dispute logging. Unit tests included.
- `src/commerce_routes.rs` — HTTP routes (`/api/v1/commerce/*`, public `POST /webhooks/stripe-commerce`), startup guard.
- `migrations/V198__commerce_orders.sql` — tables: connected accounts, checkout sessions, orders, disputes, webhook events. **Written, not applied** (embedded refinery migration; runs when an API process next opens a DB).
- `src/lib.rs`, `src/main.rs` — module registration, route merges, fatal exit at startup if a live key is configured.
- `docs/P5_LEGAL_DRAFT_OUTLINES.md` — DRAFT outlines only (marketplace terms, developer agreement, refund policy, open questions for counsel).

Web (`../wt-p5-commerce-web`):
- `src/lib/commerce/commerce-api.ts` — typed API client + `formatMinor`.
- `src/lib/commerce/request-checkout.ts` — `requestCheckout({appId, session})` host handler; `registerCheckoutPresenter` is the hook point.
- `src/lib/commerce/stripe-js.ts` — dependency-free Stripe.js loader; rejects non-`pk_test_` keys.
- `src/components/commerce/CheckoutSheet.tsx` — host-rendered sheet (Stripe Payment Element, line items, totals, merchant, legal links, explicit Pay) + `CheckoutSheetHost`. White + `--neutral-fill` surfaces.
- Tests beside each file.

## Commands and results
- `cargo test -p allternit-api --lib commerce` → **20 passed, 0 failed** (fee math, total mismatch rejection, live-key refusal, idempotency, webhook signature/replay, onboarding, pay/3DS/fulfilment retry, refunds, disputes).
- `npx vitest run src/lib/commerce src/components/commerce` (web) → **4 files, 12 tests passed**.
- `tsc` over the new web files only → no errors in them. Full-repo typecheck not run.

## How it works
1. Developer: `POST /commerce/connect/accounts` → Express account + onboarding URL; `account.updated` webhook sets `charges_enabled`.
2. App server returns a session; host handler `requestCheckout` posts it to `POST /commerce/checkout/sessions`. Server validates (line totals, subtotal, tax, total, https legal links, merchant == the app's connected account and charges enabled), stores its own copy and the platform fee. A mismatch is a 422; nothing is "corrected".
3. Sheet shows the server's copy. Pay sends only `{payment_method}`; the server charges the stored total (PI idempotency key `allternit-commerce-pi-<session>`), handles `requires_action` via `/complete`, re-checks the intent against the stored session, then calls the app's `complete_checkout {checkout_session_id, payment_intent_id}` once (atomic claim) and records the order. Failed fulfilment → `fulfillment_failed`, retryable via `/complete`.
4. Refund: `POST /commerce/orders/:id/refund` (operators only), full, reverses transfer + app fee, idempotent.

## Wiring `x-allternit/requestCheckout` (branch p6-allternit-extensions)
In that bridge method's handler: `return requestCheckout({ appId, session: params.session })` (from `@/lib/commerce/request-checkout`). `appId` must be the host's own knowledge of which MCP server the iframe is, never a View-sent field; it must not contain `.`. Mount `<CheckoutSheetHost />` once near the app root. Not duplicated here.

## Cuts / limits
- One-time charges only; no subscriptions, partial refunds, or tax calculation (tax is the app's stated `tax_minor`, validated arithmetically only).
- No route-level (AppState) HTTP tests; logic is tested at the service layer with fake Stripe/app. Real Stripe test API not called (no keys here).
- Config is read from env per request; Stripe client is built per request (no pooling).
- `@stripe/stripe-js` npm package not added; loader injects js.stripe.com instead (no lockfile change).
- Web sheet uses plain buttons/inline styles, not the design-system Button.
- Stale `fulfilling` claims are re-taken after 5 min; no background reconciler.
- Migration version V198 may collide with another branch's V198; renumber at merge if so.

## What Eoj must do
- Set env: `ALLTERNIT_COMMERCE_STRIPE_SECRET_KEY` (sk_test_), `..._PUBLISHABLE_KEY` (pk_test_), `..._WEBHOOK_SECRET`, `ALLTERNIT_COMMERCE_OPERATOR_USER_IDS` (refund-capable user ids); optionally `..._PLATFORM_FEE_BPS` / `..._FIXED_MINOR` (default 0).
- Register Stripe webhook `https://<api>/webhooks/stripe-commerce` (events: `account.updated`, `charge.dispute.*`) and enable Connect.
- Decide fee, and who may refund; have a lawyer write the real legal docs from the outlines.
- Review/apply migration V198 when ready; wire the p6 bridge method.
- Note: a stop hook complained about the shared checkout (main 88 behind, ~50 stale branches from other sessions). Not touched — outside this task's worktrees.

status: done
