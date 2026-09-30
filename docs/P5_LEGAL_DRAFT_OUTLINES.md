# P5 legal — DRAFT OUTLINES FOR A LAWYER

> **DRAFT — NOT LEGAL LANGUAGE. NOT FOR PUBLICATION.**
> Headings and discussion points only, to brief counsel. Nothing here is a
> term, policy, or promise, and none of it may be shown to users or
> developers until a lawyer has written and approved the real documents.
> Product facts below describe how P5 is built (Stripe Connect Express,
> destination charges, Allternit-rendered payment sheet, Stripe test mode
> only today) — counsel should confirm each against the code and Stripe's
> current terms.

## 1. Marketplace Terms (buyers / users of the MCP App directory)

- Parties and who is the seller of record (the app developer vs. Allternit)
- What "checkout in Allternit" means: Allternit renders the sheet and processes the payment; the app never receives card data
- Digital goods and subscriptions only (contrast: physical goods excluded)
- Pricing, currency, taxes shown at checkout (who calculates/collects tax — open question for counsel)
- Order confirmation and delivery: the app confirms via `complete_checkout`; what happens if the app fails to confirm after payment
- Subscriptions: renewal, cancellation, price changes (not built in P5 — one-time charges only; scope question)
- Refunds and cancellations (points to §3)
- Disputes/chargebacks between buyer and Allternit/developer
- Acceptable use; prohibited goods and services
- Buyer data: what Allternit shares with the developer on purchase
- Limitation of liability, disclaimers, governing law, venue, changes to terms
- Consumer-protection and auto-renewal law checklist by jurisdiction
- Test-mode disclosure while in test mode (no real charges)

## 2. Developer Agreement (MCP App developers who sell through Allternit)

- Eligibility, identity/business verification, and acceptance of Stripe Connected Account Agreement (Express onboarding)
- Roles: Allternit as platform; developer as merchant; Stripe as processor
- Platform fee: how it is stated, when it can change (config default is 0 today), notice period
- Payouts: timing, holds, reserves, minimums, negative balances
- Who bears refunds, chargebacks, dispute fees, and Stripe fees; reversal of transfers and application fees on refund
- Developer obligations: accurate listings and pricing, working `complete_checkout`, idempotent handling of repeat calls, fulfilment SLAs
- Required legal links per checkout session: terms, refund policy, support contact (privacy optional) — must be valid HTTPS and accurate
- Prohibited products, restricted businesses, sanctions/AML
- Tax responsibility (developer vs. platform; marketplace-facilitator rules)
- Data protection roles (controller/processor), buyer data use limits
- Security requirements, incident notice
- Audit, suspension, and termination rights; effect on pending orders and payouts
- IP, indemnity, limitation of liability, governing law
- Changes to the agreement

## 3. Refund Policy (marketplace-level baseline)

- Baseline refund window and conditions vs. developer-set policy shown at checkout (which wins)
- Host-initiated refunds only in P5: who at Allternit can trigger one and on what grounds (support policy — not yet defined)
- Full refunds only in P5 (no partial refunds) — confirm acceptable
- Failed fulfilment (charged but app did not confirm): automatic vs. requested refund
- Refund mechanics: transfer reversal and application-fee refund; time for funds to reach the buyer
- Subscription cancellations vs. refunds (not built)
- Chargebacks/disputes: relationship to refunds; effect on developer
- Statutory rights that cannot be waived, by region
- How buyers request a refund; response times; record-keeping

## Open questions for counsel

1. Seller of record / merchant of record — developer or Allternit? Drives tax, liability, and terms.
2. Sales tax / VAT collection responsibility and whether Stripe Tax is needed before live mode.
3. Money-transmission exposure under the chosen Connect charge type.
4. Consumer auto-renewal rules if subscriptions are added.
5. Minimum disclosures required on the payment sheet.
6. Data-sharing basis for sending buyer details to developers.
