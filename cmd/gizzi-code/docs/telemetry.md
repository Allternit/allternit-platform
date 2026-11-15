# Gizzi Code — Telemetry & Observability

This document describes what the `gizzi` CLI collects, what it never
collects, and how to turn it off. It covers the telemetry/observability
code inherited from upstream plus the fork's own runtime telemetry.

**Contact for privacy questions or deletion requests: privacy@allternit.com**

## TL;DR — how to disable everything

```bash
gizzi config telemetry off     # persistent, written to telemetry.json
GIZZI_TELEMETRY=off gizzi      # one-shot environment kill switch
```

Also honored (most restrictive signal wins): `GIZZI_DISABLE_TELEMETRY=1`,
`DISABLE_TELEMETRY=1`, `GIZZI_DISABLE_NONESSENTIAL_TRAFFIC=1` (disables all
nonessential network traffic, not just telemetry), `DO_NOT_TRACK=1`.

Check the current state any time with `gizzi config telemetry status`.

## What is collected (field level)

All telemetry is **anonymous usage statistics** for product health. Prompts,
conversations, file contents, and credentials are never collected (see below).

### 1. First-party event logging (enabled by default)

Sink: OpenTelemetry log exporter POSTing to
`https://api.allternit.com/api/event_logging/batch` (Allternit-owned; source
of truth: `src/shared/constants/cloudUrls.ts`). Batches are retried to disk
(`<config home>/telemetry/`) on failure and re-sent later; failed batches are
dropped after 8 attempts.

Fields per event (`tengu_*` event names, allow-listed in
`src/runtime/services/analytics/datadog.ts` and emitted from
`src/screens/REPL.tsx`, `src/runtime/services/api/logging.ts`, etc.):

- Event name, random `event_id` (UUID), client timestamp, session ID (random
  per-session UUID), device ID (random 64-hex user ID persisted locally).
- Model name (canonicalized; non-internal builds are bucketed to a short name
  or `other`), beta flags in use, entrypoint, client type, interactive flag.
- Environment metadata: platform, arch, node version, terminal *type* (e.g.
  `xterm-256color`), package managers present, runtimes present, CI flags,
  version/build info, VCS in use, WSL/Linux distro info when on Linux.
- Process metrics (memory/CPU gauges, base64-wrapped).
- `user_type`, subscription tier, swarm/teammate agent IDs and team names
  (when running as part of a team), GitHub Actions metadata (actor/repo
  numeric IDs) in CI.
- Repo remote hash (`rh`): a **hash** of the git remote URL used for
  server-side join — the URL itself is not sent.
- For OAuth-logged-in users: `account_uuid`/`organization_uuid`, and email
  (sent only to the Allternit API, never to third parties).
- Per-event numeric/boolean metadata only (tool success/failure, HTTP status
  ranges, durations, counters). Tool inputs are only included when
  `OTEL_LOG_TOOL_DETAILS=1` is explicitly set, and even then are truncated
  (4 KB cap, 128-char string cap, depth-2 nesting cap).
- PII-tagged fields (`_PROTO_skill_name`, `_PROTO_plugin_name`,
  `_PROTO_marketplace_name`): plugin/skill *names* from the public registry,
  routed to restricted columns, stripped from every other sink.

### 2. Usage metrics (BigQuery metrics exporter)

Sink: `https://api.allternit.com/api/gizzi/metrics` (Allternit-owned). Metric
name, numeric value, timestamp, and the same environment attribute set as
above. No content strings.

### 3. Feature flags (GrowthBook)

Sink: `https://api.allternit.com/` with Allternit-owned GrowthBook client
keys. This is a functional fetch (it decides which features are on), not
analytics. Sent with the fetch: device ID, session ID, platform, user type,
subscription tier, account/org UUIDs, and email for OAuth users. Experiment
exposure events go to sink #1 and respect the same opt-outs.

### 4. Runtime telemetry (fork-local)

`src/runtime/telemetry` — structured counters/events used by `gizzi`'s own
logging (`--print-logs`). **Stays on the machine**; records are written to
the local log file only. Strings are redacted (emails, URLs, JWTs, API-key
shapes, absolute paths → `<REDACTED:…>`) and capped at 256 chars before they
are recorded.

### 5. Customer OTLP telemetry (opt-in only)

Standard OpenTelemetry (`OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_METRICS_EXPORTER`,
etc.). Nothing is exported unless you configure it yourself. The endpoint and
headers are entirely under your control.

### 6. Usage telemetry for self-hosted/BYOC operators (opt-in only, default OFF)

Sink: `POST {ALLTERNIT_API_URL}/api/v1/analytics/gizzi-code/events` on the
operator's own allternit-api (backend: `cmd/allternit-api` —
`gizzi_code_usage_events` table). Client:
`src/runtime/services/telemetry/gizziUsageTelemetry.ts`.

