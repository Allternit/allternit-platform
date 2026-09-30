# CommRails Bridge (scoped remote identities)

Status: **built, default OFF.** Nothing listens until someone runs
`allternit-commrails bridge serve`, and the listener refuses every
non-loopback bind unless explicitly told otherwise. Enabling it on the mesh is
Eoj's call (see "Enabling" below). Tracks build-plan item P1-5 ("Box ↔ Mac
bridge for Chief").

## Why

Chief is a Grok bot on the shared "box" host. It writes `/workspace/…` there,
but CommRails DAGs live in ledgers under workspace roots on Eoj's Mac, so Chief
can neither create nor read them. The bridge gives Chief a narrow door:
**create and read plans, send and read coordination mail — nothing else.**

The shared box is not a security boundary: other tenants and processes on it
can read what Chief can read. So the design assumes the box, and the token on
it, can be compromised, and limits what a compromised box can do.

## What exists

| Piece | Where |
|---|---|
| Identity store (hashed tokens, 0600) | `src/bridge/identity.rs` |
| Listener, route table, guard, audit | `src/bridge/server.rs` |
| `identity add\|list\|revoke`, `bridge serve` | `src/bin/allternit-commrails.rs` |
| Remote prompt attribution (Gate 0) | `Gate::plan_new_with_origin`, `templates::plan_from_template_with_origin` |
| Box-side client (Python 3.8+ stdlib) | `tools/commrails-bridge-client/commrails-bridge` |
| Tests (loopback only) | `tests/bridge.rs`, unit tests in `src/bridge/*` |

### Identities

```bash
allternit-commrails identity add --actor bot:chief \
  --scopes plan:create,plan:read,mail:send,mail:read,template:instantiate
# prints the bearer token ONCE on stdout (crb_<64 hex>)
allternit-commrails identity list
allternit-commrails identity revoke --actor bot:chief     # or --id bid_…
```

- File: `$ALLTERNIT_COMMRAILS_BRIDGE_IDENTITIES`, else
  `~/.allternit/commrails-bridge/identities.json` (outside every workspace
  root, so it never lands in a repo). Written atomically at mode 0600; the
  listener refuses to authenticate against a file readable by group/other.
- Stored per identity: id, actor, scopes, **SHA-256 of the token** (never the
  token), created/revoked timestamps. Tokens are 256-bit random, so an unsalted
  hash is sufficient; comparison is constant-time.
- Actors are `bot:<slug>` or `agent:<slug>`. `user:` is refused: a remote
  identity never speaks as a human approver.
- Grantable scopes, exhaustively: `plan:create`, `plan:read`, `mail:send`,
  `mail:read`, `template:instantiate`.
- **Never grantable:** `wih:pickup`, `wih:close`, `lease:*`,
  `wait-gate:resolve`, `gate:*`, and `plan:refine` (graph mutation on an
  existing plan can include `ChangeStatus`, which would emulate a close).
  `identity add` refuses them; if someone hand-edits the file to add them, they
  are stripped when the file is loaded and the routes still answer 403.
- The file is re-read on every request, so **revoke takes effect on the next
  request** with no restart.

### Listener

```bash
allternit-commrails bridge serve --root <workspace-root> \
  [--bind 127.0.0.1:7433] [--allow-remote] [--identities <file>] \
  [--rate-limit-per-min 60]
```

Bind policy (checked before any socket opens):

- loopback: allowed;
- `0.0.0.0` / `::`: always refused, bind the single mesh address;
- any other address: needs `--allow-remote` **and** at least one active
  identity.

Every request, including `whoami`, needs `Authorization: Bearer <token>`.

