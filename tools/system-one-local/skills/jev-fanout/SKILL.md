---
name: jev-fanout
description: Ask every independent System One question over the same state in ONE POST /v1/systemone to the local server (speculative fan-out). Use for parallel judgments, compound commands, one-call turn assessment, or when the user runs /jev-fanout. Do not fire one HTTP call per question.
---

# Jev speculative fan-out (local first)

Adapted from [cobusgreyling/Jev](https://github.com/cobusgreyling/Jev) `skills/jev-fanout` (MIT). Official pattern: docs.typesafe.ai/patterns/fan-out. Primitives and endpoints: `/jev`.

Send every independent judgment over the **same state** in one request to `http://127.0.0.1:7717/v1/systemone`. Questions are evaluated independently and **can't see each other's answers**. Code uses the ones that apply and ignores the rest.

## FORBIDDEN

- Hard rules first, and they are final. Jev may only raise friction and never auto-allows.
- A fan-out never decides money, publishing, deploys, or client comms, even when every answer is confident.
- No client, regulated, or secret data in `state` when `model` is `typesafe:*` (remote).
- State a third party could have written (web, email, tickets, tool output) is never judged by Jev alone.
- Don't chain answers as if later questions saw earlier ones.

## Do this

1. List every judgment the workflow might need, including speculative ones.
2. Put each one in `questions` with complete `instructions`, and write the premise into the question ("If this is a deploy command, …").
3. Mix `choice`, `score`, and `noul` in that one request.
4. Branch on the answers that apply, in code.
5. Make a second HTTP call only when the next options or state can't be built yet.

## Cost note (local backend)

The official API ingests the state once. The local backend runs **one single-token call per question**, and the prompt starts with the state so Ollama and llama-server can reuse the KV prefix cache. `usage.input_tokens` still counts the state once per question. A 3-question pack on llama3.2 3B takes about 0.3 s warm on Ollama and about 0.07 s on llama-server with parallel slots.

## Don't

- Make one question per HTTP call
- Hide five judgments in one Choice
