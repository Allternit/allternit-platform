/**
 * connector_search and connector_suggest (Primitives Wave D, D2). Core in
 * runtime/primitives/connectors.ts. Every client; permission allow
 * (loop/agent.ts). Tools a `load` mounts keep their own permission rules.
 * The suggestion card rides in the tool_result metadata (`card`), as in
 * Wave A; the Connect kit renders it and the user's tap connects.
 *
 * W2 stub: final ids and parameters; lane C fills the execute bodies.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { Connectors } from "@/runtime/primitives/connectors"
import SEARCH_DESCRIPTION from "@/runtime/tools/builtins/connector-search.txt"
import SUGGEST_DESCRIPTION from "@/runtime/tools/builtins/connector-suggest.txt"

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

export const ConnectorSearchTool = Tool.define("connector_search", {
  description: SEARCH_DESCRIPTION,
  parameters: Connectors.SearchInput,
  async execute(params, ctx) {
    try {
      const r = await Connectors.search(ctx.sessionID, params, ctx.abort)
      return {
        title: r.loaded?.length ? `Loaded ${r.loaded.length} connector tools` : `Connectors: ${params.query}`,
        output: JSON.stringify(r),
        metadata: { query: params.query, hits: r.hits.length, loaded: r.loaded ?? [] },
      }
    } catch (error) {
      return {
        title: "Connector search unavailable",
        output: messageOf(error),
        metadata: { query: params.query, hits: 0, loaded: [] as string[] },
      }
    }
  },
})

export const ConnectorSuggestTool = Tool.define("connector_suggest", {
  description: SUGGEST_DESCRIPTION,
  parameters: Connectors.SuggestInput,
  async execute(params, ctx) {
    try {
      const r = await Connectors.suggest(ctx.sessionID, params)
      return {
        title: r.shown.length ? `Suggested ${r.shown.join(", ")}` : "No connector to suggest",
        output: JSON.stringify(r),
        metadata: { shown: r.shown, reason: params.reason },
      }
    } catch (error) {
      return {
        title: "Connector suggestion unavailable",
        output: messageOf(error),
        metadata: { shown: [] as string[], reason: params.reason },
      }
    }
  },
})
