# Task (ao-phone-sms): phone numbers and SMS, backend (platform)
Read CHANNELS_MAP.md and CHANNELS_CONTRACTS.md first. Migrations: V214 (allternit-api) and 024 (cloud-api pg).

Goal: a bot gets a real phone number. People text it and get replies in a thread. Calls are handled by ao-voice-cloud and the voice session, not here.

## Build

### 1. Carrier interface in cloud-api (new module `carriers/`)
- `trait Carrier`: search numbers (country, area code/locality, type local|toll_free), buy, release, set messaging webhook, send SMS, parse+verify inbound webhook, 10DLC brand/campaign submit+status, toll-free verification submit+status, port-in order create+status.
- **Telnyx adapter** (primary): Numbers API, Messaging profiles, 10DLC ISV API, TFV API, porting. Cite the docs.
- **Twilio adapter** (fallback): IncomingPhoneNumbers, Messaging Services, TrustHub/A2P ISV API.
- Env: `ALLTERNIT_TELNYX_API_KEY`, `ALLTERNIT_TELNYX_PUBLIC_KEY` (webhook ed25519), `ALLTERNIT_TWILIO_*`. Unset means 503.

### 2. Numbers in cloud-api (pg 024)
- Table `phone_numbers`: id, user_id, runtime_id, bot_id, e164, carrier, carrier_number_id, type, sms_state (pending_registration|active|rejected|blocked), voice_state, created_at.
- Table `sms_registrations`: brand/campaign or TFV ids, state, rejection reason, submitted fields.
- Routes:
  - `GET /api/v1/phone/numbers/search`
  - `POST /api/v1/phone/numbers` (buy and assign to a bot; also creates the relay inbound route, provider `sms`)
  - `GET /api/v1/phone/numbers`
  - `DELETE /api/v1/phone/numbers/:id`
  - `POST /api/v1/phone/numbers/:id/registration` (business form → submit)
  - `GET .../registration` (status)
  - `POST /api/v1/phone/numbers/:id/port` (port-in)
- Carrier status webhooks update sms_state.

### 3. SMS in, at the cloud edge
- Add provider `sms` to `channel_inbound` (target `/webhooks/channels/sms`).
- **Before queueing**, in the cloud:
  - verify the carrier signature;
  - dedupe;
  - **handle STOP/STOPALL/UNSUBSCRIBE/CANCEL/END/QUIT, HELP and START**: keep a per-number opt-out list (`sms_opt_outs`), answer the HELP and STOP confirmations, and log consent changes to `sms_consent_log`.
- Opted-out senders never reach the runtime.

### 4. SMS out
- `POST /api/v1/channels/sms/send` in cloud. It refuses when:
  - the recipient opted out;
  - the number's sms_state is not active;
  - the reply falls outside the bot's allowed use (no cold outreach: only to numbers that texted first or have a consent record).

### 5. allternit-api (runtime): `channel_phone.rs`, new and yours
- `pub fn resolve_thread(db, rt, number_id, caller_e164) -> Result<(String /*thread_id*/, String /*session_id*/), String>` with key `phone:<botE164>:<callerE164>`. **This exact signature is frozen with the voice session.**
- An SMS `ChannelTransport` (provider `sms`) whose post goes through the cloud send route.
- Inbound SMS goes to route_inbound or resolve_thread, so calls and texts from one caller share a thread.
- Replies go out as SMS. Keep each reply ≤ 1,600 chars and split politely if longer.

### 6. Consent gate for outbound calls (cloud)
- `POST /api/v1/phone/calls/outbound` {numberId, to, botId, purpose}.
- It creates a consentRef only if the callee has prior consent: they texted or called this number first, or there's an explicit consent record. Otherwise 403 `{error:"no_consent"}`.
- Store it in `call_consents`.
- The actual dial (LiveKit CreateSIPParticipant) is ao-voice-cloud's. Expose `fn consent_ref_for(...)` for it.

## Tests
With fake carrier HTTP:
- search/buy;
- inbound SMS → relay request;
- STOP → no relay + confirmation + opt-out stored;
- send refused to an opted-out recipient;
- registration submit/status;
- `resolve_thread` shares a thread for the same caller;
- consent gate allow/deny.

## Notes
`docs/PHONE_SMS_NOTES.md`, then the sentinel `docs/PHONE_SMS_NOTES.sentinel`.
