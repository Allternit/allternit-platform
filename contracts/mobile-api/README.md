# mobile-api: the native Android app's generated API client

`generate.mjs` turns OpenAPI operations into Kotlin for `:core:api`, so the native app
never hand-writes the REST clients iOS has (`ios/Core/API/*Client.swift`).

| Input | What it is |
|---|---|
| `cmd/allternit-api/openapi/app-v1.yaml` | First-party app routes on allternit-api (`/me`, `/agent-sessions`, `/models`, `/providers`) and the instance route `/api/agent-chat` |
| `cmd/allternit-cloud-api/openapi/device-tokens.yaml` | cloud-api push token routes |
| `operations.json` | The allowlist: which `operationId`s are generated, grouped by plan phase |

Output (never edit by hand), in `surfaces/allternit-mobile/android/core/api/src/main/java/com/allternit/core/api/gen/`:

- `Models.gen.kt`: `@Serializable` data classes (wire names via `@SerialName`, optional or nullable → `T? = null`), enums with an `Unknown` fallback.
- `Api.gen.kt`: `AllternitApi(client, config, json)` with one `suspend fun` per operation returning `ApiResult<T>`. Hand-written runtime: `ApiCall.kt` (`ApiResult`, `ApiJson`, error-body parsing).

These are app routes, not the public developer API. `platform-v1.yaml` is pinned to `/v1` routes by the
`openapi_matches_router` test in cloud-api and is published to the docs site, and `selfhost-v1.yaml` is
the customer-run Agents Server. So app routes go in the files above, never there.

## Add an endpoint

1. Read the Rust handler. Copy its exact wire shape into the spec: field names as serialized (check
   `#[serde(rename_all)]`, `rename`, `alias`), `required` only for fields the handler always sends, and
   `type: [string, "null"]` for `Option` fields serialized as `null`. Put `x-handler: <file> <fn>` on the operation.
2. Set `x-base` on the operation: `api` (path relative to `ApiConfig.apiBaseUrl`, `…/api/v1`), `cloud`
   (absolute path on `ApiConfig.cloudBaseUrl`) or `instance` (absolute path on the user's computer; the
   function takes `instanceBaseUrl`).
3. Name every object schema under `components/schemas` (inline objects are refused). For a field whose
   shape the server does not own (a gizzi passthrough, an open metadata bag, `#[serde(untagged)]`), use
   `x-kotlin-raw: true` → `JsonElement`, with a `description` saying why.
4. Streams: `x-stream: sse` generates only `<operationId>Request(...)`; read it in `:core:stream`
   (see `AgentChatStream.kt`).
5. Add the `operationId` to `operations.json`, run `node contracts/mobile-api/generate.mjs`, and commit
   the spec, the allowlist and both `.gen.kt` files together. `--check` (in `scripts/android-check.sh`)
   fails when they drift.
6. Add a decode test with a fixture copied from the handler (or its Rust test) to
   `core/api/src/test/.../GeneratedModelsTest.kt`, naming the source.

The YAML reader in `generate.mjs` handles the subset these specs use (block maps and lists, flow `{}`/`[]`,
quoted and plain scalars, `|` blocks). It throws, with the line, on anything else.
