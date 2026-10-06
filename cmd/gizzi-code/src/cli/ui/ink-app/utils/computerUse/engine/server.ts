/**
 * In-process MCP server for the Computer Use engine adapter.
 *
 * Replaces `createComputerUseMcpServer` from `@ant/computer-use-mcp`. The
 * CallTool handler is a stub by design — real dispatch goes through
 * wrapper.tsx's `.call()` override (same pattern as the native package).
 * mcpServer.ts replaces the ListTools handler to enrich the request_access
 * description with installed-app names.
 */

import { Server } from '@modelcontextprotocol/server'

import { buildComputerUseTools } from './tools.js'
import type {
  ComputerUseHostAdapter,
  CoordinateMode,
} from './types.js'

export function createComputerUseMcpServer(
  adapter: ComputerUseHostAdapter,
  coordinateMode: CoordinateMode = 'pixels',
): Server {
  const server = new Server(
    { name: adapter.serverName, version: '0.1.0' },
    { capabilities: { tools: {} } },
  )

  server.setRequestHandler('tools/call', async () => ({
    content: [
      {
        type: 'text' as const,
        text: 'Direct CallTool dispatch is not available on this server; use the Gizzi Code CLI computer-use integration.',
      },
    ],
    isError: true,
  }))

  server.setRequestHandler('tools/list', async () =>
    adapter.isDisabled()
      ? { tools: [] }
      : { tools: buildComputerUseTools(adapter.executor.capabilities, coordinateMode) },
  )

  return server
}
