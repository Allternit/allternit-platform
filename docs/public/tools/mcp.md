# MCP Integration

Allternit can act as an **MCP client** (consuming external MCP servers through the SDK Tool Belt) and as an **MCP server** (exposing its own tool registry to external MCP clients). Both directions share the same tool definitions and execution paths so a capability only needs to be documented once.

## Attaching an MCP server to the Tool Belt

The SDK's `NativeToolBelt.attachMcpServer()` discovers an MCP server's tools and registers each one as a namespaced, model-facing tool in the active `ToolRegistry`.

```typescript
import { ToolRegistry } from '@allternit/sdk/ai-runtime/tools';
import { NativeToolBelt } from '@allternit/sdk/ai-runtime/tools/search';

const registry = new ToolRegistry();
const belt = new NativeToolBelt(registry);

const names = await belt.attachMcpServer({
  serverId: 'docs',
  listTools: async () => [
    {
      name: 'read',
      description: 'Read a document',
      inputSchema: {
        type: 'object',
        properties: { id: { type: 'string' } },
        required: ['id'],
      },
    },
  ],
  callTool: async (name, args) => {
    return { content: `Read ${name} with ${JSON.stringify(args)}` };
  },
});

console.log(names); // => ['docs.read']
const tool = registry.getTool('docs.read');
console.log(tool?.input_schema.additionalProperties); // => false
```

Each attached tool is registered under the namespace `<serverId>.<toolName>` with `strict: true` so the schema is closed. The execution proxy simply forwards the call to the server's `callTool` method.

## Bundled and remote MCP servers

`cmd/gizzi-code` ships a bundled MCP catalog in `src/runtime/tools/mcp/bundled.ts`. Bundled servers are merged with user-defined config at startup; user config overrides any bundled entry with the same key.

| Server key | Source | Purpose |
|------------|--------|---------|
| `sequential-thinking` | npm package | Structured reasoning steps |
| `context7` | npm package | Document retrieval and context |
| `allternit-connectors` | Rust API `/internal/connectors/mcp` | Per-user actions on connected apps |
| `allternit-tools` | Rust API `/internal/tools/mcp` | Platform tool registry over MCP |

Remote servers are configured as `type: "remote"` with a URL and optional headers:

```json
{
  "mcpServers": {
    "allternit-tools": {
      "type": "remote",
      "url": "http://127.0.0.1:8013/internal/tools/mcp",
      "headers": {
        "x-allternit-internal-token": "...",
        "x-allternit-user-id": "local-dev-user"
      },
      "oauth": false
    }
  }
}
```

The platform also discovers public MCP servers through the remote directory at `https://registry.modelcontextprotocol.io/v0.1/servers`. Results are merged with the local catalog and the Allternit verified registry so users can browse and connect servers from a single UI.

## Server-side tool execution mode

`cmd/allternit-api` exposes the same tool registry over MCP at `/mcp/server`. The endpoint speaks JSON-RPC 2.0 with a single request object per call and supports:

- `initialize`
- `ping`
- `tools/list`
- `tools/call`

Example `tools/call` request:

```bash
curl -X POST http://127.0.0.1:8013/mcp/server \
  -H "Authorization: Bearer <clerk-token>" \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "tools/call",
    "params": {
      "name": "file.read",
      "arguments": { "path": "README.md" }
    }
  }'
```

The internal route `/internal/tools/mcp` uses the same handler but is gated by a static internal token (`x-allternit-internal-token`) and an explicit user ID header (`x-allternit-user-id`). This is the path `allternit-tools` and `allternit-connectors` use from gizzi-code.

## MCP tunnel security

The public `/mcp/server` endpoint can be fronted by an MCP tunnel with optional mTLS and OAuth checks. The policy is stored in `mcp_tunnel_auth` and is **fail-open when no row exists** for backward compatibility; once a row exists, it is **fail-closed**.

Policy fields:

| Field | Purpose |
|-------|---------|
| `tunnel_id` | Identifier passed by the tunnel in `x-allternit-tunnel-id` |
| `client_cert_pem` | PEM bundle of the trusted client certificate |
| `oauth_issuer` | Allowed `iss` claim from the OAuth token |
| `audience` | Allowed `aud` claim (optional) |

Headers consumed by `mcp_server_routes.rs`:

| Header | Meaning |
|--------|---------|
| `x-allternit-tunnel-id` | Selects the tunnel policy |
| `x-allternit-client-cert-thumbprint` | SHA-256 thumbprint presented by the mTLS terminator |
| `x-allternit-oauth-issuer` | `iss` claim from the OAuth bearer token |
| `x-allternit-oauth-audience` | `aud` claim from the OAuth bearer token |

Only the gateway (the mTLS terminator) may set these headers. They are believed only when the same request carries the internal service token in `x-allternit-internal-token`; a request that carries any of them (or any `x-allternit-tunnel-*` header) without it gets HTTP `401`, JSON-RPC `-32001`, `tunnel headers are set by the gateway only`. Requests with no tunnel headers run as the signed-in user, unchanged. A policy that pins a certificate or issuer can only be satisfied through the gateway.

The thumbprint is computed as the uppercase, colon-free SHA-256 fingerprint of the first certificate in the PEM bundle, matching `openssl x509 -sha256 -fingerprint`.

```json
{
  "tunnel_id": "prod-tunnel-1",
  "client_cert_pem": "-----BEGIN CERTIFICATE-----\nMIICqDCCAZACCQCPLNNrnTDMTDANBgkqhkiG9w0BAQsFAD...",
  "oauth_issuer": "https://issuer.example/",
  "audience": "allternit-mcp"
}
```

A request that fails tunnel auth receives a JSON-RPC error with code `-32001` and HTTP 401:

```json
{
  "jsonrpc": "2.0",
  "id": null,
  "error": {
    "code": -32001,
    "message": "MCP tunnel auth failed: missing client certificate"
  }
}
```

> **Current scope:** the scaffold validates header-supplied thumbprints, issuer strings, and audience strings. Live TLS client certificate extraction, JWKS signature verification, and admin CRUD endpoints for tunnel policies are tracked for future work.
