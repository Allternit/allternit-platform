# Glossary

**What this is:** every gateway term in one or two plain sentences, with where it lives in code.
**Who it's for:** anyone reading the gateway docs or code for the first time.
**Last verified against:** platform commit `c3af0730ca`.

Paths are relative to the platform repo root unless they start with `allternit-ai`. Index: [README.md](README.md).

| Term | Meaning | Where |
|---|---|---|
| AAI (Allternit Agent Interface) | The one interface every vendor is reached through. 18 operations (`agent.list`, `agent.context.open`, `agent.events`, ...). REST, SDKs, MCP and A2A are facades over it. | `platform/packages/subscription-fabric-contracts/src/agent.ts` (`aaiOperationSchema`) |
| AaiProvider | The TypeScript interface an adapter implements, one method per AAI operation. | `platform/packages/agent-gateway/src/provider.ts` |
| AaiRouter | Binds a provider to an execution binding. Enforces idempotency by correlation id, `maxParallel`, and human-only approval responses. | `platform/packages/agent-gateway/src/router.ts` |
| AaiHost | The subscription gateway's registry of providers. Applies the kill switch and pacing. Serves `/aai/*`. | `services/subscription-gateway/src/aai/registry.ts` |
| AAIError | The error shape and code list every operation uses (`AUTH_REQUIRED`, `RATE_LIMITED`, `LANE_BLOCKED`, ...). | `subscription-fabric-contracts/src/agent.ts` (`aaiErrorCodeSchema`) |
| Adapter | Code that connects one vendor to AAI: manifest, provider, registration, fixtures. | `services/subscription-gateway/adapters/<id>/` ([ADAPTERS.md](ADAPTERS.md)) |
| Agent Context | AAI's name for whatever a vendor calls a thread, chat, session, task or run. Not called a Thread on purpose. | `agent.context.*` operations |
| Bot | The durable Allternit worker identity. A vendor agent enters as an ordinary Bot. | existing `agents` table, `is_bot` |
| ExecutionBinding | How a Bot executes: native, or a vendor (mode, adapter, account, lane, capabilities, state). | table `bot_execution_bindings`, `cmd/allternit-api/src/agent_gateway_routes.rs` |
| ProviderAccountBinding | A connected vendor or channel account, holding a reference to a sealed secret or local session. | table `provider_account_bindings` |
| Thread | The one unit of work, owned by a Bot. Allternit owns it, the vendor owns the execution context. | table `bot_threads` (V186), `thread_routes.rs` |
| Generation | One runtime context of a Thread. A Thread hands off between generations by checkpoint. | `thread_routes.rs` |
| RemoteThreadBinding | Links one Thread generation to one vendor context. `lane` and capability snapshot are frozen at open. | table `remote_thread_bindings` |
| Mode | Who holds authority: `hosted`, `linked`, `mirror` (`native` for Allternit bots). Never "Twin". | `bindings.ts` |
| Lane | How the vendor is reached: `official` (API), `channel`, `ui_bridge` (driving the app), `local`. | `agent.ts` (`laneSchema`) |
| Guarantee | What a lane promises: `exact`, `best_effort`, `read_only`. Events are `exact`, `best_effort` or `inferred`. | `agent.ts` |
| Capability manifest | `AgentCapabilityManifest`: what an agent can do on its current lane (context, messaging, memory, approvals, events, runtime). | `agent.ts`, each adapter's `manifest.ts` |
| Transport | The concrete channel to a vendor: official API, CDP, macOS AX, Playwright browser, local HTTP. Also the Rust `AaiTransport` trait that calls the gateway. | [ADAPTERS.md](ADAPTERS.md#transports), `gateway_runner.rs` |
| ui_bridge | A lane that drives the vendor's own web or desktop app. Fragile and terms-sensitive, so it is `best_effort` and shows a warning. | manifests with `lane: "ui_bridge"` |
| Kill switch | Turns a lane off. Per account in the UI (bindings go `DISABLED`), per adapter in the gateway (`LANE_BLOCKED`). | `registry.ts`, `ConnectionCard.tsx` |
| Pacing | Minimum gap and hourly cap on `context.open` and `context.message` per adapter. | `registry.ts` (`AaiPacing`) |
| Correlation id | Idempotency key on a send. A replay returns the first result. | `gateway_sends`, router |
| Conformance | The 13-area test harness every adapter must pass against fixtures. | `platform/packages/agent-gateway/src/conformance.ts` |
| Loopback | An Allternit Bot exposed as an AAI provider, used to prove the contract without a vendor. | `platform/packages/agent-gateway/src/loopback.ts` |
| Event envelope | The fields added to ledger events: bot, thread, generation, source, vendor, lane, remote ids, correlation, guarantee. | [ARCHITECTURE.md](ARCHITECTURE.md#6-event-envelope) |
| Ledger | The existing bot event log `bot_events`, where gateway events land tagged with the thread. | `gateway_runner.rs` (`led`) |
| Approval (two authorities) | `allternit` approval gates our sends, `vendor` approval is the vendor's own prompt. Neither resolves the other, and only a human answers. | table `gateway_approvals` |
| Needs attention / `needs_you` | UI and thread status when a binding needs auth, a lane is blocked, or an approval is open. | `gateway_runner.rs` (`fail`) |
| Revocation cascade | An account going `REVOKED` or `EXPIRED` moves its bindings to `NEEDS_AUTH`. Threads survive. | `cascade_needs_auth` |
| Sealed key | A user's own API key encrypted with `token_crypto` (AES-256-GCM). Stored only as `enc:v1:...`. | `seal_strict` |
| Coordinator placement | Capability-aware assignment of plan steps to Bots. No vendor names. | `cmd/allternit-api/src/gateway_placement.rs` |
| Vendor pack | The full description of a vendor: look, connection profiles, terms. | `allternit-ai/src/lib/gateway/vendor-packs.ts` |
| Look pack | The React components and profile that style a vendor's thread. | `allternit-ai/src/lib/gateway/look-packs/` ([LOOK_PACKS.md](LOOK_PACKS.md)) |
| LookProfile | Data-only look description. Has no field for the provenance layer. | `allternit-ai/src/lib/gateway/look-profile.ts` |
| Provenance layer | The bar above a pack that always shows agent, vendor, Verified, mode, lane, guarantee and status. A pack cannot hide it. | `allternit-ai/src/components/gateway/GatewayProvenanceBar.tsx` |
| PackGap | A recorded case where a pack fell back to a generic renderer. Has surface, severity (`visual_parity`, `functional`, `data_loss`) and status. | table `vendor_pack_gaps` |
| Parity | `full` (no open gaps), `partial` (only visual gaps), `blocked` (any data-loss or functional gap). | `agent_gateway_routes.rs` |
| Mirror | A mode where Allternit keeps a copy of observable vendor state, synced per field. | `platform/packages/agent-gateway/src/mirror.ts` |
| MirrorFieldState | Per-field sync result: `synced`, `partial`, `stale`, `conflict`, `unobservable`. | `mirror.ts` |
| Vendor memory | A separate memory partition under the Bot. Promoted into native memory only by an explicit action. UI merged, backend in review. | `allternit-ai/src/components/gateway/VendorMemoryPartition.tsx` |
| Channel pack | Slack, Teams, Discord and WhatsApp conversations as Threads. | [CHANNELS.md](CHANNELS.md), `channel_gateway.rs` |
| ChannelTransport | The Rust trait every channel platform implements (verify, normalize, post, identity, fetch_since). | `channel_gateway.rs` |
| ChannelConversationBinding | Links one Thread to one external conversation. | table `channel_conversation_bindings` |
| Muse lane | Meta Muse reached over WhatsApp as a `channel` lane. | `channel_transports.rs` (`ChannelLaneTransport`) |
| Facade | An outward API over AAI: REST, TS and Python SDKs, MCP server tools, A2A. | [FACADES.md](FACADES.md), `aai_facade.rs` |
| Sessions computer | The separate computer that hosts the subscription gateway. allternit-api reaches the gateway only through it. | `cmd/allternit-api/src/subscription_routes.rs` |
| subsfab | The subscription fabric: the existing service and lanes the gateway currently runs on. | `services/subscription-gateway`, `subscription-fabric-contracts` |
