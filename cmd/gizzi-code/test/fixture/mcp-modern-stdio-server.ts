// A dual-era MCP server over stdio (SDK v2 serveStdio: answers server/discover for
// 2026-07-28 clients and initialize for 2025-era ones).
import { Server } from "@modelcontextprotocol/server"
import { serveStdio } from "@modelcontextprotocol/server/stdio"

serveStdio(() => {
  const server = new Server({ name: "modern-stdio", version: "1.0.0" }, { capabilities: { tools: {} } })
  server.setRequestHandler("tools/list", async () => ({
    tools: [{ name: "echo", inputSchema: { type: "object", properties: {} } }],
  }))
  return server
})