**Default: OFF.** Enable per session with:

```bash
GIZZI_TELEMETRY=1 gizzi
```

Note the shared name: `GIZZI_TELEMETRY` is also the upstream kill switch
(`=off` disables everything, see the consent model below). This client turns
on only when the value is explicitly truthy (`1`/`true`/`yes`/`on`) AND the
global privacy level permits telemetry — every opt-out in the table below
(`GIZZI_TELEMETRY=off`, `GIZZI_DISABLE_TELEMETRY=1`, `gizzi config telemetry
off`, …) keeps it silent.

One event per session, flushed fire-and-forget at shutdown (short timeout,
≤1 retry, never blocks exit, never crashes the CLI). Fields: session id,
model(s) used, tokens in/out, cost (when known), tool-call accepted/rejected
counts, and `lines_accepted` — the count of added (`+`) lines in edit/write
patches actually applied to disk (whole-file creations count all lines; pure
deletions count 0). No file paths, no prompt content, no tool arguments —
only what fits the backend schema. Target URL and auth follow
`src/runtime/services/api/allternitApi.ts` (`ALLTERNIT_API_URL`,
`ALLTERNIT_API_TOKEN` bearer, `x-allternit-user-id` for local dev).


## Disabled sinks

### Datadog (upstream vendor account) — disabled by default

The inherited Datadog sink pointed at
`https://http-intake.logs.us5.datadoghq.com` with a hardcoded client token
belonging to the **upstream vendor's** Datadog account — a third party
Allternit users never consented to. There is no Allternit-owned Datadog
endpoint to repoint to (`cloudUrls.ts` lists only
`api/clerk/headscale/install`), so the sink is compiled off by default
(`DATADOG_SINK_ENABLED = false` in both copies of
`src/**/services/analytics/datadog.ts`). Events flow only to the Allternit
first-party sinks above.

### Legacy in-process analytics service — inert

`src/services/analytics/index.ts` is a local publish/subscribe logger with
no registered network handlers; `src/services/analytics/growthbook.ts`
re-exports the Allternit-hosted client and its third-party CDN fallback is
never invoked.

## What is NEVER collected

- Prompt text, conversation content, tool outputs, or file contents.
- Absolute paths or home-directory names — the sink-level sanitizer
  (`src/shared/utils/telemetryRedact.ts`) rewrites paths, emails, URLs with
  credentials, JWTs, and common token shapes (`sk-…`, `ghp_…`, `alt_…`,
  `AKIA…`, `xox…`) to `<REDACTED:…>` on every string that enters a payload,
  as defense in depth on top of the type-level "no strings" metadata rule.
- API keys, OAuth tokens, or auth headers in event bodies. Auth material
  travels only in HTTP headers to the Allternit API (the same host the CLI
  already talks to for auth), never inside event payloads.
- IP addresses are not attached to events client-side; server-side IP
  handling is governed by the Allternit API's own privacy policy.

## First-run disclosure

On the first interactive launch with telemetry enabled, the CLI prints a
one-line notice to stderr telling you telemetry is on and how to disable it.
The notice is shown once; state lives in `telemetry.json`
(`gizzi config telemetry status` shows whether it has been shown).

## Consent model

Telemetry is **on by default** and **opt-out**:

| Mechanism | Scope |
|---|---|
| `GIZZI_TELEMETRY=off` (or `0`/`false`/`no`/`disabled`) | Environment, one process tree |
| `GIZZI_DISABLE_TELEMETRY=1`, `DISABLE_TELEMETRY=1` | Environment (legacy upstream names, still honored) |
| `GIZZI_DISABLE_NONESSENTIAL_TRAFFIC=1` | Environment, also disables auto-updates, release notes, etc. |
| `DO_NOT_TRACK=1` | Environment (RuntimeTelemetry) |
| `ALLTERNIT_SELF_HOSTED=1` | Environment. Self-hosted mode: everything above, plus no calls at all to `*.allternit.com` / `*.gizziio.com` (see `docs/SELFHOST.md`) |
| `gizzi config telemetry off` | Persistent (`telemetry.json` under `~/.config/gizzi-code/` or `$XDG_CONFIG_HOME/gizzi-code/`) |

The kill switch is evaluated at the earliest init point
(`src/shared/utils/privacyLevel.ts`, imported by the analytics config before
any sink attaches), so nothing is emitted before the opt-out is read. The
check is re-evaluated per event, so `gizzi config telemetry off` takes effect
for every process started after the change.

## Retention

**TBD.** Event retention and deletion procedures on the Allternit API side
are not yet finalized; contact privacy@allternit.com for questions or
deletion requests in the meantime.
