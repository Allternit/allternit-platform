# allternit-system-one

A local, owned server for small typed decisions. It speaks the same contract as TypeSafe's **System One** API (`POST /v1/systemone`, with `noul` / `choice` / `score` questions), but the answers come from a model running on this machine: Ollama or llama-server. It's personal tooling first, and could go to Allternit as a beta once the dry-run data says it's worth it.

- **Local by default.** It binds `127.0.0.1` only, needs no key, and the state never leaves the machine.
- **Official TypeSafe is optional.** It only runs when `TYPESAFE_API_KEY` is set **and** the request asks for `model: "typesafe:<model>"`. That request is passed straight through to `https://api.typesafe.ai/v1/systemone`.
- **The OpenRouter `typesafe/jev-router` model is not used for System One.** It routes between chat models and has no System One API. It's exposed only through the separate `route-model` helper, which is paid and needs `--allow-paid`.

Runtime: Bun (≥ 1.1), with no npm dependencies.

## Quick start

```sh
cd tools/system-one-local
bun src/cli.ts serve                       # http://127.0.0.1:7717 (SYSTEM_ONE_PORT / --port)
bun scripts/smoke.ts                       # example 3-question pack, prints answers + latency
bun src/cli.ts ask --state examples/state.txt --questions examples/questions.json
bun src/cli.ts ask --state s.json --questions q.json --server http://127.0.0.1:7717
bun test test/                             # unit tests (no network)
```

Library:

```ts
import { SystemOne } from "./tools/system-one-local/src/index.ts";
const res = await new SystemOne().evaluate({ model: "jev-latest", state, questions });
```

## API

| Route | Notes |
|---|---|
| `POST /v1/systemone` | `{model, state: string\|object\|array, questions: {id: {type, instructions, criteria?}}}` → `{model, answers, usage: {input_tokens, output_tokens}, x_allternit}` |
| `GET /v1/models` | `{object: "list", data: [{id, description, backend}]}` |
| `GET /healthz` | liveness |

These models are accepted: `jev-latest`, `jev-preview`, `jev-<version>`, `local`, `local-latest`, `default`, and `local:<runtime model>`. The `jev-*` aliases are there so clients built on the official SDK work unchanged. Use `typesafe:<model>` for the remote lane.

Answers follow the official shapes:

- **noul**: `{type, noul}`, where `noul` is P(yes).
- **choice**: `{type, choice, probabilities (sum 1), confidence}`.
- **score**: `{type, score, legend, probabilities, confidence}`. `score = Σ i·p_i` over 0-indexed levels, so it can fall between levels.
- **confidence**: `(n·max − 1)/(n − 1)`, clamped to [0, 1].

Validation matches the documented limits: choice allows up to 255 options (at least 2), score needs 2–10 levels, and noul `criteria` only takes `true` and `false`.

The response adds one extension field, `x_allternit`:

- `backend` and `runtime`
- `methods[id]`: `logprobs`, `sampled`, or `remote`
- `label_mass[id]`: the share of the top-k probability mass that landed on valid labels
- `latency_ms`

Errors come back as `{error: {type, message, details?}}`:

| Status | When |
|---|---|
| 401 | `SYSTEM_ONE_TOKEN` is set and the bearer is missing, or `typesafe:*` was requested without a key |
| 422 | Validation failed; `details[]` gives `{path, message}` |
| 429 | Too many requests in flight (over `SYSTEM_ONE_MAX_INFLIGHT`, default 8) |
| 529 | The local runtime is unreachable, or the upstream is overloaded |

## How the probabilities are obtained

1. **One call per question, same state.** Each question is a separate chat completion, so questions can't see each other. The prompt opens with the state, which lets Ollama and llama-server reuse the KV prefix cache across the questions in one request.
2. **Closed-set labelling.** Options are shown with letter labels (`A) billing — …`) and score levels with digits (`0: Calm`). A noul becomes a two-option lettered choice (`A) Yes`, `B) No`). The model is asked for exactly one label, with `max_tokens: 1`, `temperature: 0`, `logprobs: true` and `top_logprobs: 20`.
3. **Read the next-token distribution. Never parse free text as a probability.** Token strings are trimmed, so `" A"` and `"A"` sum together, and the result is renormalized over the valid labels only.
   - A label missing from the top-20 gets a floor: at most the smallest probability shown, and together no more than the leftover mass.
   - `label_mass` reports how much of the top-k fell on valid labels. A low value means the model wanted to say something else.
4. **More than 20 options:** `top_logprobs` is capped at 20, so options are split into groups of up to 20. The model scores the group first, then the option within every group, and P(option) = P(group) · P(option | group).
5. **Fallback when logprobs aren't available:** this applies if the runtime returns no logprobs, or if no valid label appears in the top-k. The question is then asked `SYSTEM_ONE_SAMPLES` times (default 8) at temperature 1. The first label token of each reply is counted as a vote, and the method is reported as `sampled`.
6. **Optional debiasing:** `SYSTEM_ONE_DEBIAS=1` asks each question twice, with the options forward and reversed, and averages the two. It doubles the calls. It's **off by default** because on llama3.2 3B it pushed benign commands toward "yes" (see Limitations).

