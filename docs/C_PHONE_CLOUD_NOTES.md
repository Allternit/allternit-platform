# c-phone-cloud notes: phone numbers and SMS, cloud side

Scope: PHONE_SMS_TASK sections 1, 2, 3, 4, 6, all in `cmd/allternit-cloud-api`. Migration **pg 024**. Section 5 (`channel_phone.rs`) is c-phone-runtime's.

## Files

- `migrations_pg/024_phone_numbers.sql`: `phone_numbers`, `sms_registrations`, `sms_opt_outs`, `sms_consent_log`, `sms_inbound_seen`, `sms_outbound_log`, `call_consents`. Not added to the `MIGRATIONS` runner list (020/021 aren't either): **apply by hand to prod** (`psql -f`, idempotent `IF NOT EXISTS`).
- `src/carriers/{mod,telnyx,twilio}.rs`: `trait Carrier` + `CarrierHttp` seam (fake in tests), Telnyx and Twilio adapters.
- `src/routes/phone.rs`: every route, the SMS edge, the consent gate, `consent_ref_for`, `record_inbound_call`.
- `src/routes/channel_inbound.rs`: `target_path("sms") = /webhooks/channels/sms`; the sms edge hook in `inbound_inner` (verify → dedupe → keywords → queue a normalised body; undoes the dedupe mark if queueing fails); `new_key`/`sha256_hex`/`public_base` made `pub(crate)`.
- `src/lib.rs`, `src/routes/mod.rs`: `pub mod carriers`, `routes::phone::routes()` merged next to `channel_inbound`.
- `Cargo.toml`: `sha1`, `urlencoding` (both already in the workspace).
- `surfaces/docs/api/phone-numbers.mdx` + nav entry (`check_links.py`: 0 problems).

## Env (all unset = every phone route 503 `{"error":"phone_not_configured"}`; nothing runs at boot)

`ALLTERNIT_PHONE_CARRIER` (`telnyx` default | `twilio`), `ALLTERNIT_TELNYX_API_KEY`, `ALLTERNIT_TELNYX_PUBLIC_KEY` (base64 ed25519), `ALLTERNIT_TWILIO_ACCOUNT_SID`, `ALLTERNIT_TWILIO_AUTH_TOKEN`; optional `ALLTERNIT_PHONE_MAX_PER_USER` (3), `ALLTERNIT_SMS_DAILY_CAP` (1000/number/day), `ALLTERNIT_CLOUD_API_URL` (existing).
Telnyx: with an API key but no public key, inbound webhooks answer 503 (never accepted unverified).

## Verified API names (official OpenAPI specs, fetched 2026-10-02)

- Telnyx: https://github.com/team-telnyx/openapi (`openapi/spec3.json`): `GET /available_phone_numbers` (filter[country_code, national_destination_code, locality, phone_number_type, features, limit]), `POST /number_orders`, `GET|DELETE /phone_numbers`, `POST/PATCH/DELETE /messaging_profiles` (needs `whitelisted_destinations`), `POST /messages`, `POST /10dlc/brand`, `POST /10dlc/campaignBuilder`, `GET /10dlc/brand/{id}`, `GET /10dlc/campaign/{id}`, `POST /10dlc/phone_number_campaigns`, `POST|GET /messaging_tollfree/verification/requests[/{id}]`, `POST /porting_orders`, `GET|PATCH /porting_orders/{id}` (`phone_number_configuration.messaging_profile_id`, `messaging.enable_messaging`, `webhook_url`).
  Webhook: https://developers.telnyx.com/docs/messaging/messages/receiving-webhooks (`telnyx-signature-ed25519` over `{telnyx-timestamp}|{raw body}`, base64 public key; `data.event_type=message.received`, `data.payload.{id,direction,from.phone_number,to[0].phone_number,text}`).
  Enums used: campaign `campaignStatus` (MNO_PROVISIONED/MNO_ACCEPTED → approved; TCR_FAILED/TELNYX_FAILED/MNO_REJECTED/MNO_PROVISIONING_FAILED/TCR_SUSPENDED/TCR_EXPIRED → rejected), brand `status` REGISTRATION_FAILED, toll-free `verificationStatus` (Verified/Rejected/else pending), porting `status.value`.
- Twilio: https://github.com/twilio/twilio-oai (`spec/json/twilio_api_v2010.json`, `twilio_messaging_v1.json`): AvailablePhoneNumbers Local/TollFree, IncomingPhoneNumbers, Messages, Messaging `Services`, `Services/{sid}/PhoneNumbers`, `a2p/BrandRegistrations`, `Services/{sid}/Compliance/Usa2p`, `Tollfree/Verifications`. Webhook signature: https://www.twilio.com/docs/usage/webhooks/webhooks-security (HMAC-SHA1, matches Twilio's published example in a test).
- Contradiction with the task: none blocking. Two spots differ from the task text: (1) the task's `POST /numbers/:id/port` can't exist for a number we don't hold yet, so port-in is `POST /api/v1/phone/numbers/port` (creates the row) + `GET /api/v1/phone/numbers/:id/port` (status). (2) Twilio port-in needs signed LOA documents; Twilio returns 501 `carrier_unsupported` for port (do it on Telnyx or the Twilio console).

## Endpoints (JSON)

All need Clerk session or `compute`-scoped key. Errors are `{"error":"<code>","message"?}`.

- `GET /api/v1/phone/numbers/search?country=US&areaCode=415&locality=&type=local|toll_free&limit=10` → `{"carrier":"telnyx","numbers":[{"e164","type","locality","monthlyCost"}]}`
- `POST /api/v1/phone/numbers` `{"e164","runtimeId","botId","type"?}` → 201 `{"number":{id,runtimeId,botId,e164,carrier,type,smsState,voiceState,portState,createdAt}}`; 409 `number_taken`, 403 `number_limit`, 404 `runtime_not_found`, 400 `bad_request`, 502 `carrier_error`. Also creates the relay route (provider `sms`), the carrier messaging profile/service, and points the carrier's inbound webhook at `https://api.allternit.com/channels/in/<key>`.
- `POST /api/v1/phone/numbers/port` same body → 201 `{"number"}` (`portState` = carrier status word).
- `GET /api/v1/phone/numbers` → `{"numbers":[…]}`; `DELETE /api/v1/phone/numbers/:id` → 204 (carrier release, relay route revoked).
- `POST /api/v1/phone/numbers/:id/registration` body = business form (camelCase: entityType, legalName, displayName, ein, website, vertical, street, city, state, postalCode, country, contactFirstName/LastName/Email/Phone, useCase, useCaseSummary, sampleMessages[], optInWorkflow, optInImageUrls[], messageVolume, privacyPolicyUrl, termsUrl, twilioCustomerProfileSid, twilioA2pProfileSid) + optional `kind` (`10dlc`|`tollfree`; default by number type) → 201 `{"registration":{id,kind,state,rejectionReason,brandId,campaignId,tfvId,submittedAt,updatedAt},"smsState"}`; 409 `already_registered` (a rejected one can be resubmitted). EIN stored masked.
- `GET /api/v1/phone/numbers/:id/registration` → same shape; re-reads a pending one from the carrier; `smsState` becomes `active` / `rejected`. 404 `no_registration`.
- `GET /api/v1/phone/numbers/:id/port` → `{"port":{"orderId","state"},"number"}`.
- `POST /api/v1/phone/numbers/:id/consent` `{"e164","source","evidence"?}` → 201 `{"ok":true,"optedOut":bool}` (explicit consent; never lifts a STOP).
- `POST /api/v1/phone/webhooks/:carrier` (public, signed): verifies the carrier signature, then re-reads from the carrier every pending registration/port order whose id appears in the payload (no reliance on unverified event names).

### The send route: the contract with c-phone-runtime

`POST /api/v1/channels/sms/send` (same auth as `channel-inbound-routes`)
```json
{ "numberId": "<phone_numbers.id>", "to": "+15551230000", "text": "≤1600 chars" }
```
→ 200 `{"ok":true,"messageId":"…","parts":1}`
Refusals: 403 `sms_not_active` | `recipient_opted_out` | `no_consent`; 429 `daily_limit`; 400 `bad_request` (not E.164, empty, >1600 chars); 404 `number_not_found`; 503 `phone_not_configured`; 502 `carrier_error`. The runtime must not retry 403s. `from` is the number's own E.164 (the cloud adds it).

### Inbound body the runtime receives at `/webhooks/channels/sms`

Content-Type `application/json`, **already verified, deduped and consent-filtered** (no carrier signature headers are forwarded, so the runtime must not verify one):
```json
{ "provider":"sms","messageId":"…","numberId":"…","botId":"…","from":"+1555…","to":"+1415…","text":"…","receivedAt":"…" }
```
`botId` and `numberId` come from the cloud's `phone_numbers` row, so thread key `phone:<to>:<from>` needs no extra lookup. The runtime's `x-allternit-channel-queued-at` header still applies.

### Outbound-call consent gate

`POST /api/v1/phone/calls/outbound` `{"numberId","to","botId","purpose"}` → 200 `{"consentRef":"cc_…","basis":"inbound_text|inbound_call|opt_in|explicit","expiresAt":"+30min"}` or 403 `{"error":"no_consent"}`. Stored in `call_consents`.
For ao-voice-cloud (Rust, same crate): `routes::phone::consent_ref_for(&db, user_id, number_id, to_e164, bot_id, purpose) -> Result<Option<ConsentRef{id,basis,expires_at}>, PhoneError>`; and `routes::phone::record_inbound_call(&db, number_id, caller_e164)` to log "they called first".

## Tests

`CARGO_TARGET_DIR=/Users/joe/Desktop/allternit-workspace/.gw-target cargo test -p allternit-cloud-api --lib -- carriers phone:: channel_inbound` (needs the CI Postgres on localhost:5432 / `TEST_DATABASE_URL`; schema-per-test, runs the real 020 + 024 SQL). Covers: Telnyx signature good/tampered/stale/wrong-key/no-key; Twilio signature against Twilio's published example; search/buy (+rollback of orphan profile and of the reserved row/route); inbound text → normalised queue row over the real `/channels/in/:key` router with the real Telnyx adapter; dedupe + retry; STOP → no relay + confirmation + opt-out + consent log, HELP, START; send refused (not active, opted out, no consent, too long, daily cap, wrong user); explicit consent vs STOP; registration submit/status/reject/resubmit/approve; status webhook; call-consent allow/deny; port-in; release + rebuy.

## Left / decisions for Eoj

- **Who pays for a number.** Buying is gated only by carrier env and a per-user limit (3). There is no plan/entitlement/billing check; that needs a product decision before the env is switched on.
- STOP handling is defensive. If the carrier also auto-answers STOP/HELP at its own level, a sender may get two confirmations.
- Telnyx 10DLC: brand and campaign are created in one submit; a brand created before a campaign failure is orphaned at the carrier (resubmit makes a new brand). The number is linked to the campaign when its status refresh first sees it approved.
- Prod DB: apply `024_phone_numbers.sql` by hand before setting any carrier env.

(PR number and merge commit are appended below after merge.)
