# Unified channel API: live provider tests (next session)

Everything in `UNIFIED_CHANNEL_API.md` was verified offline against fake platforms (173 runtime channel tests, including the contract tests across 9 adapters). These live runs are still to do. They send real messages, so each one needs Eoj's explicit OK, a named test account, and a named destination (a test channel or chat, never a customer's). Record the evidence (time, binding id, correlation ids, screenshots) per provider in a dated section at the bottom of this file.

## Before starting

- Desktop runtime published from a main that contains platform #1524 (check `runtime.allternit.com` version against the merge commit). ai.allternit.com and the PWA should carry ai #543.
- One test bot switched on for each test connection. Sender policy: "Only me and people I add", with the tester listed.
- Note which providers are live on this deployment (`GET /api/v1/channels/availability`). Skip any that say "Coming soon" and say so in the record.

## Per provider (Telegram, Slack shared app, Discord app, Teams app, WhatsApp Business, SMS; WhatsApp personal only if enabled)

1. **Round trip.** Message the bot from the test account. The thread opens and the bot answers in the chat. `GET /gateway/threads/{id}/channel` shows the provider, `online: true` and the capabilities from the table in `channel-gateway-api.mdx`.
2. **Idempotent send.** `POST …/channel-send` twice with the same `correlationId`: one message in the chat, and the second response has `replay: true`. `GET …/channel-messages/{correlationId}` shows `confirmed` with a `remoteId`.
3. **Receipts.** WhatsApp only: the delivered/read receipt moves `delivery` and the UI shows Delivered/Read. Everywhere else the UI shows "Sent", never Read.
4. **Edits, deletes, reactions** (where the capabilities say so): each appears as a thread event and starts no bot turn.
5. **Files** (where `filesOut` / `filesIn`): a file sent each way shows in the thread and in the chat.
6. **Isolation.** Two owners' bots (or two bots of one owner on different connections), and the same person messaging both, give two separate threads. Most important on Telegram.
7. **Restart mid-turn.** Send a message that makes the bot work for a while, then quit Desktop mid-answer and relaunch. The chat gets the "interrupted by a restart" note, the turn is not run twice, and the inbound row's `detail_json.turn.state` is `interrupted`.
8. **Restart before the turn starts.** Quit Desktop and send a message while it's down (the cloud queue holds it for the relayed providers). After relaunch it is answered once.
9. **Rate limit** (only if it can be triggered safely in a test chat): the UI shows "Rate limited, not sent", `retryAfterSecs` matches the platform's wait, and a resend with the same correlation id goes out once.

## Not testable live without risk

- Crash between recording a send and the platform's answer: covered offline (`sends_cut_off_by_a_restart_become_unconfirmed_once`). Don't force it against a real platform.
