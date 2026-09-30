---
name: jev
description: Ask typed System One judgments (noul / choice / score) against Allternit's LOCAL System One server instead of prompting a chat model to "return JSON". Use when classifying, routing, scoring, or screening something and code needs a probability it can branch on, or when the user runs /jev. Official TypeSafe Jev is an optional, explicit remote lane.
---

# Jev — System One judgments (local first)

Adapted from [cobusgreyling/Jev](https://github.com/cobusgreyling/Jev) `skills/jev` (MIT, © 2026 Jev Showcase contributors). The contract is TypeSafe's official System One API (docs.typesafe.ai). `jevai.org` is a community site, **not** official — don't use it as a source or endpoint.

## Where to send it

1. **Local (default):** `POST http://127.0.0.1:7717/v1/systemone` — the owned server in `tools/system-one-local/` (`bun src/cli.ts serve`). No key, loopback only, runs on the local Ollama/llama-server model.
2. **Official TypeSafe (optional, paid):** same body with `"model": "typesafe:jev-latest"`, and only when `TYPESAFE_API_KEY` is set. The local server passes it through to `https://api.typesafe.ai/v1/systemone`. Never for client, regulated, or secret data (see FORBIDDEN).

CLI: `system-one ask --state state.txt --questions questions.json [--server http://127.0.0.1:7717]`.

## FORBIDDEN

- **Hard rules first, and they are final.** Deterministic checks, Claude Code permission deny/ask, and every CLAUDE.md review gate run before Jev and can't be overridden by it.
- **Jev may only raise friction** (pass → review, allow → ask). Never use a Jev answer to auto-allow, auto-approve, skip a confirmation, or relax a threshold.
- **Jev never decides money, publishing, deploys, or client comms.** `stripe_*`, `cloudflare_deploy_*`, invoices, quotes, SOWs, and customer messages stay human-approved.
- **No client or regulated data in `state` for remote backends** (`typesafe:*`): no client names/records, health/financial/legal data, credentials, or secrets. Keep that local, and redact it anyway.
- **State a third party could have written is never judged by Jev alone.** That covers web pages, emails, tickets, tool output, and PR text. Pair it with a deterministic check or a human.
- Don't fake System One through OpenRouter's `typesafe/jev-router`. It's a chat-model router (see `/jev-route`).
- Local probabilities come from a small uncalibrated model (llama3.2 3B by default). Don't copy TypeSafe's published thresholds as if they carry over. Tune on your own data first.

## When to call it

- Classify, route, score, guardrail, re-rank, or verify something where code needs a value it can `if` on
- Cases where you'd otherwise prompt an LLM to "return JSON"

Don't call it for writing, explaining, coding, chatting, arithmetic, date math, or open-ended extraction (generate candidates first, then use a Choice).

## Primitives

| Type | Returns | Notes |
|------|---------|-------|
| `noul` | `noul` = P(yes) | optional `criteria: {true, false}`; no confidence field |
| `choice` | `choice`, `probabilities` (sum 1), `confidence` | `criteria` map option → description or null, up to 255 options |
| `score` | `score` (probability-weighted, can fall between levels), `legend`, `probabilities`, `confidence` | `criteria` is an ordered array of 2–10 levels |

`confidence = (n·max − 1)/(n − 1)`. Question ids aren't sent to the model, so put the whole question in `instructions`. Questions are evaluated independently and can't see each other.

The local server adds `x_allternit.methods[id]`, which is `logprobs` (read from token probabilities) or `sampled` (k-sample vote fallback, so treat it as coarser).

## Rules

1. Ask every independent question in **one** request (`/jev-fanout`).
2. Keep control flow, weights, and side effects in **code**.
3. Gate Choice/Score on `confidence` and Noul on the probability. Don't reuse a Noul threshold on a Choice.
4. Send only the state the questions need, and point at fields like `` `ticket.messages[0].text` ``.

## Minimal request

```json
{
  "model": "jev-latest",
  "state": "I was charged twice. Please refund the duplicate today.",
  "questions": {
    "department": { "type": "choice", "instructions": "Which team should handle this?",
      "criteria": { "billing": "Payments and refunds", "technical": "Bugs or integrations", "other": null } },
    "refund_requested": { "type": "noul", "instructions": "Does the message request a refund?" },
    "urgency": { "type": "score", "instructions": "How time-sensitive is this?",
      "criteria": ["No deadline", "Within a week", "Today or sooner"] }
  }
}
```

Siblings: `/jev-fanout`, `/jev-guardrail`, `/jev-route`.
