# pally Messages adapter (text/iMessage)

Texts the Pally assistant through the user's own Apple ID / phone number in macOS Messages on the Sessions computer. Route 3 (MESSAGES). Shared transport: adapters/_shared/messages-local.ts.

- Official site: https://pally.com
- Texting handle: none published on the page (its demo number is not a public line); enter your number
- Handle: per-account credential JSON {"handle":"+1..."} wins, then env SUBS_GATEWAY_PALLY_HANDLE, then the published default. Enter with country code.
- Required: SUBS_GATEWAY_MESSAGES_ENABLED=1 (opt-in). macOS only; inert elsewhere.
- Sessions Mac needs: signed in to Messages; Automation permission for the gateway to control Messages; Full Disk Access for the gateway process (read of ~/Library/Messages/chat.db).
- Capabilities: send and reply only, guarantee best_effort, one context per handle, busy lock per handle. Reply = inbound messages from that handle after the send's baseline ROWID, joined; 120 s timeout then a 3 s gather.
- Unsupported: steer, cancel, tools, streaming, memory, computer, artifacts.
- Limit: attribution is by handle and time window, so a manual text to the same handle during a request can be read as the reply.
- Live verification pending: not tested against a real Messages account.
