# Platform SDK generator

`generate.py` reads `cmd/allternit-cloud-api/openapi/platform-v1.yaml` and writes the typed layer of both SDKs:

- `sdk/platform-ts/src/generated/{types.ts,operations.ts}`
- `sdk/platform-python/allternit_platform/_generated.py`

Run it after any change to the yaml (`--check` fails when the output is stale; both SDK test suites run it). Needs Python 3.9+ with PyYAML. Operations are grouped by their first OpenAPI tag into a resource (`client.agents`, `client.conversations`, …); method names drop the resource noun from the operationId (`createAgent` → `agents.create`). New tags become new client properties with no hand-written change.
