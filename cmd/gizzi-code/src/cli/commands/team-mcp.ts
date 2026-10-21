import type { CommandModule } from "yargs"
import { serveStdio } from "@/runtime/live/team-mcp"

/** `gizzi team-mcp`: the stdio MCP server with `list_team`, `send_message` and `check_inbox`. */
export const TeamMcpCommand: CommandModule<{}, {}> = {
  command: "team-mcp",
  describe: "MCP server (stdio) with list_team, send_message and check_inbox for any MCP client",
  handler: async () => {
    await serveStdio()
  },
}
