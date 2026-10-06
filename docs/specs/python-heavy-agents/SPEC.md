# SPEC — Reorienting gizzi-code for Python-heavy frontier agents

**Date:** 2026-09-08
**Status:** Proposed (companion to `REPORT.md`)
**Author:** research session (Kimi), session branch `session/pyagents-spec`
**Scope:** `cmd/gizzi-code`, `sdk/allternit-sdk` (code-execution tool), `cmd/allternit-api` (gateway permission path), surfaces where noted.

---

## 1. Problem statement

Frontier models driving gizzi-code sessions (Claude Fable 5/5.1, GPT-6 Astra) increasingly accomplish tasks by writing and executing Python. Today that traffic flows through `bash` with **no active sandbox** (the `@anthropic-ai/sandbox-runtime` integration is stubbed — `src/vendor/anthropic-stubs/sandbox-runtime.ts` makes `shouldUseSandbox` always false), the headless gateway path (Code surface via `allternit-api` LLM proxy) has no defined permission resolution, and the only dedicated code-execution tool in the repo (SDK tool belt) is insecure and overclaims. See `REPORT.md` for the evidence base.

## 2. Goals / non-goals

**Goals**
1. Every command/script a model executes runs inside a real OS-level sandbox with deny-by-default networking, on both macOS (Seatbelt) and Linux (bubblewrap).
2. Headless (gateway-driven) sessions have an explicit, safe-by-default permission policy — no silent `yolo`.
3. Code execution becomes a first-class, honestly-described tool with resource limits, structured results, and harness-written receipts.
4. The harness logs which model actually served each request (vendor silent-fallback detection).
5. The capability is surfaced deliberately on all four surfaces per the `GIZZI.md` scoping gate.

**Non-goals (this spec)**
- Multi-agent orchestration contracts (receipts across sub-agents) — follow-up spec.
- Windows sandboxing (PowerShell provider currently unsandboxed; document as unsupported for exec).
- Porting models to the Responses API — we wrap CLIs/AI SDK, not raw OpenAI API.

## 3. Phased design

### P0 — Honesty and containment (blocking)

**P0.1 — Remove or replace the sandbox stub.**
Two acceptable paths; decide in implementation:
- (a) **Restore the real dependency**: un-stub `@anthropic-ai/sandbox-runtime` in `cmd/gizzi-code/tsconfig.json`, add it to `package.json`, and wire `sandbox-adapter.ts` end-to-end. This is the design intent already present in the codebase.
- (b) **Delete the dead path**: remove the stub, `shouldUseSandbox`, `dangerouslyDisableSandbox` schema param, and all inert allow/deny config, so nothing claims sandboxing that doesn't exist.
Path (a) is strongly preferred — path (b) leaves us building the P1 tool without containment. Note `ComputerUse.md` already flags the F-grade computer-use subtree with "dependency missing — unbuildable" for the same vendoring reason; check whether one un-stubbing fixes both.

**P0.2 — Harden `sdk/allternit-sdk/src/ai-runtime/tools/code-execution.ts`.**
- Eliminate shell interpolation: run `python3 -c` via `execFile` with an argv array, never `sh -c "...'<code>'..."`.
- Remove inline `pip install` prepending. Dependencies become a declared input resolved through the P1 environment layer; until that lands, the tool refuses `dependencies` with a clear error rather than silently installing.
- Rewrite the tool description to state the real isolation properties (today: none). Overclaiming isolation in a model-facing tool description is a safety bug, not marketing.

**P0.3 — Define headless permission resolution.**
- In `PermissionNext` (`src/runtime/tools/guard/permission/next.ts`), add an explicit `headless` resolution path for sessions created via the gateway (`cmd/allternit-api/src/llm_gateway/proxy.rs`): never interactive-prompt; resolve against the session's DB-persisted ruleset; **default-deny** for anything the ruleset doesn't explicitly allow; emit a `permission.skipped_headless` trace event for every auto-decision.
- Until P1 lands, the gateway path must refuse `yolo`/`bypassPermissions`/`dontAsk` modes at session creation.

**P0 acceptance:** `isSandboxingEnabled()` returns true on macOS in a test run; SDK tool no longer interpolates code into a shell string; a gateway-created session with no ruleset cannot execute a write command without a trace event.

### P1 — First-class code execution (`python_exec`)

New builtin tool `python_exec` in gizzi-code (`src/runtime/tools/builtins/`):

```
python_exec(code: string,            # executed via argv, never shell
            timeout?: number,        # default 120s, hard max 600s
            dependencies?: string[], # requires allowlist match; else BLOCKED receipt
            run_in_background?: boolean)
→ { stdout, stderr, exit_code, artifacts: [{path, kind}], duration_ms, receipt_id }
```

