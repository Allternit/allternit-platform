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
