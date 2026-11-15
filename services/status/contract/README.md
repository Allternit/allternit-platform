# Status contract v1

Files: `status.schema.json` (JSON Schema 2020-12; every payload has a `schema` discriminator and the root validates any one), `types.ts` (same shapes), `fixtures.json` (`operational`, `degraded`, `down`; each holds `status`, `history`, `incidents`), `targets.json` (the checks table).

Status enum: `operational | degraded | down | maintenance | unknown`. Times are UTC ISO 8601. Uptime is a fraction 0..1, `null` = no data.

| Endpoint | Auth | Notes |
|---|---|---|
| `GET /api/v1/status.json` | none | `schema`, `last_checked_at`, `overall`, `components[]`, open `incidents[]` with `updates[]` |
| `GET /api/v1/history.json?days=90` | none | per component per UTC day: `status` + `uptime`, both `null` for no data. `days` 1..400, default 90 |
| `GET /api/v1/incidents.json` | none | open and resolved, newest first |
| `GET /api/health` | none | `{status: ok|stale, last_checked_at, age_seconds}`; 200 when `age_seconds` <= 300, else 503 with `stale`. Used by `surfaces/docs/api/tools.mdx` (http.get example) and launchd `site-monitor` |
| `GET /api/v1/targets.json` | none | the table the vantages probe |
| `GET /feed.xml` | none | Atom feed of incidents |
| `POST /api/v1/ingest` | HMAC | standby vantage posts `IngestPayload` each minute; header `X-Allternit-Signature: sha256=<hex>` over the raw body, shared secret `INGEST_HMAC_SECRET`; reject if `ts` is more than 5 min off |
| `POST /api/admin/incidents`, `PATCH /api/admin/incidents/:id` | `Authorization: Bearer` | `AdminIncidentCreate` / `AdminIncidentPatch`; any manual edit sets `auto=false` |
| `POST /api/admin/maintenance` | `Authorization: Bearer` | `AdminMaintenanceCreate` (`allternit.admin.maintenance.create/v1`): title, component_ids, body, starts_at, ends_at (ISO 8601, end after start and in the future); returns 201 `{id}` |

Cache and CORS, all GETs: `Cache-Control: public, max-age=30`, `Access-Control-Allow-Origin: *`. `/api/health` uses `no-store`. POST/PATCH send no CORS headers.

Clients: fetch `status.json` at most every 5 min and after an API failure, 3 s timeout; show a notice only when an incident touches a component the surface uses; show nothing if the fetch fails.

Notes: `cloud-computers` is checked through cloud-api `GET /api/v1/health/computers` (reachable + healthy). `other` has `max_status: degraded`. No SLA wording; label "measured by our checks, last 90 days".
