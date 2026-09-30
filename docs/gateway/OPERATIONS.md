# Operations

**What this is:** the environment variables, migrations, merge gate, test caveats and live-verification checklist for the gateway.
**Who it's for:** whoever deploys, merges or live-tests the gateway.
**Last verified against:** platform commit `c3af0730ca`. Env vars were taken from `grep` over the gateway sources, not from a running deployment.

Index: [README.md](README.md). Setup for a dev machine: [QUICKSTART.md](QUICKSTART.md).

## Environment variables

### subscription-gateway (`services/subscription-gateway`)

| Variable | Default | Effect | Source |
|---|---|---|---|
| `SUBS_GATEWAY_STATE_DIR` | `~/.allternit/subscriptions/` | State, `state.db`, `gateway.sock`, `keychain.json`, `policy.yaml` | `src/config.ts` |
| `SUBS_GATEWAY_ADAPTERS_DIR` | built-in adapters dir | Where `adapters/*/aai.ts` are loaded from | `src/config.ts` |
| `SUBS_GATEWAY_KEYCHAIN` | `keychain` | `file` or `keychain`; anything else throws | `src/config.ts` |
| `SUBS_GATEWAY_TCP`, `_TCP_HOST`, `_TCP_PORT` | off, `127.0.0.1`, `7788` | TCP listener (token required). Unix socket is always on. | `src/config.ts` |
| `SUBS_GATEWAY_TOKEN` | issued at first boot | Caller bearer token override for the CLI | `src/security/tokens.ts` |
| `SUBS_GATEWAY_API_BASE` | `http://127.0.0.1:18013` | allternit-api base for the loopback provider | `src/config.ts` |
| `SUBS_GATEWAY_LOGIN_BROWSER` | detected Firefox | Browser for login flows | `src/config.ts` |
| `SUBS_GATEWAY_IMAGE_PROJECT`, `_IMAGE_CHAT_MAX` | `Allternit`, `20` | chatgpt-web image chats | `src/config.ts` |
| `SUBS_GATEWAY_AAI_DISABLED` | empty | Comma-separated adapter ids started with the kill switch on | `src/config.ts` |
| `SUBS_GATEWAY_AAI_LOOPBACK_BASE` | `${API_BASE}/api/v1` | Loopback provider base URL | `src/config.ts` |
| `SUBS_GATEWAY_AAI_LOOPBACK_BOTS` | empty | Comma-separated bot ids the loopback exposes | `src/config.ts` |
| `SUBS_GATEWAY_AAI_LOOPBACK_TOKEN` | none | Bearer for the loopback calls to allternit-api | `src/aai/registry.ts` |

### Adapters

| Variable | Adapter | Effect |
|---|---|---|
| `SUBS_GATEWAY_GROK_BOT_CDP_PORT` | grok-bot | CDP port, default 9222 |
| `SUBS_GATEWAY_CLAUDE_DESKTOP_CDP_PORT` | claude-desktop | CDP port |
| `SUBS_GATEWAY_CLAUDE_DESKTOP_TRANSPORT` | claude-desktop | `ax` to use macOS Accessibility instead of CDP (default CDP) |
| `SUBS_GATEWAY_CLAUDE_AX_CONSENT` | claude-desktop | `1` allows the AX bridge to attach. Without it every call is `LANE_BLOCKED`. |
| `SUBS_GATEWAY_AX_BRIDGE_BIN` | claude-desktop, chatgpt-dots | Path to the built `native/ax-bridge` helper |
| `SUBS_GATEWAY_DOTS_TRANSPORT` | chatgpt-dots | `chatgpt-app` for the native ChatGPT app over AX (default Playwright browser) |
| `SUBS_GATEWAY_CHATGPT_APP_AX_CONSENT` | chatgpt-dots | `1` allows AX attach to ChatGPT.app |
| `SUBS_GATEWAY_DOTS_CONSENT` | chatgpt-dots | `1` allows launching Chrome for dots |
| `SUBS_GATEWAY_DOTS_PROFILE_DIR` | chatgpt-dots | Absolute Chrome user-data dir. Required with consent. |
| `SUBS_GATEWAY_DOTS_DEFAULT_DOT` | chatgpt-dots | Default dot to open |
| `SUBS_GATEWAY_CLAUDE_MA_ENVIRONMENT_ID` | claude-managed-agents | Managed Agents environment id |
| `SUBS_GATEWAY_CLAUDE_MA_BASE_URL` | claude-managed-agents | API base override |
| `SUBS_GATEWAY_CLAUDE_MA_MAX_PARALLEL` | claude-managed-agents | Declared `maxParallel` |
| `SUBS_GATEWAY_CLAUDE_MA_ARCHIVE_ON_CLOSE` | claude-managed-agents | Archive sessions on close |
| `SUBS_GATEWAY_CLAUDE_MA_MEMORY_STORE_IDS`, `_VAULT_IDS` | claude-managed-agents | Memory stores and vaults to attach |
| `SUBS_GATEWAY_OPENCLAW_URL`, `_AGENT`, `_TOKEN`, `_MAX_PARALLEL` | openclaw | Endpoint (default `http://127.0.0.1:18789`), agent id, bearer, declared parallelism |

Defaults for the Claude Managed Agents and OpenClaw variables are in each adapter's `aai.ts` and `manifest.ts`.

### allternit-api

