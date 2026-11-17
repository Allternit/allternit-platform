/**
 * Context left (Primitives Wave B, lane L4): the `get_context_remaining`
 * core and the source of `/context`'s `compactions`.
 *
 * Uses the same accounting as the "Context window" panel
 * (SessionContextUsage → ContextBreakdown, reconciled with the provider's
 * reported usage) and the same compaction budget auto-compaction uses
 * (SessionCompaction.bufferTokens), so the tool, the panel and the notice
 * never disagree.
 */
import { Provider } from "@/runtime/providers/provider"
import { SessionCompaction } from "@/runtime/session/compaction"
import { SessionContextUsage } from "@/runtime/session/context-usage"
import type { ContextRemainingResult } from "@/runtime/primitives/wave-b-types"

export namespace ContextRemaining {
  /**
   * The pure arithmetic, shared with ContextNotice. `buffer` is what
   * auto-compaction keeps free (0 when it is off, so nothing compacts and
   * until_compaction is just what is left).
   */
  export function compute(input: { window: number; used: number; buffer: number; compactions: number }): ContextRemainingResult {
    const window = Math.max(0, Math.round(input.window))
    const used = Math.max(0, Math.round(input.used))
    const remaining = Math.max(0, window - used)
    const until_compaction = input.buffer > 0 ? Math.max(0, window - Math.round(input.buffer) - used) : remaining
    return { window, used, remaining, until_compaction, compactions: Math.max(0, input.compactions) }
  }

  /** Tokens for this session against `model`'s window; `model` defaults to the session's last model. */
  export async function read(input: { sessionID: string; model?: Provider.Model }): Promise<ContextRemainingResult> {
    const usage = await SessionContextUsage.read(input.sessionID, { limits: false })
    const breakdown = usage.breakdown
    const model =
      input.model ??
      (breakdown ? await Provider.getModel(breakdown.providerID, breakdown.modelID).catch(() => undefined) : undefined)

    // A brain that measures itself (Claude Code, Codex) reports its own
    // window and buffer; otherwise the model's window and our compaction budget.
    const brainMeasured = breakdown?.source === "brain"
    const window = brainMeasured ? breakdown.contextWindow : (model?.limit.context ?? breakdown?.contextWindow ?? 0)
    const buffer = brainMeasured
      ? breakdown.autoCompactEnabled
        ? breakdown.autoCompactBuffer
        : 0
      : model
        ? await SessionCompaction.bufferTokens(model)
        : breakdown?.autoCompactEnabled
          ? breakdown.autoCompactBuffer
          : 0

    return compute({ window, used: breakdown?.usedTokens ?? 0, buffer, compactions: usage.compactions })
  }
}
