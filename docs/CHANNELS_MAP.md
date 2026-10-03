# Channels program map (orchestrator: joe-07; plan approved by Eoj 2026-10-02)

Full plan: `/Users/joe/Desktop/Allternit/Allternit Brain/Research/specs/channel-packs.md`, sections v3, v3.1 and v3.2. Plan page: https://claude.ai/artifact/4fdoE9GWqiJhrTK8ptzzyF. CommRails: dag_889022 / wih_3533.

**Goal:** every Allternit bot can be reached on the chat apps (Telegram, Discord, Slack, Teams, WhatsApp), on email, and on a real phone number (SMS + calls). It answers there, and each conversation is a thread in Allternit. Setup is a clean in-app wizard with no pasted secrets by default. The design is for production: the cloud relay `api.allternit.com/channels/in/<key>` (cmd/allternit-cloud-api/src/routes/channel_inbound.rs) delivers to each user's runtime, either a cloud computer (woken if it is asleep) or a Desktop.

**Already merged:**
- platform #1189: messaging connectors, V211 channel_account_bots, member bots, @mention sub-threads.
- platform #1192: vendor `payload.reply` read; switched-off-bot notice.
- ai #361: Messaging section and per-bot switches.
- ai #365: Turn on card.

**Waves:**

| # | Workstream | Repo | State |
|---|---|---|---|
| 1 | Bot email that replies (this doc's sibling task) | platform | **wave 1** |
| 2 | Telegram Managed Bots (one-tap bot creation, deep-link pairing) | platform (allternit-api + cloud-api) | wave 1, separate executor |
| 3 | Connect wizard UI framework + Telegram/email wizards | allternit-ai | after 2's API |
| 4 | Phone numbers: Telnyx, SMS via relay, `channel_phone.rs`, consent gate, call/SMS UI | platform + ai | waits on Eoj's Telnyx account; voice engine is joe-9e's program |
| 5 | Discord shared app (cloud gateway) | platform | later |
| 6 | Slack shared app | platform | later |
| 7 | Teams (two-tenant spike first) | platform + ai | later |
| 8 | WhatsApp (Embedded Signup + personal QR) | platform + ai | waits on Meta verification |

**Do not touch:**
- joe-9e's voice files: `cmd/allternit-api/src/voice_calls.rs`, `services/voice/**`, any LiveKit deploy.
- The other wave-1 executor's files, unless your task names them.
