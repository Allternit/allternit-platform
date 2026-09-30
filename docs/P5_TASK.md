# P5 — MCP App commerce, Stripe TEST MODE ONLY (platform repo here + web repo at ../wt-p5-commerce-web)
Allternit's directory allows digital goods/subscriptions (ChatGPT only allows physical). Model: OpenAI's requestCheckout /
complete_checkout flow (https://developers.openai.com/plugins/build/monetization), but the payment sheet is Allternit's.
HARD: Stripe test keys only (sk_test_/pk_test_ from env); refuse to start if a live key is configured for this feature.
Never trust totals from the View; the server recomputes from the app's checkout session.
Backend (allternit-api):
1. Stripe Connect Express: POST create connected account + onboarding link for an MCP App developer; webhook for account.updated.
2. Checkout sessions: MCP App's server returns a session (id, line_items, totals in minor units, currency, merchant = connected
   account, legal/refund/support links). Host validates it, creates a PaymentIntent with application_fee_amount (platform fee
   from config, default 0 until Eoj sets it) + transfer_data.destination, confirms after user approval, then calls the app's
   `complete_checkout` tool with {checkout_session_id, payment_intent_id}; returns the app's order result. Idempotency keys.
3. Orders ledger table (migration file written, NOT applied), refunds endpoint (full refund, host-initiated), dispute webhook logging.
Web (../wt-p5-commerce-web, branch p5-commerce-web):
4. Host-rendered payment sheet component (Stripe Elements, test mode): line items, totals, merchant name, legal links, explicit
   Pay button. The View can only REQUEST it; the sheet renders in the host, outside the iframe.
5. API module + a `requestCheckout(session)` host handler function with a clear hook point; the x-allternit/requestCheckout
   bridge method lives on branch p6-allternit-extensions (being built in parallel) — document how to wire it, don't duplicate.
Tests: fee math, total mismatch rejection, live-key refusal, idempotency, webhook signature check.
Also draft (as DRAFTS for a lawyer, clearly marked) docs/P5_LEGAL_DRAFT_OUTLINES.md: headings/points only for marketplace
terms, developer agreement, refund policy. No final legal language.
Deliverable: docs/P5_NOTES.md in this worktree.

## Rules
- Only your worktree(s)/branch(es). Commit there. Do NOT push, merge, deploy, apply migrations to shared/prod DBs.
- Time-box ~90 minutes; smallest complete tested version; list cuts in NOTES. Match surrounding code style.
- UI: white + --neutral-fill surfaces, never tan.
- Deliverable NOTES file (files changed, commands + results, cuts, what Eoj must do). Last line exactly: `status: done`
