# Hosted runtime operations

Allternit managed hosting is a paid control-plane product. Local compute stays
included and unmetered; customer-purchased VPS instances stay provider-billed;
enterprise BYOC infrastructure stays in the customer's cloud and is metered by
the enterprise service.

## One-time setup

1. Let `Publish Hosted Runtime Image` publish
   `ghcr.io/allternit/allternit-hosted-runtime:latest`. The GHCR package must be
   public so Fly Machines can pull it without storing registry credentials.
2. Configure the Cloud API with a Fly organization/app and two secrets:

   ```sh
   fly secrets set \
     FLY_API_TOKEN=... \
     FLY_ORG_SLUG=... \
     ALLTERNIT_BILLING_SYNC_SECRET=... \
     --app allternit-cloud-api
   ```

   Use an organization-scoped Fly token with only the access needed to manage
   the hosted-runtime app. `ALLTERNIT_BILLING_SYNC_SECRET` must be a random
   value of at least 32 characters. Set `FLY_ORG_SLUG` explicitly so Machines
   can never fall back to a personal Fly account. Optional configuration
   includes `FLY_HOSTED_APP`, `HOSTED_RUNTIME_IMAGE`, and the public billing URLs.
3. Keep `HOSTED_RUNTIME_IDLE_TIMEOUT_MINUTES` enabled. The lifecycle reconciler
   also stops a machine when its plan is revoked, monthly hours are exhausted,
   or Fly reports the machine stopped.

## Grant or revoke a paid plan

The billing system calls the internal endpoint with its own stable event ID.
Retries with the same event ID are idempotent.

```sh
curl -X POST https://api.allternit.com/api/v1/internal/billing/hosted-entitlement \
  -H "Content-Type: application/json" \
  -H "X-Allternit-Billing-Secret: $ALLTERNIT_BILLING_SYNC_SECRET" \
  -d '{
    "eventId": "subscription-event-id",
    "userId": "user_clerk_id",
    "email": "customer@example.com",
    "planTierId": "pro",
    "source": "subscription"
  }'
```

Use `planTierId: "free"` with a new event ID to revoke managed hosting. The
background reconciler stops any running hosted machine within the configured
reconcile interval. Current defaults are one 1 GB machine and 100 hours/month
for Pro, or five 2 GB machines and 500 hours/month for Team.

## Relay load and cost measurement

Create a managed runtime from **Settings → Plans & compute**, then run:

```sh
ALLTERNIT_CLERK_TOKEN=... node scripts/hosted-runtime-relay-load.mjs \
  --duration 60 \
  --concurrency 4 \
  --start
```

The benchmark goes through the real authenticated runtime relay, reports
throughput and p50/p95/p99 latency, and compares entitlement metering before and
after the run to calculate the configured estimated cost per runtime-hour. The
actual charge remains the Fly invoice/Cost Explorer total, which may also
include stopped root filesystem, volume, network, and other account charges. If
the script starts a stopped machine, it stops it after the test unless
`--keep-running` is passed.
