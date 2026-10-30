# Allternit phones add-on

Let a coding agent drive your Allternit phones, phone numbers and mailboxes: take a screenshot
and tap, run a goal on a phone, replay a saved phone skill, run one job on several phones, give an
agent its own number and mailbox, and read one-time codes.

Everything goes through Allternit's remote MCP server:

| | |
|---|---|
| URL | `https://mcp.allternit.com/mcp` (streamable HTTP) |
| Auth | `Authorization: Bearer alt_live_...` (a Platform API project key) |
| Scopes | `phones`, `numbers`, `messaging`, `mail` (only the ones you need) |

This folder holds:

- a Claude Code plugin (`.claude-plugin/plugin.json`, `.mcp.json`, `skills/`, `commands/`)
- a Codex skill (`codex/allternit-phones/SKILL.md`)

## 1. Get a key

1. Open platform.allternit.com, API Keys, Platform API.
2. Pick a project (sandbox for testing: `alt_test_...`, simulated numbers; live: `alt_live_...`).
3. Create key with the scopes above. Leave **account** empty: phones belong to the project owner,
   and an account-bound key is refused.
4. Put it in your shell profile, not in a repo:

```bash
export ALLTERNIT_API_KEY=alt_live_...
```

Your phones must be linked to Allternit Desktop (Phone Link, adb or a USB Agent), and Desktop must
be open and signed in for phone tools to work. Texts, calls and reading texts always ask for your
approval on Desktop.

## 2. Claude Code

```text
/plugin marketplace add Allternit/allternit-platform
/plugin install allternit-phones@allternit
```

Restart Claude Code with `ALLTERNIT_API_KEY` set. The plugin registers the `allternit` MCP server,
the `allternit-phones` skill, and two commands:

- `/allternit-phones:phone [goal]`: screenshot, drive, or run a goal on a phone
- `/allternit-phones:phone-otp [mailbox or number]`: read the newest one-time code

Without the plugin, add only the server:

```bash
claude mcp add --transport http allternit https://mcp.allternit.com/mcp \
  --header "Authorization: Bearer $ALLTERNIT_API_KEY"
```

(That command writes the key itself into your Claude config. The plugin keeps `${ALLTERNIT_API_KEY}`
as a reference instead.)

## 3. Codex

Add the server to `~/.codex/config.toml`. Codex reads the token from the environment variable at
start-up, so the key never sits in the file:

```toml
[mcp_servers.allternit]
url = "https://mcp.allternit.com/mcp"
bearer_token_env_var = "ALLTERNIT_API_KEY"
```

Or run `codex mcp add allternit --url https://mcp.allternit.com/mcp --bearer-token-env-var ALLTERNIT_API_KEY`
(Codex CLI 0.165 and later). Check it with `codex mcp list`.

Then install the skill:

```bash
mkdir -p ~/.codex/skills
cp -R codex/allternit-phones ~/.codex/skills/
```

## 4. Other MCP clients (Cursor, Windsurf, VS Code, and others)

Any client that speaks streamable HTTP with custom headers works. Cursor (`~/.cursor/mcp.json` or
`.cursor/mcp.json`):

```json
{
  "mcpServers": {
    "allternit": {
      "url": "https://mcp.allternit.com/mcp",
      "headers": { "Authorization": "Bearer ${env:ALLTERNIT_API_KEY}" }
    }
  }
}
```

For a client that only speaks stdio, bridge it with `mcp-remote`:

```json
{
  "mcpServers": {
    "allternit": {
      "command": "npx",
      "args": ["-y", "mcp-remote", "https://mcp.allternit.com/mcp",
               "--header", "Authorization: Bearer ${ALLTERNIT_API_KEY}"],
      "env": { "ALLTERNIT_API_KEY": "alt_live_..." }
    }
  }
}
```

Copy the skill text from `skills/allternit-phones/SKILL.md` into the client's rules if it has no
skills.

## Tools

`phones_list`, `phone_screenshot`, `phone_tap`, `phone_type`, `phone_open_app`, `phone_task`,
`phone_run_status`, `phone_skill_run`, `phone_history`, `phone_live_url`, `mailbox_create`,
`mailbox_list`, `mail_list`, `mail_read`, `mail_read_otp`.

The same key works on the REST API at `https://api.allternit.com/v1`: `/v1/phones`,
`/v1/phones/runs` (fleet runs), `/v1/numbers`, `/v1/messages`, `/v1/mailboxes`, and
`/v1/identities` (number, mailbox and phone in one call).

Docs: https://docs.allternit.com/api/platform/sdks/claude-code-codex
