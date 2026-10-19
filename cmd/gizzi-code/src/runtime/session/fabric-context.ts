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
  /**
   * Estimates for the rows a signed-in app hides (tokens, chars/4), from the
   * best public measure of each vendor's own prompt:
   * - Claude: Anthropic's published claude.ai system prompts
   *   (platform.claude.com/docs/en/release-notes/system-prompts, latest
   *   entry per model, Oct 2026); built-in tools ≈ 10.3k = the ~15k full
   *   claude.ai prompt measured for Claude 4.6 (Adversa AI, Feb 2026) minus
   *   that model's 4.7k published core, plus the documented 286-token
   *   tool-use system prompt.
   * - ChatGPT: the leaked ChatGPT prompts (github.com/asgeirtj/
   *   system_prompts_leaks, gpt-5.6-sol thinking / gpt-5.5-instant), core
   *   prompt and each tool namespace measured.
   */
  const CLAUDE_PROMPT: Array<[RegExp, number]> = [
    [/opus-5-5|opus-5\.5/, 6_780],
    [/sonnet-5-5|sonnet-5\.5/, 5_390],
    [/haiku-5-5|haiku-5\.5/, 6_880],
    [/fable/, 6_990],
  ]
  const CLAUDE_PROMPT_DEFAULT = 6_800
  const CLAUDE_TOOLS = 10_300 + 286

  const CHATGPT_THINKING = {
    prompt: 2_570,
    tools: { web: 9_785, canmore: 10_526, automations: 2_175, api_tool: 1_093, personal_context: 886, bio: 773, genui: 741, image_gen: 700, python_user_visible: 603, container: 358, summary_reader: 344, python: 298, local: 298, user_settings: 229, user_info: 151, hotline: 75 },
  }
  const CHATGPT_INSTANT = {
    prompt: 6_030,
    tools: { web: 6_266, user_settings: 4_234, file_search: 667, personal_context: 362, python_user_visible: 351, bio: 326, image_gen: 324, api_tool: 278, container: 256, python: 239 },
    connectors: { gmail: 914, gcal: 716, gcontacts: 138 },
  }

  /** Other signed-in apps: core prompt and built-in tools measured from the same leak collection. */
  const OTHER_APPS: Record<string, { prompt: number; tools: number }> = {
    // Google/gemini-3.8-flash.md: core before "# Tools", then the tool section.
    google: { prompt: 8_333, tools: 6_081 },
    // Kimi/kimi-3.md: core prompt, then its JSON tool definitions.
    kimi: { prompt: 8_286, tools: 685 },
  }

  /** Fill the hidden rows with the vendor estimates above; returns which rows were estimated. */
  export function estimateHidden(report: Report): { report: Report; estimated: string[] } {
    const hidden = new Set(report.hidden)
    const rows = { ...report.rows }
    const estimated: string[] = []
    let systemTools = report.system_tools
    const model = (report.model ?? "").toLowerCase()
    if (report.vendor === "claude") {
      if (hidden.has("system_prompt")) {
        rows.system_prompt = CLAUDE_PROMPT.find(([pattern]) => pattern.test(model))?.[1] ?? CLAUDE_PROMPT_DEFAULT
        estimated.push("system_prompt")
      }
      if (hidden.has("system_tools")) {
        rows.system_tools = CLAUDE_TOOLS
        systemTools = [...systemTools.filter((t) => t.tokens > 0), { name: "claude.ai built-in tools (estimated)", tokens: CLAUDE_TOOLS }]
        estimated.push("system_tools")
      }
    } else if (report.vendor === "chatgpt") {
      const table = /instant|mini|chat-latest|-chat/.test(model) ? CHATGPT_INSTANT : CHATGPT_THINKING
      if (hidden.has("system_prompt")) {
        rows.system_prompt = table.prompt
        estimated.push("system_prompt")
      }
      if (hidden.has("system_tools")) {
        systemTools = Object.entries(table.tools).map(([name, tokens]) => ({ name, tokens }))
        rows.system_tools = systemTools.reduce((sum, t) => sum + t.tokens, 0)
        estimated.push("system_tools")
      }
      if (hidden.has("mcp_tools") && "connectors" in table) {
        rows.mcp_tools = Object.values(table.connectors).reduce((sum, t) => sum + t, 0)
        report = { ...report, mcp_tools: Object.entries(table.connectors).map(([name, tokens]) => ({ name, server: name, tokens })) }
        estimated.push("mcp_tools")
      }
    } else if (OTHER_APPS[report.vendor]) {
      const table = OTHER_APPS[report.vendor]
      if (hidden.has("system_prompt")) {
        rows.system_prompt = table.prompt
        estimated.push("system_prompt")
      }
      if (hidden.has("system_tools")) {
        rows.system_tools = table.tools
        systemTools = [...systemTools.filter((t) => t.tokens > 0), { name: `${report.vendor} built-in tools (estimated)`, tokens: table.tools }]
        estimated.push("system_tools")
      }
    }
    return {
      report: { ...report, rows, system_tools: systemTools, hidden: report.hidden.filter((id) => !estimated.includes(id)) },
      estimated,
    }
  }

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
    const { report, estimated } = estimateHidden(input.report)
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
    const ids = (list: string[]) => list.filter((id) => ContextBreakdown.CategoryID.safeParse(id).success) as ContextBreakdown.CategoryID[]
    return { ...snapshot, hidden: ids(report.hidden), estimatedRows: ids(estimated) }
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
