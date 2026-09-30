---
name: jev-guardrail
description: Screen text or agent tool calls with a battery of local System One Noul hazards plus a Score, with policy in code — and the Claude Code PreToolUse guard (hooks/pretooluse-guard) that puts deterministic hard rules first and lets Jev only raise friction. Use for injection/secret/destructive-action screening or when the user runs /jev-guardrail.
---

# Jev guardrail (local first, raise-only)

Adapted from [cobusgreyling/Jev](https://github.com/cobusgreyling/Jev) `skills/jev-guardrail` (MIT). Cookbook: docs.typesafe.ai/cookbooks/llm_guardrails.

Jev doesn't refuse in prose. It scores hazards, and **code** returns pass, review, or block.

## FORBIDDEN

- **Hard rules run first and are final.** A deterministic deny/ask (rm -rf outside tmp, force-push to main/master, credential paths, `confirm: true` on `stripe_*`/`cloudflare_deploy_*`, prod migrations, shell deploy/publish) is never softened, skipped, or re-asked by Jev.
- **Raise-only.** Jev can turn pass into review, or no-decision into ask. It never emits allow, never approves, and never lowers a hard-rule verdict.
- **Never the decider for money, publishing, deploys, or client comms.** Those are human gates in CLAUDE.md whatever the scores say.
- **No client or regulated data, and no secrets, in `state` for remote backends.** The Claude Code guard only ever sends the tool name, redacted command text, redacted paths, the cwd, and MCP argument *keys*. It never sends file contents, edit bodies, or MCP argument values.
- **Injection-exposed state is never screened by Jev alone.** A 3B local model is easy to talk around. Keep a deterministic layer in front.

## Two lanes

**1. Content guardrail (product code).** One request carrying independent Nouls per hazard (`jailbreak`, `prompt_injection`, `secret_exfil`, `harmful_request`; on output `secret_leak`, `broke_policy`, `hallucination`) plus a harm Score. Gate each Noul on its probability. Content guardrails **fail closed**: if the call errors, send the item to review.

**2. Claude Code PreToolUse guard** (`tools/system-one-local/hooks/pretooluse-guard`):

- The hard rules settle the call first. When they do, the model isn't called at all.
- Otherwise the hazard pack (`destructive`, `exfiltration`, `production`, `money_or_publish` Nouls and a `blast_radius` Score) goes to the local server.
- `SYSTEM_ONE_HOOK_MODE=log` (default) is a dry run. It never emits a decision and appends to `~/.allternit/system-one/dryrun/<date>.jsonl`.
- `advise` emits hard-rule deny/ask, and may escalate to `ask` when any Noul is at or above 0.7 or the Score is at or above 2.5.
- If the server is down or times out, the hook emits **no decision**, so Claude Code's normal permission flow applies. It never fails to allow.
- Review with `bun scripts/dryrun-summary.ts` before switching on `advise`.

Measured on llama3.2 3B (2026-09-29, 8 commands, not a calibration study): the `destructive` Noul separated benign reads (0.45–0.67) from deletes/resets (0.90–0.97). `blast_radius` stayed flat at about 2.0 on everything, so it carries no signal on this model. Treat the dry-run log as the evidence to collect before trusting any threshold.
