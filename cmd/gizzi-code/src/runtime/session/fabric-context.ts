import { ContextBreakdown } from "@/runtime/session/context-breakdown"
import { ContextBreakdownStore } from "@/runtime/session/context-breakdown-store"

/**
 * A signed-in app brain's context (ChatGPT, Claude, Kimi… in the user's own
 * browser): the gateway reads the vendor's conversation, memory, project
 * and usage endpoints in the page after each turn and returns a report with
 * the task result. Rows the vendor hides (its own system prompt and tool
 * schemas) arrive as 0 and are named in `hidden`.
 */
export namespace FabricContext {
  export interface Report {
    vendor: string
    model?: string
    context_window?: number
    rows: ContextBreakdown.Parts
    memory_files: Array<{ path: string; tokens: number }>
    mcp_tools: Array<{ name: string; server: string; tokens: number }>
    system_tools: Array<{ name: string; tokens: number }>
    skills: Array<{ name: string; tokens: number }>
    hidden: string[]
    limits?: Array<{ id: string; label: string; used_ratio: number; resets_at: string | null }>
  }

  export function snapshot(input: { sessionID: string; providerID: string; modelID: string; contextWindow: number; report: Report }) {
    const { report } = input
    const window = report.context_window && report.context_window > 0 ? report.context_window : input.contextWindow
    if (!(window > 0)) return undefined
    const snapshot = ContextBreakdown.fromParts({
      sessionID: input.sessionID,
      providerID: input.providerID,
      modelID: report.model ?? input.modelID,
      contextWindow: window,
      parts: report.rows,
      memoryFiles: report.memory_files,
      mcpTools: report.mcp_tools,
      systemTools: report.system_tools,
      skills: report.skills,
      basis: "estimated",
    })
    return { ...snapshot, hidden: report.hidden.filter((id) => ContextBreakdown.CategoryID.safeParse(id).success) as ContextBreakdown.CategoryID[] }
  }

  export async function record(input: { sessionID: string; providerID: string; modelID: string; report: Report }) {
    const contextWindow = await import("@/runtime/providers/provider")
      .then(({ Provider }) => Provider.getModel(input.providerID, input.modelID))
      .then((model) => model.limit.context)
      .catch(() => 0)
    const next = snapshot({ ...input, contextWindow: contextWindow || 200_000 })
    if (next) ContextBreakdownStore.record(next)
    // The page's own usage windows feed the panel's plan limits.
    if (input.report.limits?.length) {
      const { ProviderQuotas } = await import("@/runtime/providers/quota")
      ProviderQuotas.record(
        input.providerID,
        input.report.limits.map((w) => ({ id: w.id, label: w.label, usedRatio: w.used_ratio, ...(w.resets_at ? { resetAt: w.resets_at } : {}) })),
      )
    }
  }
}
