# Task (ao-whatsapp): WhatsApp business and personal (platform)
Read CHANNELS_MAP.md and CHANNELS_CONTRACTS.md. Migrations: V218 and 028.

Policy: since 2025-10-15, Meta bans general-purpose AI assistants on the WhatsApp Business Platform. Allternit is the **Tech Provider for each user's own business number** and never runs a shared Allternit assistant number.

## Build

### 1. Business number, Embedded Signup (cloud-api, new `channels/whatsapp_es.rs`)
- Env: META_APP_ID, META_APP_SECRET, META_ES_CONFIG_ID, META_SYSTEM_TOKEN.
- `POST /api/v1/channels/whatsapp/embedded-signup/complete` {code, wabaId, phoneNumberId} does:
  - exchange the code for a business-integration token (sealed, pg 028);
  - `POST /<WABA>/subscribed_apps` with `override_callback_uri` = the owner's relay address (create the inbound route, provider `whatsapp`) and a generated verify token;
  - `POST /<phone_id>/register` with a generated 6-digit PIN.
- Meta's GET verify handshake is answered in the cloud.
- Send: `POST /api/v1/channels/whatsapp/send` enforces the **24-hour window**. Outside it, only a pre-approved template may go out; otherwise return 409 `{error:"outside_24h_window"}`.
- Cite the Graph API version and docs.

### 2. Personal number, linked-device QR (behind the flag `ALLTERNIT_WA_PERSONAL=1`)
- A Node sidecar `services/wa-personal/` using **Baileys (MIT)** that runs on the user's runtime (Desktop or cloud computer), not in the cloud.
- It exposes a local HTTP API: start a session → QR string; status; send; and inbound → the local allternit-api `/webhooks/channels/whatsapp-personal`.
- Session creds are stored encrypted on the device.
- Defaults: owner-only DMs (others need pairing approval) and mention required in groups.
- The API surfaces a red warning text: unofficial, risk of a ban, a dedicated number recommended.

### 3. allternit-api, new `channel_whatsapp_app.rs`
- A business transport that sends through the cloud.
- A personal transport that sends through the sidecar.
- Each message is prefixed with the bot's name in bold, because one number has one display name.

## Tests
- the code exchange and subscribed_apps body (fake Graph);
- the verify handshake;
- the 24h-window refusal;
- the personal sidecar's API contract, with Baileys mocked;
- the speaker prefix.

## Notes
`docs/WHATSAPP_NOTES.md`, listing the Meta prerequisites Eoj must finish. Then the sentinel.
