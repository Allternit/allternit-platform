/**
 * connector_search and connector_suggest (Primitives Wave D, D2). Core in
 * runtime/primitives/connectors.ts. Every client; permission allow
 * (loop/agent.ts). Tools a `load` mounts keep their own permission rules.
 * The suggestion card rides in the tool_result metadata (`card`), as in
 * Wave A; the Connect kit renders it and the user's tap connects. The card
 * carries ids, names, groups and the reason only: never a link or a code.
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
      const loaded = r.loaded ?? []
      const notes: string[] = []
      if (params.load && !loaded.length) {
        notes.push(
          r.hits.some((h) => h.state === "connected")
            ? "Nothing new to mount: the connected matches are reached through execute_action, or their tools are already listed."
            : "Nothing was mounted: none of the matches is connected. Offer one with connector_suggest.",
        )
      }
      if (loaded.length) notes.push(`Mounted ${loaded.length} tools for the rest of this session; they are callable from your next step.`)
      return {
        title: loaded.length ? `Loaded ${loaded.length} connector tools` : `Connectors: ${params.query}`,
        output: JSON.stringify({ ...r, ...(notes.length ? { note: notes.join(" ") } : {}) }),
        metadata: {
          query: params.query,
          hits: r.hits.length,
          loaded,
          connectors: r.hits.map((h) => ({ id: h.id, name: h.name, group: h.group, state: h.state })),
        },
      }
    } catch (error) {
      return {
        title: "Connector search unavailable",
        output: messageOf(error),
        metadata: { query: params.query, hits: 0, loaded: [] as string[], connectors: [] as Array<Pick<Connectors.Hit, "id" | "name" | "group" | "state">> },
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
      const names = r.card?.connectors.map((c) => c.name) ?? []
      return {
        title: `Suggested ${names.join(", ")}`,
        output: JSON.stringify({ shown: r.shown, ...(r.dropped.length ? { dropped: r.dropped } : {}) }),
        metadata: { shown: r.shown, reason: r.card?.reason ?? params.reason, card: r.card },
      }
    } catch (error) {
      return {
        title: error instanceof Connectors.RefusedError ? "Connector suggestion refused" : "Connector suggestion unavailable",
        output: messageOf(error),
        metadata: { shown: [] as string[], reason: params.reason, card: undefined as Connectors.SuggestCard | undefined },
      }
    }
  },
})