Why letters for noul: on llama3.2 3B, bare `Yes`/`No` tokens leaned strongly toward "No" (P(yes) = 0.38 for "Please refund the duplicate charge" → "does it request a refund?"). Lettered options gave 0.97. On a 7-item probe, mean absolute error was 0.32 with Yes/No tokens and 0.05 with letters. That's a sanity check, not a calibration measurement.

## Configuration (env)

| Var | Default | |
|---|---|---|
| `SYSTEM_ONE_PORT` | `7717` | host is always `127.0.0.1` |
| `SYSTEM_ONE_RUNTIME_URL` | `http://127.0.0.1:11434/v1` | Ollama; llama-server e.g. `http://127.0.0.1:8080/v1` |
| `SYSTEM_ONE_RUNTIME_MODEL` | `llama3.2:latest` | |
| `SYSTEM_ONE_CONCURRENCY` | `4` | questions in flight per request |
| `SYSTEM_ONE_SAMPLES` | `8` | sampling fallback |
| `SYSTEM_ONE_DEBIAS` | off | `1` = forward+reversed averaging |
| `SYSTEM_ONE_TOKEN` | unset | require bearer auth |
| `SYSTEM_ONE_MAX_INFLIGHT` | `8` | → 429 |
| `SYSTEM_ONE_LOG` | off | `1` = `~/.allternit/system-one/log/<date>.jsonl` (request sha256, token counts, answers — never state or question text) |
| `TYPESAFE_API_KEY` | unset | enables `typesafe:*` passthrough |
| `OPENROUTER_API_KEY` | from env | only used by `route-model --allow-paid` |

## Claude Code hook: `hooks/pretooluse-guard`

The hook works through each tool call in this order:

1. **Hard rules first.** They're deterministic and final:
   - `rm -rf` outside a temp dir → ask. On root, home, top-level directories, or credential paths → deny.
   - Force-push, or deleting `main`/`master` on a remote → deny. A force-push with no explicit branch → ask.
   - Credential paths (`~/.ssh`, `~/.aws`, `.env*` except `.env.example`) → reading asks. Writing or removing `~/.ssh` and `~/.aws` → deny.
   - `confirm: true` on any `stripe_*` or `cloudflare_deploy*` MCP tool → ask.
   - Prod migrations → ask. That covers `prisma migrate deploy`, `supabase db push`, `wrangler d1 migrations apply --remote`, and migrate commands carrying prod markers or a non-local `DATABASE_URL`.
   - Deploy or publish from the shell → ask. That covers `wrangler deploy` / `pages deploy`, `vercel --prod`, `netlify deploy --prod`, `npm`/`pnpm`/`yarn`/`bun`/`cargo publish`, and `gh release create`. `--dry-run` passes, and runners like `npx`, `bunx`, and `pnpm exec`/`dlx` are unwrapped first.
2. **Otherwise, the hazard pack goes to the local server.** The state is the tool name, the **redacted** command text, the redacted paths, the cwd, and MCP argument *keys*. It never includes file contents, edit bodies, or MCP argument values.
   - Redaction replaces secrets with `[SECRET]` and emails with `[EMAIL]`. It replaces `/Clients/<name>` with `[CLIENT]` and `X LLC`/`X Inc` with `[ORG]`, and flags anything that looks like a person's name.
   - The pack asks four Nouls (`destructive`, `exfiltration`, `production`, `money_or_publish`) and one Score (`blast_radius`).
3. **The model can only raise friction.** In `advise` mode the call is escalated to `ask` if any Noul is ≥ `SYSTEM_ONE_HOOK_NOUL_BAR` (0.7) or the Score is ≥ `SYSTEM_ONE_HOOK_SCORE_BAR` (2.5).
   - The hook never emits `allow`.
   - It never consults the model once a hard rule has decided.
   - If the server is down, times out (`SYSTEM_ONE_HOOK_TIMEOUT_MS`, default 4000), or returns an error, the hook emits nothing and Claude Code's normal permission flow applies.

Modes are set with `SYSTEM_ONE_HOOK_MODE`:

| Mode | Behaviour |
|---|---|
| `log` (default) | A pure dry run. It never changes a decision, not even for a hard rule, and appends one record per call to `~/.allternit/system-one/dryrun/<date>.jsonl` (override the path with `SYSTEM_ONE_DRYRUN_DIR`). Each record holds the hard-rule verdict, the pack hash and size, a token estimate, the answers, and the redaction flags. |
| `advise` | Emits the hard-rule deny/ask, and may escalate to ask. |
| `off` | Does nothing. |

Add the hook to a **project-scoped** `.claude/settings.json`. This repo doesn't wire it anywhere, and you shouldn't put it in `~/.claude/settings.json` until the dry run has been reviewed.

