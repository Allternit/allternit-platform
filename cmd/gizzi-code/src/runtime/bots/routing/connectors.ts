/**
 * Connector search route (Primitives Wave D, D2). Server: allternit-api
 * `agent_connector_tools.rs`. It matches the connector catalog and the
 * caller's own connectors (cloud connectors and their OAuth tokens live in
 * allternit-api; the runtime only ever sees proxied MCP tools). Whose
 * connectors they are comes from the session id, never from model input.
 *
 * - `POST /bot-routing/connector/search` `{sessionId, query, limit}` →
 *   `{hits: [{id, name, group, state, available, reason?, mcpConnectorId?, serverName?}]}`.
 *   `mcpConnectorId` is the MCP connector whose proxied tools belong to a
 *   connected hit (`MCP.ToolDescriptor.connectorId`), for `load`;
 *   `serverName` is the proxy server they mount under
 *   (`mcp__<server>__<connector>__<tool>`). A connected hit without them is
 *   reached through the app connectors' `execute_action` tool.
 *
 * Errors surface as AppRoutingError (status, message, code).
 * Free of runtime state and UI imports so bun tests can stub `fetch`.
 */
import { botRoutingPost } from "@/runtime/bots/app-bot-routing"
import type { Connectors } from "@/runtime/primitives/connectors"

export interface ConnectorSearchHit extends Omit<Connectors.Hit, "tools"> {
  /** The MCP connector whose proxied tools belong to this hit, when connected. */
  mcpConnectorId?: string | null
  /** The proxied MCP server the connector's tools mount under, when connected. */
  serverName?: string | null
}

export async function searchConnectors(
  sessionID: string,
  input: { query: string; limit: number },
): Promise<{ hits: ConnectorSearchHit[] }> {
  return botRoutingPost("/bot-routing/connector/search", { sessionId: sessionID, ...input }, 10_000)
}
