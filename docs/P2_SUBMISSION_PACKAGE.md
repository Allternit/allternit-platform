# Allternit Plugin — Submission Package (P2)

Package name: **Allternit Plugin**. UI surface: **MCP App**. Server: `POST /mcp/server`
in `allternit-api`, published at `MCP_PUBLIC_URL` (default `https://mcp.allternit.com/mcp`).
Standard: MCP Apps (SEP-1865) + the OpenAI plugin format.

## 1. Plugin manifest (Agent Plugins `mcp.json`, streamable-http)

```json
{
  "mcpServers": {
    "allternit-agents": {
      "type": "streamable-http",
      "url": "https://mcp.allternit.com/mcp"
    }
  }
}
```

Auth is discovered, not configured: an unauthenticated request returns
`401` with `WWW-Authenticate: Bearer resource_metadata="https://mcp.allternit.com/.well-known/oauth-protected-resource/mcp"`.
That document lists `authorization_servers: ["https://allternit.com/__clerk"]` and
`scopes_supported: ["agents:read"]`. Clerk handles registration (CIMD) and issues
tokens with `aud` = the MCP URL.

Tools (all read-only: `readOnlyHint: true`, `destructiveHint: false`, `openWorldHint: false`):
`list_agents`, `get_agent`, `list_runs`, `get_run`, `get_run_result`, `render_run_status`.
MCP App resource: `ui://allternit/run-status.v1.html` (`text/html;profile=mcp-app`).

## 2. Test cases

### Positive (5)

| # | Prompt | Expected tool | Expected result |
|---|--------|---------------|-----------------|
| 1 | "List my Allternit agents." | `list_agents` | Array of the caller's agents with id, name, status, model. |
| 2 | "Show me the details of agent `<agent id>`." | `get_agent` | One agent record; only the caller's agents are returned. |
| 3 | "Which of my runs failed?" | `list_runs` with `status: "failed"` | Failed runs, newest first, each with run_id, agent, timestamps. |
| 4 | "What's the status of run `<run id>`?" | `get_run` | Status, started/completed timestamps, duration. No output body. |
| 5 | "Show me run `<run id>` as a status card." | `render_run_status` | Structured run data plus the run-status MCP App rendered inline. |

Also verify: "What did run `<run id>` produce?" → `get_run_result` returns the output text (truncated and flagged past 20,000 characters).

### Negative (3)

| # | Prompt | Expected behaviour |
|---|--------|--------------------|
| 1 | "Start a new run of my support agent." | No tool called. The plugin has no write tools; the assistant says it can only read agents and runs. |
| 2 | "Delete agent `<id>`." | No tool called. Same reason. |
| 3 | "Show run `<run id belonging to another account>`." | `get_run` is called and returns `Run not found` (`isError: true`). Nothing about the other account is disclosed. |

Auth checks for the reviewer: a token with the wrong `aud` returns 401 (`invalid_token`); a token without `agents:read` returns 403 (`insufficient_scope`); both carry the `WWW-Authenticate` challenge.

## 3. Listing copy (Register 1)

**Name:** Allternit Plugin

**Short description:** Look up your Allternit agents and their runs from chat.

**Long description:**
Allternit Plugin gives your assistant read access to your Allternit account. Ask it to list your agents, find runs by agent or status, check when a run started and finished, and read a finished run's output. A run status card can be shown inline. The plugin only reads; it does not start, change, or delete anything. You sign in with your Allternit account and approve the `agents:read` permission.

**Privacy / data note:** Requests return only the signed-in user's own agents and runs. Agent system prompts and configuration are not returned.

## 4. Icon and screenshot checklist

- [ ] Icon: square, transparent-or-white background, exported at 512×512 and 1024×1024 PNG; legible at 32 px; Gizzi mark, no text.
- [ ] Screenshot 1: assistant listing agents (`list_agents`).
- [ ] Screenshot 2: run status card rendered inline (`render_run_status`), light theme.
- [ ] Screenshot 3: same card, dark theme.
- [ ] Screenshot 4: consent screen showing the `agents:read` scope.
- [ ] All screenshots use a test account with non-sensitive sample data; white / `--neutral-fill` surfaces only (no tan).
- [ ] No pricing, ranking, or comparison claims in any image.

## 5. Eoj-only items

- [ ] OpenAI organization verification for the publishing org.
- [ ] Demo video (walkthrough of positive cases 1, 3, 5 and the negative cases).
- [ ] Reviewer test account: an Allternit account with sample agents and runs; credentials go into the submission form only, never into the repo.
- [ ] Domain verification token (OpenAI-issued) served from the MCP domain.
- [ ] Deploy: route `mcp.allternit.com/mcp` to `POST /mcp/server` and `mcp.allternit.com/.well-known/oauth-protected-resource*` to the public metadata routes; set `MCP_PUBLIC_URL` if it differs from the default.
- [ ] Clerk dashboard: confirm OAuth application with scope `agents:read`, CIMD on, "Include Audience" on (audience = the MCP URL).
- [ ] Final human approval of listing copy before submission.
