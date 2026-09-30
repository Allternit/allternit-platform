# Allternit Agent Gateway

**What this is:** the index and one-page overview of the Agent Gateway and the Allternit Agent Interface (AAI).
**Who it's for:** engineers working on or integrating with the gateway.
**Last verified against:** platform commit `c3af0730ca` (`gateway/integration`).

Intent lives in the spec (`Allternit Brain/Research/specs/agent-gateway.md` and `channel-packs.md`, outside this repo). These docs describe what the merged code does. Where something is unverified live or blocked, the page says so. Nothing here is a plan.

## Overview

A vendor's bot (an OpenAI dot, a Grok Bot, a Claude Managed Agent, an OpenClaw) can be added to Allternit as an ordinary Allternit Bot whose execution is bound to the vendor's runtime. The user works with it in a normal Allternit Thread. Every vendor is reached through one interface, AAI, so the rest of Allternit sees one contract instead of one per vendor.

Two directions work today, both server-side:

- **In:** a Thread turn on a vendor-bound Bot goes to `allternit-api`, which calls the subscription gateway's `POST /aai/call`, which routes to a vendor adapter. Events and replies come back into the thread ledger and transcript.
- **Out:** other agents reach an Allternit Bot through AAI facades: REST, TypeScript and Python SDKs, MCP tools, and an A2A card plus task endpoint.

Channels (Slack, Teams, Discord, WhatsApp) use the same tables and event ledger. A thread can be bound to one external conversation.

## Locked architecture statement

These are decisions from the spec, locked 2026-09-29. The full log is in [ARCHITECTURE.md](ARCHITECTURE.md#12-decisions-log).

- The Agent Gateway and AAI are the long-term product, separate from subsfab. Today they use subsfab's lanes underneath. If vendors open official APIs, lanes change and AAI does not.
- Bots stay the one worker model. There is no VendorAgent, VendorTask or VendorSession object and no second thread hierarchy.
- Threads (`bot_threads`) stay the one unit of work. Allternit owns the work, the vendor owns the execution context.
- The Coordinator never talks to vendors. It asks generic questions: capability, availability, capacity, policy.
- Modes are Hosted, Linked and Mirror. Never call Mirror "Twin".
- Mirror sync is per field. A field we cannot observe is never claimed synced.
- Look packs are authentic inside the vendor thread, and a provenance layer above them cannot be hidden by the pack.
- Approvals have two authorities, Allternit and vendor. One never resolves the other, and a bot never answers an approval.

## Documents

| Doc | Read it for |
|---|---|
| [ARCHITECTURE.md](ARCHITECTURE.md) | Object model, lanes and guarantees, file map, vendor turn flow, event envelope, approvals, failure table, Mirror, placement, decisions log |
| [ADAPTERS.md](ADAPTERS.md) | Writing an adapter, transports, credentials, status of every shipped adapter |
| [LOOK_PACKS.md](LOOK_PACKS.md) | Vendor look packs, the provenance layer, PackGap and parity rules |
| [CONNECTIONS_AND_CREDENTIALS.md](CONNECTIONS_AND_CREDENTIALS.md) | Connection state machine, auth types, account bindings, sealed keys, kill switch, revocation |
| [CHANNELS.md](CHANNELS.md) | ChannelTransport, Slack/Teams/Discord/WhatsApp, channel send policy, Muse lane |
| [OPERATIONS.md](OPERATIONS.md) | Env vars, migrations, the `gateway/integration` gate, known test issues, live-verification checklist |
| [FACADES.md](FACADES.md) | REST, SDKs, MCP tools, A2A, with examples |
| [QUICKSTART.md](QUICKSTART.md) | **Start here.** Run it locally, connect Grok Bot, bind a bot, send a turn, approve |
| [GLOSSARY.md](GLOSSARY.md) | Every term in one or two sentences, with the code location |
| [AAI_REST.md](AAI_REST.md) | Route-by-route REST reference |
| [ACCEPTANCE.md](ACCEPTANCE.md) | Acceptance matrix with test names and status |

The web repo (`allternit-ai`) has the UI side: `docs/gateway-ui.md`, `docs/gateway-look-packs.md`, `docs/gateway-acceptance.md`.

## Status in one paragraph

Contracts, persistence, thread runner, event and approval bridge, placement, facades and channel transports are merged and covered by offline tests. The Rust tests were not run while writing these docs. Live verification exists for one adapter only: Grok Bot (2026-09-29, Grok Bot 0.61.0, over CDP with consent). Every other vendor adapter is verified offline against fixtures or fake servers. Real Slack and Teams accounts, a real Managed Agents key, a real dots session and a real OpenClaw have not been exercised. See the status table in [ADAPTERS.md](ADAPTERS.md#shipped-adapters).
