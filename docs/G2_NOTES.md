# G2 notes — close the three connector gaps from #1182

Worktree: `allternit-wt-p-g2`, branch `prog-g2-connector-gaps`.
Started from the previous worker's WIP commit `3aebb7d78` ("WIP G2 (Claude
worker, handed to Kimi)"); finished work is commit `b1fb09838`, pushed.

## PR

DRAFT (not merged, per instructions):
https://github.com/Gizziio/allternit-platform/pull/1185

## Gap 1 — thread posts reach the live session

Kept the WIP implementation (reviewed line by line, compiled, tested):

- `effects/thread.rs` gained `deliver_live_with` / `deliver_live`: after the
  ledger post, the same reply is appended to the thread's **open session**
  (the gizzi transcript the thread UI streams) through the same
  `agent_session_routes::append_vendor_message` transport the Agent Gateway
  uses for bot replies — placement-target aware, local gizzi otherwise.
  `Ok(None)` when the thread has no open session yet (the ledger post is
  still there for when it opens). Ownership + non-incognito checked in SQL.
- Executor (`executor/generic.rs`): the `thread:` arm posts to the ledger,
  then delivers live **best effort** (`tracing::warn` on failure — the
  ledger post is the record).
- Idempotency is end to end: gizzi dedupes vendor messages by
  `metadata.remote_event_id`, set to `agency:<effect key>`. A replayed
  effect after a crash between "applied" and "committed" re-sends with the
  same dedupe id (gizzi keeps one message); a committed effect never reaches
  the transport again (P1's journal serves the result before the closure).
- Tests (WIP, kept): `live_post_reaches_the_open_session_once` — no session
  → ledger only; wrong owner → nothing; fenced replay → no re-delivery;
  crash-window replay → same dedupe id; ledger count 1.

## Gap 2 — a template that stops for attention parks the parent

Kept the WIP implementation and hardened it:

- `effects/template.rs`: a child in `needs_attention` with an open item
  returns a typed `NeedsPerson` error (child run id + attention item)
  instead of failing the effect. When the item was answered,
  `requeue_child` re-queues the child (or fails it on a rejection) and
  drives it; `template_exec`'s journal resumes after the answered step.
  The parent-side match also parks a child that ended `needs_attention`.
- `executor/generic.rs`: the effect closure captures `NeedsPerson`, the
  journaled failure stays **retryable**, and `park_on_child` opens ONE
  parent attention item (`template_child_attention`) linked by the child's
  attention id, then `StepErr::Stop`s. Re-drives with the child still
  waiting find the open linked item and do not duplicate it.
- `mod.rs`: `resolve_attention_linked` — answering the parent's item
  forwards the answer to the child's item first (same validation, same
  rules, `link_parents=false`); a `template_child_attention` answer
  re-queues the parent. `release_parents` handles the answer landing on the
  child's item directly: every linked parent item is resolved with the same
  resolution and those parents re-queue; their drive re-checks the child
  under the fence. Only a real failure fails the effect (a rejection fails
  the child → the effect fails closed; a genuinely failed child likewise).