- **Execution**: inside the P0 sandbox (Seatbelt/bubblewrap), cwd = session workspace, no network except domain-allowlist via proxy ports (the `srt` model), rlimits on memory/CPU/pids.
- **Environment**: per-session venv at `<project>/.gizzi/venv`, `uv` preferred if present (matches what Fable itself reaches for), `python3` fallback; interpreter discovery cached. `dependencies` install into the session venv only, and every install is written to the receipt.
- **Receipts**: every execution appends an append-only JSONL receipt (harness-written, not model-written): `{receipt_id, ts, code_hash, exit_code, artifacts, installs, sandbox_profile}` under the session's data dir, mirrored to `SessionTrace`.
- **System-prompt contract**: the tool description states completion criteria expectations (this is also where Astra's early-stopping and Fable's proactivity get shaped: "when the oracle command exits 0, stop").
- **Notebook parity**: `notebook_edit` gains an execute path through the same sandbox, or the tool is marked edit-only in its description (today it edits but never executes — undocumented).

**Model-facing gating:** `python_exec` is offered to all models; `bash` remains available. We do *not* try to suppress bash-Python (the CodeAct evidence says leaning in beats fighting).

**P1 also includes:**
- **Executing-model logging**: extend `SessionTrace.append` (`llm.ts:223`) with the served model id + provider per request; surface `stop_reason: "refusal"` (Fable fallback signal) as a first-class trace event; add a `model.mismatch` warning when failover rotated providers mid-session.
- **Provider param maps** (`adapters/transform.ts:264-396`): Astra-class → `reasoningEffort` levels (low/medium/high/xhigh/max) and no `temperature`; Fable 5.1 → always-on thinking (reject `thinking: disabled`), effort parameter instead of thinking budgets. Add both models to the catalog with correct context/output limits (Astra 1.05M ctx with the 272K pricing cliff noted; Fable 1M ctx / 128K out).
- **Streaming robustness**: parts parsing must select by block `type`, never `content[0]` (empty-thinking swallowing); progress expectations adjusted for Fable's sparse updates.

**P1 acceptance:** `bun test test/session/` green including new `python-exec` tests (sandbox engagement, argv-not-shell, receipt append, dependency allowlist, timeout kill); a live Fable 5.1 session doing the Willison-style task (screenshot → window enumeration) runs its Python inside the sandbox with a complete receipt; trace shows the true served model.

### P2 — Verification oracle, cost guards, and surfaces

- **Oracle**: session/task spec may declare `oracle: "<command>"`; when declared, the harness runs it (sandboxed) after the model's finish and stamps the session `verified | failed` — completion is no longer inferred from stream end alone.
- **Cost guards**: per-session token/time budget with doom-loop-adjacent caps (the existing 3-identical-calls detector generalizes); cheap-model routing hook point for the debug subloop.
- **Surfaces (the `GIZZI.md` gate — this is product work, not an afterthought):**
  - `gizzi-code` CLI: tool + `gizzi sandbox status/doctor` command showing profile and violation store state.
  - **Web/Desktop** (`surfaces/ai.allternit.com`): approvals UI for code-exec events on gateway sessions (the headless ruleset editor + per-event review where policy is `review`).
  - **iOS**: agent-sessions bridge inherits the same policy; render receipts in the session view.
  - A://Labs: this spec is a natural ADV-tier module topic (tool design + containment) — track, don't block.
- **Docs**: `docs/public/tools/tool-belt.md` updated to describe real isolation; `docs.gizziio.com` sandbox page.

**P2 acceptance:** e2e via gateway: model writes and runs Python through `python_exec`, web UI shows the approval/receipt, oracle stamp appears in session record, iOS session view renders the receipt.

## 4. Risks and open questions

| Risk | Mitigation |
|---|---|
| Seatbelt UX friction (permission fatigue) | Sandbox-first design *reduces* prompts (84% in Anthropic data); default-deny headless only outside sandbox |
| `uv` not installed on user machines | Graceful `python3 -m venv` fallback; docs recommend uv |
| Models route around `python_exec` via bash anyway | Accept (CodeAct); bash is sandboxed too post-P0, so the bypass is contained |
| Sandbox breaks legitimate workflows (git hooks, etc.) | GitSpawn cuts both ways — document that P0.1 audits what runs *before* sandbox arms; provide fs-profile escape hatch with receipts |
| Astra Python-bias premise unverified | First-party eval: run 20-repo task corpus through both models, log tool-call mix; spec does not depend on the premise (Fable evidence alone justifies it) |
| Vendor patch status drift (GitSpawn follow-ons) | Subscribe to Manifold Security advisories; `gizzi agents doctor`-style version gate for executor CLIs |

## 5. Verification plan

- `bun test test/session/` (currently 109 passing — must stay green + new tests per phase).
- `bun run typecheck` in `cmd/gizzi-code`; `cargo test -p allternit-api provider_routing` if gateway path touched.
- Live smoke: Fable 5.1 + Astra sessions on a representative task (each writes Python); verify sandbox engagement via `SandboxViolationStore`, receipt completeness, served-model logging.
- `dead-code-guard.test.ts` / `test/AGENTS.md` conventions respected.

## 6. Out of scope but noted

- Allternit Brain `Infra/model-routing.md` A://Fe → should move Fable 5 → **Fable 5.1** (cache pricing change materially affects our long-session loops). Brain-side change, not this repo.
- `agent-orchestrator` skill: its yolo spawn table should prefer sandboxed executor modes where available. Separate repo/skill change.