| Route | Scope |
|---|---|
| `GET /v1/whoami` | (any identity) |
| `POST /v1/plan` `{text, decision_ref?}` | `plan:create` |
| `GET /v1/plan/:dag_id` | `plan:read` |
| `GET /v1/plan/:dag_id/nodes/:node_id/output` | `plan:read` |
| `GET /v1/templates` | `template:instantiate` |
| `POST /v1/templates/:id/instantiate` `{params, text?, decision_ref?}` | `template:instantiate` |
| `POST /v1/mail/send` `{thread_id, body, subject?, to?, importance?}` | `mail:send` |
| `GET /v1/mail/inbox?thread_id=&limit=` | `mail:read` |

Answered **403 `forbidden_capability`** for any authenticated caller, any
method: `/v1/wihs/…` (pickup, sign, close), `/v1/leases/…`,
`/v1/mail/reserve|release`, `/v1/wait-gates/…`, `/v1/gate/…`,
`/v1/mail/decide|review`, `/v1/plan/refine`. Everything else is 404. The
route table is one function (`classify_route`) used for both authorization and
the 403s, so the two cannot drift.

Other limits: 256 KiB request bodies, 16 KiB plan text, 64 KiB mail bodies,
template ids resolved from the store only (never a file path), ids restricted
to `[A-Za-z0-9_.-]`.

Status codes: 401 missing/unknown/revoked token, 403 missing scope or
forbidden capability, 429 rate limited (with `Retry-After`), 422 Gate 0 or
template-param refusal, 404 unknown route/dag/node/template.

### Provenance and audit

