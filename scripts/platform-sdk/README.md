# Platform SDK generator

`generate.py` reads `cmd/allternit-cloud-api/openapi/platform-v1.yaml` and writes the typed layer of both SDKs:

- `sdk/platform-ts/src/generated/{types.ts,operations.ts}`
- `sdk/platform-python/allternit_platform/_generated.py`

Run it after any change to the yaml (`--check` fails when the output is stale; both SDK test suites run it). Needs Python 3.9+ with PyYAML. Operations are grouped by their first OpenAPI tag into a resource (`client.agents`, `client.conversations`, …); method names drop the resource noun from the operationId (`createAgent` → `agents.create`). New tags become new client properties with no hand-written change.

## Other specs

`--spec`, `--out` and `--out-py` point the generator at another OpenAPI file. The self-hosted Agents Server client is made this way:

```bash
python3 scripts/platform-sdk/generate.py \
  --spec cmd/allternit-api/openapi/selfhost-v1.yaml \
  --out sdk/platform-ts/src/selfhost \
  --out-py sdk/platform-python/allternit_platform/_selfhost_generated.py \
  --client-name SelfHostClient
```

- `--out` is a folder directly under `sdk/platform-ts/src` (the generated `operations.ts` imports `../core.ts`); `--out-py` is a module in `sdk/platform-python/allternit_platform` (it imports `._core`). Relative paths are taken from the repo root.
- `--spec` without both outputs is refused, so the Platform files are never overwritten by another spec.
- Operations marked `x-internal: true` are skipped (the selfhost loopback route).
- An operation that can answer `204` returns `undefined` / `None` for it (long-poll routes such as work claims).
- When a component schema already has an operation alias's name (`CreateAgentRequest`), the alias reuses the schema, or is named `<Op>Op<Kind>` if the two differ.
- Run with `--check` to see whether the files are stale. Both SDK test suites check both outputs.
