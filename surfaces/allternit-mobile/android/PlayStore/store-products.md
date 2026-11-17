# Google Play products — Allternit (com.allternit.app)

Source for the Play Console product drafts (plan step 16). Ids are permanent once created. Possible only after a build with the billing permission is on the internal track. Nothing here is entered in the console yet.

Ids and prices match `ios/AppStore/store-products.md` (approved by Eoj 2026-11-17, G0). Play limits: product name 55 characters, description 80 characters; the strings below are well inside both.

## Subscriptions

One base plan each: id `monthly`, auto-renewing, billing period 1 month, no free trial, no intro price. Prices are set for the United States and left to Play's conversion elsewhere unless Eoj says otherwise.

| Product id | Base plan | Name | Description | US price | Credits | Server plan id |
|---|---|---|---|---|---|---|
| `plan_plus` | `monthly` | Allternit Plus (14) | $22 in Cloud credits every month (32) | $24.99 | $22 per month | `plus` |
| `plan_max` | `monthly` | Allternit Max (13) | $110 in Cloud credits every month (33) | $124.99 | $110 per month | `super` |
| `plan_ultra` | `monthly` | Allternit Ultra (15) | $220 in Cloud credits every month (33) | $249.99 | $220 per month | `ultra` |

Grace period: 14 days. Play's maximum for subscriptions is 14 days, so the 16 days set on Apple cannot be matched. Confirm the value in the console when the products are created. On-hold behaviour stays at Play's default.

## One-time products

| Product id | Name | Description | US price | Credits |
|---|---|---|---|---|
| `credits_10` | $10 Cloud credits (17) | Adds $10 to your Allternit Cloud balance (40) | $11.99 | $10 |
| `credits_25` | $25 Cloud credits (17) | Adds $25 to your Allternit Cloud balance (40) | $29.99 | $25 |
| `credits_50` | $50 Cloud credits (17) | Adds $50 to your Allternit Cloud balance (40) | $59.99 | $50 |
| `credits_100` | $100 Cloud credits (18) | Adds $100 to your Allternit Cloud balance (41) | $119.99 | $100 |

Packs are consumable: the server consumes the purchase after it grants the credits. The app never consumes a pack itself.

## Real-time developer notifications

| Item | Value |
|---|---|
| Pub/Sub delivery | Push subscription |
| Push endpoint | `https://api.allternit.com/api/v1/billing/store/google/rtdn?token=<secret>` |
| Token | The server variable `GOOGLE_PLAY_RTDN_TOKEN`, at least 16 characters. The server compares it in constant time. Never written to a file in the repo. |
| Topic | One topic in the Allternit Google Cloud project; Play Console > Monetization setup > Real-time developer notifications points at it |

Server variables used: `GOOGLE_PLAY_SERVICE_ACCOUNT_JSON`, `GOOGLE_PLAY_RTDN_TOKEN`, `GOOGLE_PLAY_PACKAGE_NAME` (default `com.allternit.app`).

## Testing

License testers on the internal track buy without a charge. Like the Apple sandbox, a tester account that is not on `STORE_SANDBOX_GRANT_USER_IDS` is acknowledged and consumed but granted no credits.
