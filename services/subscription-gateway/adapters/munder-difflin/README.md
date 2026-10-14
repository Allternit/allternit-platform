# Munder Difflin native AAI adapter

Primary source: https://github.com/HarnessMD/munder-difflin at commit c9de1ca (src/main/webhook.ts, src/main/index.ts `lookupWebhookStatus`, HIVE.md, src/main/hive.ts).

Uses the supported inbound route of the Munder Difflin app: its webhook server. Nothing is typed into a terminal. The server listens on `http://127.0.0.1:3849` by default (override `SUBS_GATEWAY_MUNDER_DIFFLIN_URL`; must be loopback). Enable a webhook endpoint in Munder Difflin's Triggers settings and copy its id and secret.

Credential: JSON `{"endpointId":"<id>","secret":"<secret>"}` in the sealed account credential, or deployment `SUBS_GATEWAY_MUNDER_DIFFLIN_ENDPOINT_ID` and `SUBS_GATEWAY_MUNDER_DIFFLIN_SECRET`. Routes used: `POST /<endpointId>` with `x-md-webhook-secret`, and `GET /<endpointId>` with `x-md-webhook-token`.

Each message becomes one task routed to the floor's god orchestrator. The webhook addresses the floor, not a single agent, so the adapter exposes one agent, "Munder Difflin floor". The reply is the task's `result` once it is `done`. `blocked` and `rejected` are reported as failures. A message the operator holds (`awaiting-approval`) is polled until the timeout and then reported as unconfirmed. Approve it in Munder Difflin.

No context resume, cancellation, approvals or memory control is advertised. The task token is kept only inside the send and is never written into events. Send outcomes and contexts are kept in process.

Source-based checks only: static type-check passes. Live verification against a running Munder Difflin with an enabled endpoint remains pending.
