# Subscription Gateway

Local-only daemon that turns paid consumer AI subscriptions into addressable
capabilities for Allternit bots and surfaces. Bots request capabilities; the
fabric chooses entitlements; adapters execute them.

## State

All state lives under `~/.allternit/subscriptions/` (override with
`SUBS_GATEWAY_STATE_DIR`):

- `state.db` — SQLite (WAL) task/event/account/token store
- `artifacts/<sha256[0:2]>/<sha256>` — content-addressed, quarantined artifacts
- `policy.yaml` — optional flat-key policy file

## Transport

Default transport is a Unix domain socket at
`~/.allternit/subscriptions/gateway.sock` (mode 0600), always token-authed.
On Windows it is a named pipe, `\\.\pipe\allternit-subs-gateway-<hash>`,
keyed to the state dir (`gatewaySocketPathFor` in `src/config.ts`).
TCP `127.0.0.1:7788` is off unless `SUBS_GATEWAY_TCP=1` and still always
requires a scoped bearer token.

## Secret store (D3/D15)

The daemon refuses to start without a writable local secret store: caller
tokens and the at-rest master key live there, never in the database. Backend
selection is `SUBS_GATEWAY_KEYCHAIN=file|keychain` (default `keychain`):

- `keychain` — macOS Keychain, service `com.allternit.subscription-gateway`.
  The default; with this backend boot still refuses anywhere the Keychain is
  absent.
- `file` — the keychain-equivalent store for "Allternit Sessions" machines
  (D15: Linux/Windows guests have no macOS Keychain): a 0600 `keychain.json`
  under the state dir, atomic writes. **Honest caveat:** values are plaintext
  at rest, protected by filesystem permissions only — encrypt-at-rest via the
  master key (§A6.3) is a follow-up, not shipped yet.

Placement (D3-refined, HARDENING D15 — supersedes the original "never cloud"
reading): the gateway runs single-tenant on an Allternit Sessions machine —
T1 Hosted (the user's Allternit cloud allotment), T2 BYOC, or T3
Local-contained — never multi-tenant, never uncontained in a daily-driver
desktop session.

## CLI authentication

At boot the gateway ensures a `cli` caller token exists: if the `cli-token`
entry in the configured secret store is missing or no longer verifies against
the tokens table, a fresh token is issued and stored. The `allternit` CLI
authenticates with `SUBS_GATEWAY_TOKEN` if set (use this on Sessions
machines), else reads the macOS Keychain item via
`security find-generic-password -s com.allternit.subscription-gateway -a cli-token -w`.
With the `file` backend the entry lives in `<stateDir>/keychain.json`.

## Adapter registry

At boot, `adapters/*/manifest.yaml` are loaded, validated against the
contracts `AdapterManifest` schema (invalid manifest = boot failure), and
exposed as the live `GET /v1/capabilities` view. Override the directory with
`SUBS_GATEWAY_ADAPTERS_DIR` (tests).

## Memory Drive reads

`GET /v1/accounts/:id/memory`, `GET /v1/accounts/:id/threads?limit=N` (1–100) and
`GET /v1/accounts/:id/threads/:threadId` (scope `accounts:manage`) let allternit-api keep the
owner's Memory Drive current (`cmd/allternit-api/src/vendor_memory.rs`). They call the adapter's
optional `readMemory`, `listThreads` and `readThreadText` (contracts `SubscriptionAdapter`) through
`WorkerPool.readForMemory`. These are read-only and non-spending: they never type, click or send,
never navigate the page, and answer `409 busy` while the account has an active task. A login that
isn't ready answers `409 not_ready` with its `session_health`. An adapter without a reader answers
`501`. ChatGPT and Claude implement them with the provider's own same-origin JSON, called inside the
signed-in page (`adapters/_shared/vendor-memory.ts` turns it into plain text; the bearer never
leaves the page).

## Route router (deterministic app routing)

One deterministic choice of HOW a signed-in vendor app is driven, per the
approved spec §2–§6 (this ships P0 + the Settings API). Fixed route order:
`android-app` → `iphone-app-ictrl` → `desktop-app-cdp` → `desktop-app-ax` →
`web` → `iphone-app-usb-agent` (phones first — day-one setup with apps the
user is already signed into; vision routes last — slowest). The choice is
bound, not raced: it depends only on (vendor, eligible routes, order, user
override), and every choice is logged with its reason.

- Routes + fixed order + adapter/phone mapping: `src/routes/registry.ts`.
  `*-web` manifests (interface `ui_bridge_web`) map to `web`; AAI adapters on
  a local app runtime (e.g. claude-desktop) declare both desktop lanes; AAI
  `official`/`local` lanes are APIs, not app routes. Phone device shapes
  mirror Desktop's real `phone.devices` response (`link:<id>` serials,
  `route: 'iphone-app-ictrl' | 'android-app'`, `health.state`).
- §3 one contract, no fall-off: the 7 capability items live in
  `src/routes/contract.ts`, derived from the existing
  `POST /aai/conformance/:adapterId` suite (extended per route: results are
  recorded per `(vendor, route, app_version)`, migration `0012`; `?route=<id>`
  targets one route). An app update stops matching the old rows — the route is
  ineligible for the new version until the suite re-runs. A route missing any
  item is not offered; it never quietly does less.
- Selection: the pure `selectRoute` (`src/routes/select.ts`) — same inputs →
  same output; override honoured only when eligible; never a weaker fallback.
- Binding + failover: `RouteDirector` (`src/routes/director.ts`) writes the
  chosen route into the vendor's binding (`route_bindings`); the AAI host
  consults it only at turn start (`agent.context.open` / `agent.context.
  message` on vendor bindings), which is what makes failover strictly
  between-turn — a turn already streaming never switches. With no eligible
  route the turn fails typed: `NO_ROUTE` with a plain reason ("phone offline,
  no other signed-in route"); through the AAI surface that is `LANE_BLOCKED`
  with `details.no_route` and the reason preserved.
- §2 debug-mode rule: `desktop-app-cdp` is used only when the shipped app
  accepts its debug flag unmodified. We never patch, re-sign or inject into a
  vendor binary; a launch-time probe (`route_probes`, written at boot from
  `SUBS_GATEWAY_<ADAPTER>_TRANSPORT`, the convention claude-desktop already
  uses) records which desktop lane applies per app version — otherwise the
  app uses `desktop-app-ax`, or the phone.
- Settings API (another task builds the UI against exactly this):
  `GET /v1/routes` and `PUT /v1/routes/:vendor/override` (`{ route | null }`,
  400 when the route is not eligible) in `src/http/routes_routes.ts`, scope
  `accounts:manage` like the rest of Settings.
- Phone routes are registered now but their adapters land in a follow-up
  task: until a phone route has passing conformance it reports
  `eligible: false` with the failing items visible — never assumed.