| Variable | Effect | Source |
|---|---|---|
| `ALLTERNIT_API_PORT` | Listen port. Dev default 18013, production pins 8013. | `main.rs` |
| `ALLTERNIT_ENCRYPTION_KEY` (fallback `ENCRYPTION_KEY`, then the file at `ALLTERNIT_PLATFORM_KEY_FILE`) | Key for sealing user API keys. Without one, `POST .../secret` is 503. | `token_crypto.rs` |
| `ALLTERNIT_SLACK_BOT_TOKEN` | Slack `chat.postMessage` token. Unset means inbound works, replies do not post. | `channel_gateway.rs` |
| `ALLTERNIT_SLACK_BOT_USER_ID` | Slack user id of our app, to recognize our own echoes | `channel_gateway.rs` |
| `ALLTERNIT_DISCORD_GATEWAY` | `0` disables the Discord websocket client | `discord_gateway.rs` |

Teams, Discord and WhatsApp secrets are not env vars. They are sealed JSON on the provider account ([CHANNELS.md](CHANNELS.md#providers)). There is no gateway feature flag in allternit-api: the runner engages when a bot has a `type=vendor` execution binding, and the gateway routes are always mounted.

## Migrations

`V198__agent_gateway_bindings.sql`, `V199__gateway_runner.sql` and `V200__channel_message_log.sql` in `cmd/allternit-api/migrations/`. All are additive (`CREATE TABLE IF NOT EXISTS` and indexes; no existing table is altered). Refinery applies them when allternit-api starts. On production they apply on the first start after the Contabo deploy. Prod migrations are otherwise run manually as postgres for other services, so confirm the target is the allternit-api SQLite path and not a manual step.

| Migration | Tables |
|---|---|
| V198 | `provider_account_bindings`, `bot_execution_bindings`, `remote_thread_bindings`, `channel_conversation_bindings`, `vendor_pack_registry`, `vendor_pack_gaps`, `connection_audit` |
| V199 | `gateway_approvals`, `gateway_sends` |
| V200 | `channel_message_log` |

## The `gateway/integration` gate

Gateway work merges into `gateway/integration`, not `main`, in both repos. Each feature branch is a PR into it. `gateway/integration` goes to `main` as a whole after the live checks below pass for the adapters being enabled, and after the platform deploy that applies V198 to V200. The reason: the code is offline-verified only, migrations create tables in production, and the UI ships in Desktop builds. Landing it piecemeal on `main` would put half a stack in front of users. This docs PR targets `gateway/integration`. Do not merge `gateway/integration` to `main` from these docs.

## Build and repo rules

- **Shared `CARGO_TARGET_DIR`:** the workspace `.cargo/config.toml` sets `target-dir = "target"` locally. Several worktrees building at once is slow and can fill the disk. If you share a target dir across worktrees, export the same `CARGO_TARGET_DIR` for all of them, and do not run two cargo builds against it at once.
- **Lockfile rule:** after changing a package's dependencies, run `pnpm install --lockfile-only --filter <pkg>` and commit the lockfile change with the package change.
- Build, run tests and typecheck only when authorized for the task.

## Known test caveats

- The Rust gateway tests (`gateway_runner`, `gateway_placement`, `channel_*`, `aai_facade`, `a2a_routes`) were not run while writing these docs. `ACCEPTANCE.md` marks them `verified (automated, Rust: not run)`.
- The Rust tests set `ALLTERNIT_ENCRYPTION_KEY` with `std::env::set_var`, which is process-wide. Run them in a way that tolerates that.
- I have no information on other local test-environment problems from this pass. Add them here when found.

## Live-verification checklist

Nothing below has run except where marked. Each step needs the named person's consent, because it touches a real account or app.

| # | Check | Who must consent | Status |
|---|---|---|---|
| 1 | Grok Bot over CDP: quit, relaunch with debug port, send one message, restore the app | Eoj (his Grok Bot app) | Done 2026-09-29 (Grok Bot 0.61.0). Still unverified live: Stop button, approval card, rate-limit and bot-check banners, routines, logged-out gate. |
| 2 | Same through allternit-api: bind a Sessions computer, connect account, bind bot, send a turn, see it in the thread, answer a vendor approval | Eoj | Not run |
| 3 | Claude Managed Agents with a real API key and environment | Eoj (his Anthropic key, billed to him) | Not run |
| 4 | chatgpt-dots: own Chrome profile or ChatGPT.app over AX; confirm `/dots` routes and selectors | Eoj (his ChatGPT account, terms risk on a UI bridge) | Not run |
| 5 | claude-desktop: attach to Claude desktop over CDP or AX; the CDP route is probably vendor-blocked | Eoj | Not run |
| 6 | OpenClaw: a real gateway, confirm conversation keying | Whoever runs the OpenClaw | Not run |
| 7 | Slack: a real workspace, event to thread in under 3 s, reply in the channel thread, reconnect at cursor | The workspace admin | Not run |
| 8 | Teams: real tenant and bot registration, live JWKS validation, outbound token | The tenant admin | Not run |
| 9 | Discord: real bot with the Message Content intent, websocket session, interactions signature | The server owner | Not run |
| 10 | WhatsApp and Muse: Business account and Meta app, webhook handshake and signature, a Muse conversation | The account holder | Not run |
| 11 | Vendor memory promote flow, after `gateway/accept-rust` merges | Eoj | Blocked on backend PR |
| 12 | Look pack gaps: run a real thread per pack and read `GET /gateway/vendor-packs/:vendor/parity` | Eoj | Only Grok Bot has real traffic |
| 13 | Migrations V198 to V200 on the production allternit-api after the Contabo deploy | Eoj (deploy owner) | Not run |

For a UI-bridge lane, never retry through a bot check, never solve a challenge, and stop if the vendor shows an account-risk warning. The lane is latched and disabled by design.