- **Hardening I added**: the bail message in the executor is a fixed string
  ("template child stopped for a person: parked on it, re-checks after the
  answer"). The WIP used the child's attention title in the message, but
  `safety::classify_error` derives retryability from message text and
  template-step labels are user-controlled — a label like "policy check"
  would have classified the failure `non_retryable` and the parent would
  have failed after the answer instead of resuming.
- **New e2e**
  (`effect_template::tests::child_attention_parks_the_parent_and_its_answer_resumes_it`):
  park → parent item linked to the child's open `template_attention` item →
  (a) approve on the parent → answer forwards down → parent re-queues →
  re-checks the same child → both complete; (b) approve on the child's item
  → `release_parents` resolves the parent item with the same resolution →
  parent completes; (c) reject → child failed → parent effect fails →
  parent failed. Asserts exactly one parent item, one child run, one
  committed `tool.execute` effect per parent (the replayed re-drive after
  the answer never re-applied the effect).

## Gap 3 — computer connector against a real computer

Added the opt-in live test
(`agency_api::effects::computer::tests::live_local_read_only_against_the_real_gateway`,
`#[ignore]` + `ALLTERNIT_LIVE_COMPUTER_TEST=1`). It drives `computer:local`
through `computer::dispatch` against `ALLTERNIT_ACU_URL` (default the
Desktop's `http://127.0.0.1:8760`) with READ-ONLY actions only —
`screenshot`, `observe`, `cursor_position` — and asserts none of them is
consequential before sending. It never sends click/type/key/shell. When the
gateway is unreachable or refuses, it prints the exact URL and error and
fails (no faking).

### Live run 1 — the running Desktop's gateway (127.0.0.1:8760), PID 50618

FAILED, recorded exactly (this is the required run against the running
Desktop's computer-use gateway):

```
LIVE COMPUTER TEST FAILED against http://127.0.0.1:8760: computer action failed (200 OK): screenshot: no adapter available for direct execution; observe: no adapter available for direct execution; cursor_position: no adapter available for direct execution
test agency_api::effects::computer::tests::live_local_read_only_against_the_real_gateway ... FAILED
```

Raw gateway answer for a single screenshot (probe):
`{"run_id":"g2-probe-1",...,"status":"failed",...,"actions":[{"index":0,"action_id":"act-0","kind":"screenshot","status":"error","result":null,"error":"no adapter available for direct execution"}],...}`

Root cause (two layers, both real bugs):

1. **Repo + bundled gateway bug (fixed here).**
   `domains/computer-use/core/gateway/computer_use_router.py` imports
   `from gateway.canonical_router import history_preflight_for_task`, but
   `canonical_router.py` did not define it anywhere. The ImportError was
   caught at module load and silently set `_get_executor = None` and
   `_planning_available = False` in the router — so `/v1/computer-use/execute`
   **direct mode could never see the adapter waterfall** and always answered
   "no adapter available for direct execution" (the intent/planning path was
   equally degraded). The bundled copy under
   `/Applications/Allternit Desktop.app/Contents/Resources/computer-use/acu/`
   is byte-identical to the repo source, so the installed gateway has this
   too. Fix: `history_preflight_for_task(task)` now exists in
   `canonical_router.py` — best-effort, advisory-only: returns
   `{"status": <history_status>, "events": [...]}` from the first provider
   advertising the history tools, else `None`; never raises, never blocks a
   run.
2. **Connector error reporting bug (fixed here).** `dispatch` reported the
   gateway's top-level `error` field, which is `null` on per-action
   failures — operators would have seen `computer action failed (200 OK):
   null`. It now surfaces the per-action errors first.

### Live run 2 — repo-source gateway running this branch's fix (127.0.0.1:18762)

Started my own gateway instance from `domains/computer-use/core/gateway` on
port 18762 (all 5 adapters registered at startup). PASSED:

```
LIVE COMPUTER TEST OK against http://127.0.0.1:18762: computer:local:agency-80337ad4478df405b0c03fc3:sha256:14127b4f7fc00fc4a695085e2a003984075eeb13585a56bbc9281451cbab9d36
test agency_api::effects::computer::tests::live_local_read_only_against_the_real_gateway ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1940 filtered out
```

Per-action observations against the fixed gateway (curl probes, before the
Rust run): `screenshot` ok via `browser.cdp`; `cursor_position` ok via
`browser.cdp`; `observe` completes the run (top-level status `completed`)
but no adapter claims it (`adapter_id: none`) — see open items.

## Test results

`cargo test -p allternit-api --lib -- agency kernel_ui`, two full runs in a
row, both green:

```
run1: test result: ok. 97 passed; 0 failed; 3 ignored; 0 measured; 1841 filtered out; finished in 147.89s
run2: test result: ok. 97 passed; 0 failed; 3 ignored; 0 measured; 1841 filtered out; finished in 139.71s
```

Focused suites (all green):

```
agency_api::effect_thread::tests::live_post_reaches_the_open_session_once ... ok
agency_api::effect_template::tests::child_attention_parks_the_parent_and_its_answer_resumes_it ... ok
agency_api::effect_template::tests::starts_once_per_key_and_fails_closed ... ok
agency_api::effects::tests_c3b (computer + campaign replay/fence/approval/trigger, 4 tests) ... ok
```

## Smoke boot

Script: `cargo build -p allternit-api --bin allternit-api`, then booted the
binary ~20 s with a fresh temp HOME, `ALLTERNIT_DATA_DIR` on the same temp
dir, `ALLTERNIT_API_PORT=18321`:

```
build ok
api pid: 75901
health/live -> 200 after ~5500ms
{"status":"alive"}
```

Shutdown: killed the api (TERM, then KILL only if needed), compared
`pgrep -f "gizzi-code fabric-worker"` before/after — the boot spawned
exactly one worker (pid 76467), I killed only that one; all 6 pre-existing
baseline workers intact.

## Open items

- The **installed Desktop's ACU gateway (port 8760) still runs the pre-fix
  bundled code**. It needs the Desktop app restarted to pick up the fixed
  bundle (and, separately, its startup-time adapter registrations raced the
  Desktop relays at 16:27 today — a fresh start registers all 5 adapters; I
  verified with a same-code instance). Per the rules I did not kill or
  restart anything I didn't start.
- `observe` on the fixed gateway: run-level status `completed`, but no
  adapter claims the action (`adapter_id: none`) — the connector contract
  (top-level run status) is met, but per-action support is a gateway-side
  taxonomy follow-up.
- `docs/G2_TASK.md` left untracked (task input, not authored here).