- `POST /v1/plan` and template instantiation go through Gate 0
  (`plan_new_with_origin`). The `PromptCreated` event's **actor is the remote
  identity** (`{type: agent, id: bot:chief}`) with payload `source: "bridge"`,
  `submitted_by`, `decision_ref` (Chief's own decision/log reference, opaque),
  and `request_id`. The initial `PromptDeltaAppended` (and a template's
  instantiation delta) is authored by the same actor. DAG mutations remain
  gate-emitted with `prompt_id`/`delta_id` provenance, so every node traces to
  a prompt the remote actor owns (GATE_RULES Gate 0).
- Mail sent over the bridge is a `MessageSent` whose event actor and
  `from_agent` are the caller; the body cannot choose a different sender.
- Every request appends a ledger event:
  - `BridgeRequest` (actor = the identity) with `request_id`, `identity_id`,
    method, path, status, scope, peer, and when relevant `target_dag`,
    `prompt_id`, `mail_thread`, `message_id`, `template_id`. The event scope
    carries `dag_id` for plan requests.
  - `BridgeRequestDenied` (actor `gate:bridge`) for 401s.
  - Flood control: 401 audits share a per-IP budget, and only the first 429
    per identity per window is audited, so a hostile client cannot grow the
    ledger without bound.

### Mirror

`commrails-bridge mirror <dag_id> [runs_dir]` writes
`<runs_dir>/<dag_id>/` (default `/workspace/runs/<dag_id>/`):

```
plan.json            {"mirror": true, "authoritative": false, taken_at, source, actor, dag}
nodes/<node>.out.md  recorded node outputs, sha256-checked against the receipt
README.md            "MIRROR — NOT TRUTH" + how to refresh
```

Files are 0444, directories 0555. A refresh builds a new snapshot beside the
old one and swaps it in. Nothing written on the box is ever synced back.

## Threat model

**Assets:** the Mac's ledgers (plans, mail, receipts), execution on the Mac
(WIH pickup/close runs agents with Mac credentials), approvals (wait-gates,
review decisions, gate decisions), leases (write locks).

**Adversary:** anyone who controls the box or reads Chief's token file there
(another tenant, a compromised process, a prompt-injected Chief).

| A compromised box CAN | A compromised box CANNOT |
|---|---|
| Create plans and instantiate templates (attributed to `bot:chief`) | Pick up or sign a WIH, so it cannot make anything run on the Mac |
| Read any DAG and node output in the served root | Close a WIH or change node status (no close, no `plan:refine`) |
| Send mail on `dag:`/`wih:`/`mail:` threads as `bot:chief` | Request, renew, or release leases |
| Read mail in the served root | Resolve wait-gates or make gate/review decisions |
| Spend its rate budget (60 req/min default) | Speak as a user, or as a different bot |
| | Reach any other CommRails route (ledger, vault, peers, steer, init …) |
| | Read files other than the ledger, mail bodies, node output blobs, and the template store |

Residual risks, accepted and documented:

- **Plan spam / misleading plans.** A hostile box can create junk plans or
  mail. They are attributed to `bot:chief` and joinable to `BridgeRequest`
  events, and nothing runs until a human or a local agent picks a node up.
  Humans should treat bridge-originated plans as proposals.
- **Read exposure.** `plan:read`/`mail:read` expose every DAG and mail thread
  in the served root. Serve a root whose contents are fine for the box to see.
  Drop `mail:read` or `plan:read` from the identity to narrow it.
- **Transport.** The listener speaks plain HTTP; confidentiality comes from
  the Tailscale WireGuard tunnel. The client refuses plain http to anything but
  loopback, `100.64.0.0/10`, `fd7a:115c:a1e0::/48`, or `*.ts.net`.
- **Token at rest on the box** is only as safe as the box's `0600` file. Assume
  it can leak; the scope ceiling above is what bounds the damage.

**Revoke = one command:** `allternit-commrails identity revoke --actor
bot:chief`. Effective on the next request, no restart. Then mint a new token
if Chief should keep access.

## Enabling (Eoj's call — not done by the build)

Nothing below has been applied. Each step is a manual decision.

1. Mint the identity on the Mac and move the token to the box:

   ```bash
   allternit-commrails identity add --actor bot:chief \
     --scopes plan:create,plan:read,mail:send,mail:read,template:instantiate \
     > /tmp/chief.token
   # copy to the box as ~/.config/commrails-bridge/token (chmod 600), then:
   rm /tmp/chief.token
   ```

2. Tailscale ACL: allow only the box to reach the Mac's bridge port. Example
   policy fragment (tag names are placeholders):

   ```jsonc
   {
     "acls": [
       { "action": "accept", "src": ["tag:box"], "dst": ["tag:eoj-mac:7433"] }
     ]
   }
   ```

3. Run the listener on the Mac's mesh address only. Example launchd plist
   (`~/Library/LaunchAgents/com.allternit.commrails-bridge.plist`; replace
   `100.x.y.z`, paths and root):

   ```xml
   <?xml version="1.0" encoding="UTF-8"?>
   <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
   <plist version="1.0">
   <dict>
     <key>Label</key><string>com.allternit.commrails-bridge</string>
     <key>ProgramArguments</key>
     <array>
       <string>/Users/joe/.local/bin/allternit-commrails</string>
       <string>bridge</string><string>serve</string>
       <string>--bind</string><string>100.x.y.z:7433</string>
       <string>--allow-remote</string>
       <string>--root</string><string>/Users/joe/Desktop/allternit-workspace/allternit</string>
     </array>
     <key>RunAtLoad</key><true/>
     <key>KeepAlive</key><true/>
     <key>StandardErrorPath</key><string>/tmp/commrails-bridge.log</string>
   </dict>
   </plist>
   ```

   Binding the `100.x` address (not `0.0.0.0`) keeps the port off LAN and
   public interfaces even if the Tailscale ACL is wrong. If the mesh address
   is not up yet at boot, the bind fails and launchd retries.

4. On the box: `COMMRAILS_BRIDGE_URL=http://100.x.y.z:7433 commrails-bridge whoami`.

Acceptance (P1-5): Chief runs `plan-from-template` from the box, the DAG
appears in the Mac rail under the served root, and a pickup attempt from the
box identity gets 403 (`POST /v1/wihs/pickup` → `forbidden_capability`).

To turn it off: stop the listener (unload the plist) or revoke the identity.
