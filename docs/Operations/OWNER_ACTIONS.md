# Owner Actions — production & secrets

> **What this is:** the complete, owner-only action list for the API-consolidation
> work (PRs #85/#86/#88, 2026-09-04). Formatted like a secrets-rotation list:
> each item says what, where, how, and how to verify. Agents must NOT execute
> these — credentials, production hosts, and spending decisions are owner-only.
>
> **Last updated:** 2026-09-04 (secrets set; mail 012–014 + 8013 proxy retired; CI still needs a green deploy)

## 1. CI/CD credentials — unblocks cloud-api deploys

| | |
|---|---|
| **Status** | Secrets set 2026-09-04. Remaining: merge `session/ci-oauth` and watch the first green deploy. |
| **What** | Tailscale OAuth + root SSH key for the deploy workflow |
| **Where** | GitHub repo `Allternit/allternit-platform` → Settings → Secrets |
| **How** | Done: `TS_OAUTH_CLIENT_ID` / `TS_OAUTH_CLIENT_SECRET` / `CONTABO_SSH_KEY`. Workflow deploys to `root@45.84.138.187` (tailnet SSH is check-mode and hangs). Optional later: ACL `tag:ci` → `tag:mail` so deploys can leave the public IP. |
| **Verify** | Merge PR #91; `deploy-cloud-api-contabo.yml` test + deploy go green |

## 2. Cloud-api production env (on `mail`)

| | |
|---|---|
| **Status** | Done 2026-09-04 except JWKS verify (needs the new binary). |
| **What** | Apply migrations_pg 012–014 + set the data-plane JWT seed |
| **Where** | `mail`; DB `allternit` (prod Postgres), env `/opt/allternit-cloud-api/.env` |
| **How** | Applied via psql (idempotent). Also set `ALLTERNIT_SKIP_MIGRATIONS=1` — prod `_sqlx_migrations` is the sqlite-derived 1–24 lineage; `sqlx::migrate!("./migrations_pg")` would checksum-fail on boot. |
| **Verify** | health is `{"status":"healthy"}`. After the first new-binary deploy: `curl -s https://api.allternit.com/api/v1/auth/dp-jwks` → `keys[0].kty = "OKP"` (old binary still 401s this route). |

## 2b. Apply cloud migrations 015–017 on `mail` (pending, 2026-09-28)

| | |
|---|---|
| **Status** | **Done** (verified 2026-09-28: `computer_pairing_codes`, `paired_computers`, `webhook_events`, `handoffs` + `eventtype` values all present; `/api/v1/computers/paired` 200). Was found by the live self-pair test: `POST /api/v1/computers/pairing-codes` and `GET /api/v1/computers/paired` return 500 `DATABASE_ERROR` while `/api/v1/mesh/enroll` (same device-token auth) returns 200, so the 017 tables are missing. Prod skips embedded migrations (item 2), so #848's deploy never created them. |
| **What** | `015_webhook_events.sql`, `016_cowork_handoffs.sql` (check whether they were applied), `017_paired_computers.sql` (remote-computer pairing, ACI P4) |
| **Where** | `mail`; DB `allternit`; files in `cmd/allternit-cloud-api/migrations_pg/` |
| **How** | `\dt computer_pairing_codes paired_computers webhook_events cowork_handoffs` to see what's missing, then `psql -d allternit -f <file>` for each missing one (all idempotent: `IF NOT EXISTS`). No restart needed. |
| **Verify** | Signed in on Desktop: Settings → Computers → *Pair a remote computer* shows a code (not "Database error"). |

## 2c. Apply cloud migration 044 on `mail` (pending, 2026-10-04)

| | |
|---|---|
| **Status** | **Done 2026-10-04** (applied as postgres, `GRANT ALL ON photo_avatar_usage TO allternit`). Needed by photo avatars included with a plan (`routes::photo_avatars`). Until it's applied, `GET /api/v1/avatars/photo/allowance` answers 500 and the app hides the Included lane (no paid call is made). |
| **What** | `044_photo_avatar_usage.sql` (the `photo_avatar_usage` ledger) |
| **Where** | `mail`; DB `allternit`; file in `cmd/allternit-cloud-api/migrations_pg/` |
| **How** | `sudo -u postgres psql -d allternit -f 044_photo_avatar_usage.sql` (idempotent: `IF NOT EXISTS`), then `GRANT ALL ON photo_avatar_usage TO allternit;` if the service role doesn't own new tables. No restart needed. Optional: set `ALLTERNIT_AVATAR_MONTHLY_BUDGET_USD` in `/opt/allternit-cloud-api/.env` (default 4.00) and restart cloud-api. The key is the existing `OPENAI_API_KEY`. |
| **Verify** | `SELECT COUNT(*) FROM photo_avatar_usage;` works. Spend this month: `SELECT COALESCE(SUM(cost_estimate_usd),0) FROM photo_avatar_usage WHERE period_start = date_trunc('month', now() at time zone 'utc')::date AND status = 'used';` |

## 2d. Apply cloud migration 064 on `mail` (pending, scheduler claims)

| | |
|---|---|
| **Status** | Pending. Apply **before** the cloud-api build that contains the scheduler-claims PR deploys: its in-process scheduler claims due rows through `schedules.claimed_by/claimed_at`, and every tick errors until those columns exist. |
| **What** | `064_schedule_claims.sql`: adds nullable `claimed_by text`, `claimed_at timestamptz` to `schedules`, plus the partial index `idx_schedules_due`. Additive, no data rewrite. |
| **Where** | `mail`; DB `allternit`; file in `cmd/allternit-cloud-api/migrations_pg/` |
| **How** | `sudo -u postgres psql -d allternit -f 064_schedule_claims.sql` (idempotent: `IF NOT EXISTS`). No restart needed. Only columns and an index, so no new grants. |
| **Verify** | `\d schedules` shows `claimed_by`, `claimed_at`; after deploy the cloud-api log has no `Error during scheduler tick`. |

## 3. Retire the live 8013 nginx proxy on `mail`

| | |
|---|---|
| **Status** | Done 2026-09-04. Backups in `/root/owner-actions-2026-09-04/`. |
| **What** | Remove the interim proxy config deployed by the earlier hardening session |
| **Where** | `mail` `/etc/nginx/sites-enabled/api-allternit` (a copy, not a symlink) + `/etc/nginx/conf.d/allternit-cors-map.conf` |
| **Why** | P1 control-plane handlers replaced it. Leaving it live kept 8013 on the public API hostname |
| **How** | Stripped `/api/jobs`, `/api/v1/agent-sessions`, `/api/v1/office/`, `/api/v1/beta/`, `/api/rails/` 8013 locations; removed cors-map; `nginx -t && reload`. Left `mail.news.allternit.com` → 8013 (company desktop-cloud, not user-facing). |
| **Verify** | `https://api.allternit.com/api/jobs` → 401 `Authorization header with Bearer token required` from cloud-api; health still healthy |

## 4. Verify the dev-token backdoor is closed

| | |
|---|---|
| **Status** | Confirmed 2026-09-04: 401 `Invalid or expired token`. |
| **What** | Confirm the `dev-api-token` bearer no longer authenticates in production |
| **Where** | any shell |
| **How** | `curl -s -o /dev/null -w '%{http_code}\n' -H "Authorization: Bearer dev-api-token" https://api.allternit.com/api/v1/auth/me` |
| **Expect** | **401**. Gate is `ALLTERNIT_ALLOW_DEV_TOKEN`, default OFF; never set it on `mail`. |

## 5. allternit-api repair migrations (VPS 8013 instance)

| | |
|---|---|
| **What** | 7 renumbered migrations (V124–V130) apply on next service start |
| **Where** | `mail`, systemd unit `allternit-api` (or wherever 8013 runs) |
| **How** | Nothing to prepare — just restart/deploy the service with PR #86's code |
| **Verify** | Journal shows 7 migrations applied; fabric offer/lease/pricing columns, desktop audit log, placement canonical fields, node capability JSON now exist. **This is the repair, not a fault** |
| **Caution** | The duplicate-route fix in #86 also removes a startup panic — any binary built between merge `ea89a5fdb` and #86 would crash at router construction; do not roll back past #86 |

## 6. Secrets rotation (from the 2026-09-03 audit — independent of this work)

| Secret | Where found | Action |
|---|---|---|
| TESTING.md password | `surfaces/ai.allternit.com/TESTING.md` (now removed from HEAD — **history still has it**) | Rotate the account; consider BFG/git-filter-repo |
| ProtonMail password + TOTP seed | committed in docs (audit A1) | Rotate password AND re-seed TOTP |
| Clerk `sk_test` key | `cmd/gizzi-code/script/platform-auth-server.js` (now env-var'd) | Rotate at clerk.allternit.com; **history still has it** |
| GitHub history | all of the above | gitleaks workflow added (`.github/workflows/secrets.yml`); run `gitleaks detect` clean on HEAD; history rewrite is your call |

## 7. Enable web features (when ready — product decision)

| | |
|---|---|
| **Status** | Agent-sessions + office + beta ON. Rails/runtime/rest still fail-closed. |
| **What** | Turn on the namespaces now served by the control plane |
| **Where** | `surfaces/ai.allternit.com/.env.production` + Pages build env |
| **How** | `AGENT_SESSIONS_API`, `OFFICE_API`, `BETA_API` are `1`. Remaining: `RAILS_API`, `RUNTIME_API`, `TOOLS_API`, `PERMISSIONS_API`, `QUESTIONS_API`, `MODEL_LAB_API`. |
| **Verify** | Signed-in user on the paired account (`user_3IBvYk8…`, mail node `contabo-byo-1`) can use agent-sessions / office / beta. Other accounts get 428 "pair a device". |

## 8. Deferred (no action needed now)

- **iOS Xcode build** — mobile changes merged; build in Xcode before the next app release (owner deferred). Release builds never sent the dev token, so there is no urgency.
- **Coverage gaps** — 21 rails data-plane routes (plan/gate/leases/context-packs) and canvas get/update/delete control-plane routes are flagged TODO in code; build when those features get users.
