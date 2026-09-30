---
name: jev-route
description: Route with local System One answers — map Choice + confidence to act / confirm / human lanes, or pick a model tier by Choice label. Also documents the separate `route-model` helper for OpenRouter's typesafe/jev-router (a chat-model router, NOT System One). Use for confidence routing, model-tier routing, or when the user runs /jev-route.
---

# Jev routing (local first)

Adapted from [cobusgreyling/Jev](https://github.com/cobusgreyling/Jev) `skills/jev-route` (MIT). Official pattern: docs.typesafe.ai/patterns/confidence-routing.

Jev returns a distribution, and **code** picks the lane. Ask the extra questions in the same call (`/jev-fanout`).

## FORBIDDEN

- Hard rules first, and they are final. A routing answer never skips a permission prompt or a CLAUDE.md gate.
- Jev may only raise friction: a low-confidence answer goes *up* a lane, toward confirm or human, and never down.
- Irreversible or consequential actions (money, publishing, deploys, client comms, deletes) are never in the `act` lane. They always go to a human, whatever the confidence.
- No client, regulated, or secret data in `state` when `model` is `typesafe:*`.
- State a third party could have written is never routed on Jev alone.
- `typesafe/jev-router` on OpenRouter is **not** System One. Never use it to produce `/v1/systemone` answers.

## 1. Confidence-gated action

| Confidence | Lane |
|------------|------|
| ≥ 0.8 | `act` (reversible, low-stakes only) |
| ≥ 0.5 | `confirm` |
| otherwise | `human` |

These are starting bars. The local model is uncalibrated, so tune them on the dry-run or your own labelled data.

## 2. Model-tier routing

Choice criteria name the cheapest tier that can still do the job (`nano`, `fast`, `balanced`, `frontier`, `reasoning`). Map the label to a concrete model id in code, and never ask Jev to generate the id as text. If confidence is below 0.55 and the pick isn't `nano`, fall back to `balanced`. Allternit's own tier map lives in the `model_route` MCP tool.

## 3. `route-model` helper (OpenRouter, paid, opt-in)

`system-one route-model --task "..." --allow-paid` sends the task to OpenRouter's `typesafe/jev-router` using `OPENROUTER_API_KEY`, and returns the model OpenRouter reports it routed to. It's a paid call: it refuses without `--allow-paid` or `allowPaid: true`, and it only runs when a human asked for it. Its unit tests use a mocked HTTP layer.
