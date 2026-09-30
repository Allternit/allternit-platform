# Subscriptions ⇄ Agent Gateway: one login system (2026-09-30)

Agent: claude (session joe-bb, subsfab). Approved by Eoj in-session ("yes lets do this", "go ahead").

## What landed

| PR | Merge | What |
|---|---|---|
| platform #1002 | 6918acef1 | Sessions desktop: `chrome-root` wrapper (`--no-sandbox` as root), Chrome in the dock, Thunar pinned; login Chrome adds `--no-sandbox` as root |
| platform #1007 | 225d1076c | Cloudflare interstitial detected as `challenge_presented` (Turnstile is in a closed shadow root) |
| platform #1012 | a7207458b | Sign-ins finish by themselves: cookies on every origin, localStorage session keys (`auth.session_storage`), already-signed-in probe, re-probe through a security check; Kimi verified live (session on www.kimi.ai) |
| platform #1014 | 7dd2d3953 | Probe waits ≤8 s for SPAs; setup closes orphaned login/adapter Chromes |
| ai #296 / #303 | 80be55cbd / eafc588e4 | Computer screen: Full screen + Fit/Actual size; 16:9 resizable sign-in viewer; View/Open larger close Settings |
| platform #1030 | efa24ba3a | Several logins per subscription (`accounts.preferred`, migration 0007, PATCH /v1/accounts/:id), router prefers it; chatgpt-dots uses the preferred ChatGPT subscription profile |
| ai #306 | e3d273458 | Settings grouping + "Use this one"/Rename; model-picker login switch + "Add another login" + "Manage in Settings"; gateway wizard browser sign-in → SubscriptionAuth (`externalAccountId = subsfab:<id>`); stale usage shows its age |
| platform #1038 | da2f3e185 | Rust `subscription_sync`: linked gateway accounts follow the login's health every 60 s (EXPIRED/REVOKED cascade bots to NEEDS_AUTH) |
| platform #1034 | 0e64600f6 | Public docs page Surfaces → Subscriptions (live) |

## How it works

- One registry of record for bots: `provider_account_bindings`; a subscription-backed account links its Sessions login via `externalAccountId = subsfab:<fabric account id>`.
- One sign-in: the Sessions computer login window + auto-detect, used by Settings, the model picker and the gateway wizard.
- The router uses a subscription's preferred login first and falls back to its other signed-in logins.

## Outstanding (pick up here)

1. **Sessions gateway redeploy** from main (carries #1030). Needs the Desktop app running (relay goes through the local allternit-api): `scratchpad/deploy/redeploy.sh` from session e7759269, or `sessions-setup.sh` flow in HANDOFF-subsfab-2026-09-28-night.md.
2. **Desktop install**: the Agent Gateway session (joe-07) is building from main ce238f696 and asks Eoj directly before installing.
3. **Live checks** (need the app running; keep lean, no loops):
   - One real send each to Kimi and Claude (chat.create, pinned provider, D16 human stamp). Fix `selectors/v1.yaml` from page-shape errors if needed. ChatGPT is at low usage; skip unless asked.
   - When both pass, flip Claude and Kimi from Preview to Available in `surfaces/docs/surfaces/subscriptions.mdx` (docs deploy on merge; Eoj reviews public copy).
   - Settings → Subscriptions: add a second ChatGPT login, switch "Used first", rename; model picker shows the switch.
   - Gateway wizard for OpenAI dots: pick the ChatGPT login → account CONNECTED; then joe-07's step-5 dots live check through it.
   - Sign a subscription login out on the Sessions computer → within ~1 min the linked gateway account goes EXPIRED and its bots NEEDS_AUTH; sign back in → CONNECTED.
4. **Not built**: Claude/Kimi usage readers (Eoj: their settings pages show usage); a Kimi identity read.

## Verified

Gateway suite 488 (+ SDK/contracts), web 133 across touched areas, Rust `subscription_sync` 3/3 + `agent_gateway_routes` 24/24 + `cargo check --bins`; docs `mint validate` + typography. Live: all three subscriptions probed Ready on the Sessions gateway before #1030; Desktop b4299 live-checked (since replaced by another session's build).