```json
{
  "env": { "SYSTEM_ONE_HOOK_MODE": "log" },
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash|Read|Write|Edit|MultiEdit|NotebookEdit|mcp__.*",
        "hooks": [
          {
            "type": "command",
            "command": "\"$CLAUDE_PROJECT_DIR\"/tools/system-one-local/hooks/pretooluse-guard",
            "timeout": 10
          }
        ]
      }
    ]
  }
}
```

Summarise the dry run with `bun scripts/dryrun-summary.ts [--dir …] [--json]`. It reports:

- calls per day
- mean tokens, actual and estimated
- the share of calls settled by hard rules, broken down by rule
- the share escalated, overall and among calls that reached the server
- server status
- the method mix
- redaction flags: secrets, emails, client-looking names, possible person names

## Skills

`skills/jev`, `skills/jev-fanout`, `skills/jev-guardrail`, `skills/jev-route` are adapted from [cobusgreyling/Jev](https://github.com/cobusgreyling/Jev) (MIT, see `skills/NOTICE.md`). They point at the local server first, keep official TypeSafe as an optional lane, and each has a FORBIDDEN block.

They live here rather than in `.agents/skills/`, which is managed by `skills-lock.json`. To use one, symlink it into `~/.claude/skills/` or a project's `.claude/skills/`. `jevai.org` is a community site, not an official one, and isn't used.

## Limitations (honest)

- **Calibration is unmeasured.** The default model is llama3.2 3B (Q4_K_M). Nobody has checked that its probabilities are calibrated; they are token probabilities from a small general chat model, not a model trained for this the way TypeSafe says Jev is. Don't carry TypeSafe's published thresholds over. Collect dry-run data and labelled examples first.
- **Hazard-pack signal on 3B is weak.** On 8 probe commands (2026-09-29):
  - `destructive` separated reads (0.45–0.67) from deletes/resets (0.90–0.97), but `ls -la` still scored 0.67, just under the 0.7 bar.
  - `blast_radius` was about 2.0 for everything, so it carries no signal.
  - `money_or_publish` missed `wrangler pages deploy` (0.18). Shell deploys and publishes are now a hard rule for that reason.
  - With debiasing on, every Noul drifted up (benign commands scored 0.72–0.83), which is why it's off by default.
  - This is why the hook ships in `log` mode.
- **Usage counts the state once per question.** A 5-question pack on a short command is about 900 input tokens locally. Hook latency was about 0.85–1.0 s per consulted call with Ollama running the questions one after another, and about 0.1–0.2 s when a hard rule settles the call. With llama-server's parallel slots a warm 3-question pack took about 73 ms.
- **Position bias remains** (letter-A preference). Debiasing exists but didn't help on this model.
- **The hard rules are heuristics over shell text.** They don't expand variables, aliases, or scripts. `bash -c '…'` is inspected recursively, but `eval`, sourced scripts, and indirect paths aren't.
- **The official-API error body shapes aren't documented publicly** beyond the status codes. The `{error: {type, message}}` bodies here are this server's own.

## Canonical decision runtime (WP8)

This package is the ONE canonical decision server. It speaks the frozen Kernel ABI 1.0.0 decision contracts (`spec/Contracts/kernel/v1/schemas/decision.schema.json`) on `POST /v1/decision` (`{request: DecisionRequestV1, state, reversible?}` returns `DecisionResultV1`); `/v1/systemone` stays as the SDK-compatible wire shape over the same engine.

- `src/decision/readout.ts`: `DecisionReadoutProvider` (logits first, then calibrated readouts). `LocalLogitReadoutProvider` wraps this engine; `FixtureReadoutProvider` is for tests and replay.
- `src/decision/manifest.ts`: calibration manifests bound to model / revision / tokenizer / quantization / runtime / question / candidate schema / candidate set. Any drift breaks the binding.
- `src/decision/gate.ts`: the Q22 gate as code. ECE <= 0.05 held-out, <= 5% observed error on the auto-act subset with Wilson/bootstrap bounds, minimum sample floors, reversible-only, and agreement with another LLM is never an input.
- `src/decision/router.ts`: SHADOW by default. Without a bound, gate-passing manifest the result is `UNCALIBRATED`, abstained, and never `AUTO`. `AUTO` needs live mode + a passing per-primitive manifest + the caller attesting reversibility + confidence inside the calibrated auto-act region.
- `src/decision/threshold.ts` (CL-158 Threshold Policy) and `src/decision/motifs.ts` (CL-156 DecisionMotif library): thresholds are policy, never model output.
- Metrics: accuracy, macro F1, Brier, NLL, ECE, coverage-at-risk, flip and order sensitivity (`metrics.ts`).

Python side: `domains/computer-use/core/core/{decision_head,laya_head,semif_head}.py` are in-process `DecisionHead` backends for the computer-use planning loop, not servers. They stay as thin adapters (backend profiles of kind `decoder_readout` / `schema_encoder`) to be consumed through a `DecisionReadoutProvider`; no second server is kept.
