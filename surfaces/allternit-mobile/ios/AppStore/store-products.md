# App Store in-app purchases — Allternit for iPhone (com.allternit.mobile)

Source for the App Store Connect product drafts (plan step 12). Ids, names and prices approved by Eoj 2026-11-17 (G0). Product ids are permanent once created. Numbers in brackets are character counts. Limits: display name 30, description 45.

The prices have not been checked against this account's live price-point list; step 12 reads that list before saving. Apple publishes steps of $0.50 between $10 and $50, $1 between $50 and $200, and $5 between $200 and $500, and all seven prices fit those steps.

## Subscription group

| Field | Value |
|---|---|
| Group reference name | Allternit Plans |
| Group display name (App Store, 30) | Allternit Plans (15) |
| Family Sharing | Off |
| Grace period | 16 days, all renewals, production and sandbox (set at step 15, saved only on Eoj's go) |

Levels (1 is highest): Ultra is level 1, Max is level 2, Plus is level 3. Moving up is an upgrade, moving down is a downgrade, and Apple handles proration.

## Subscriptions

All three renew monthly (1 month duration).

| Reference name | Product id | Level | Display name | Description | US price | Credits | Server plan id |
|---|---|---|---|---|---|---|---|
| Plus Monthly | `com.allternit.mobile.plan.plus.monthly` | 3 | Allternit Plus (14) | $22 in Cloud credits every month (32) | $24.99 | $22 per month | `plus` |
| Max Monthly | `com.allternit.mobile.plan.max.monthly` | 2 | Allternit Max (13) | $110 in Cloud credits every month (33) | $124.99 | $110 per month | `super` |
| Ultra Monthly | `com.allternit.mobile.plan.ultra.monthly` | 1 | Allternit Ultra (15) | $220 in Cloud credits every month (33) | $249.99 | $220 per month | `ultra` |

The Max products map to the server plan id `super`.

## Consumables

| Reference name | Product id | Display name | Description | US price | Credits |
|---|---|---|---|---|---|
| Credits 10 | `com.allternit.mobile.credits.10` | $10 Cloud credits (17) | Adds $10 to your Allternit Cloud balance (40) | $11.99 | $10 |
| Credits 25 | `com.allternit.mobile.credits.25` | $25 Cloud credits (17) | Adds $25 to your Allternit Cloud balance (40) | $29.99 | $25 |
| Credits 50 | `com.allternit.mobile.credits.50` | $50 Cloud credits (17) | Adds $50 to your Allternit Cloud balance (40) | $59.99 | $50 |
| Credits 100 | `com.allternit.mobile.credits.100` | $100 Cloud credits (18) | Adds $100 to your Allternit Cloud balance (41) | $119.99 | $100 |

App Review may question "$10 Cloud credits" sold at $11.99. The fallback name is "10 credits"; the descriptions stay as written.

Net to Allternit: at the 15% Small Business Program rate every pack nets at least the credits it grants. At 30% every pack nets less than the credits it grants, so the Small Business Program decision (G2) decides whether packs make money on Apple.

## App Review notes for the in-app purchases

Paste into the review notes of the first version that includes the products:

```
In-app purchases are in Settings > Plan and credits.

Sign in with the demo account appreview@allternit.com (password in the App Review sign-in fields). Purchases in the sandbox grant credits and plans only for accounts on our allow list. The demo account is on that list. Any other account can buy in the sandbox, and Apple is charged nothing, but our server records the purchase without granting credits.

Subscriptions (Allternit Plus, Max, Ultra) add monthly Cloud credits to the account balance. Credit packs add a one-time amount to the same balance. Credits are spent on Allternit Cloud models and cloud computers and are not tied to this device.

A person who already pays for a plan on allternit.com sees a message that the plan is billed by Stripe and cannot buy a second plan in the app. Restore Purchases and Manage Subscriptions are on the same screen.
```

Open item for Eoj before submission: the demo account's user id must be in the server variable `STORE_SANDBOX_GRANT_USER_IDS`, or the reviewer's purchases grant nothing and the paragraph above is false.

## App Store Server Notifications, version 2

Set at step 15 (App Store Connect > App Information > App Store Server Notifications). Saved only on Eoj's go.

| Environment | URL |
|---|---|
| Production | `https://api.allternit.com/api/v1/billing/store/apple/notifications` |
| Sandbox | `https://api.allternit.com/api/v1/billing/store/apple/notifications/sandbox` |

Version: 2.

## Keys

The In-App Purchase key is "Allternit Server" (key id `87N65U8U7Y`). The private key is in the macOS Keychain, service `allternit-apple-iap`. It is never written to a file in the repo.
