import {
  getCwdState,
  getSessionId,
  getTotalCostUSD,
  getTotalDuration,
  getTotalInputTokens,
  getTotalOutputTokens,
  getTotalCacheReadInputTokens,
  getTotalCacheCreationInputTokens,
} from '../../bootstrap/state.js'
import { formatDuration, formatTokens } from '../../utils/format.js'
import {
  calculateContextPercentages,
  getContextWindowForModel,
} from '../../utils/context.js'
import { getCurrentUsage } from '../../utils/tokens.js'
import {
  getRuntimeMainLoopModel,
  renderModelName,
} from '../../utils/model/model.js'
import {
  fetchProviderQuota,
  resolveQuotaProviderId,
} from '../../utils/telemetry/providerQuota.js'
import { formatResetsIn } from '../../utils/telemetry/runTelemetryModel.js'
import { reasoningTokensFromMessages } from '../../utils/telemetry/turnSignals.js'
import type { LocalCommandCall } from '../../types/command.js'
import type { Message } from '../../types/message.js'

const PROGRESS_BAR_WIDTH = 24

function renderProgressBar(ratio: number, width: number): string {
  const clamped = Math.max(0, Math.min(1, ratio))
  const filled = Math.round(clamped * width)
  const empty = width - filled
  return '█'.repeat(filled) + '░'.repeat(empty)
}

function severityColor(ratio: number): string {
  if (ratio >= 0.9) return 'red'
  if (ratio >= 0.75) return 'yellow'
  return 'green'
}

/**
 * Plan quota for `model`'s provider, when the provider reports it
 * (ProviderQuotas caches 60s). Providers without a quota source get an
 * honest "not reported" — never a fabricated window. Shared with /context.
 */
export async function planQuotaLines(model: string): Promise<string[]> {
  const lines = ['Plan quota']
  const quotaProviderId = resolveQuotaProviderId(model)
  if (!quotaProviderId) {
    lines.push('  Not reported for this provider.')
    return lines
  }
  const quota = await fetchProviderQuota(quotaProviderId)
  if (!quota || quota.status === 'unsupported') {
    lines.push('  Not reported for this provider.')
  } else if (quota.status !== 'ok') {
    lines.push(`  ${quota.message}`)
  } else if (quota.quota.windows.length === 0) {
    lines.push('  Reported, but no plan windows were included.')
  } else {
    for (const window of quota.quota.windows) {
      const bar = renderProgressBar(window.usedRatio, PROGRESS_BAR_WIDTH)
      const resets = formatResetsIn(window.resetAt)
      lines.push(
        `  ${window.id.padEnd(6)}${bar}  ${Math.round(window.usedRatio * 100)}% used${resets ? ` · resets in ${resets}` : ''}`,
      )
    }
  }
  return lines
}

export const call: LocalCommandCall = async (_args, context) => {
  // TODO(types): AppState has no `messages` field (upstream drift shared with
  // status.tsx/session-info.tsx/statusModel.ts); runtime value is always
  // undefined, so the `?? []` fallback was always in effect.
  const messages = (context.getAppState() as { messages?: Message[] }).messages ?? []
  const model = getRuntimeMainLoopModel({
    permissionMode: context.getAppState().toolPermissionContext.mode,
    mainLoopModel: context.options.mainLoopModel,
    exceeds200kTokens: false,
  })

  const modelDisplay = renderModelName(model)
  const cwd = getCwdState()
  const sessionId = getSessionId()

  const contextWindowSize = getContextWindowForModel(model, undefined)
  const currentUsage = getCurrentUsage(messages)
  const contextPercentages = calculateContextPercentages(
    currentUsage,
    contextWindowSize,
  )

  const totalCost = getTotalCostUSD()
  const totalDuration = getTotalDuration()
  const totalInput = getTotalInputTokens()
  const totalOutput = getTotalOutputTokens()
  const cacheRead = getTotalCacheReadInputTokens()
  const cacheCreation = getTotalCacheCreationInputTokens()

  const lines: string[] = []
  lines.push('')
  lines.push('╭────────────────────────────────────────╮')
  lines.push('│  Usage                                 │')
  lines.push('╰────────────────────────────────────────╯')
  lines.push('')
  lines.push(`  Model:     ${modelDisplay}`)
  lines.push(`  Directory: ${cwd}`)
  lines.push(`  Session:   ${sessionId}`)
  lines.push('')

  lines.push('Context window')
  if (contextPercentages.used !== null && currentUsage) {
    const totalInputNow =
      currentUsage.input_tokens +
      currentUsage.cache_creation_input_tokens +
      currentUsage.cache_read_input_tokens
    const ratio = contextPercentages.used / 100
    const bar = renderProgressBar(ratio, PROGRESS_BAR_WIDTH)
    lines.push(
      `  ${bar}  ${contextPercentages.used}% (${formatTokens(totalInputNow)} / ${formatTokens(contextWindowSize)})`,
    )
    lines.push(`  Color: ${severityColor(ratio)}`)
  } else {
    lines.push('  No context usage data available.')
  }

  lines.push('')
  lines.push('Session totals')
  lines.push(`  Cost:      $${totalCost.toFixed(4)}`)
  lines.push(`  Duration:  ${formatDuration(totalDuration, { mostSignificantOnly: true })}`)
  lines.push(`  Tokens:    ${formatTokens(totalInput + totalOutput)}`)
  lines.push(`             ${formatTokens(totalInput)} in / ${formatTokens(totalOutput)} out`)
  if (cacheRead > 0 || cacheCreation > 0) {
    lines.push(`             ${formatTokens(cacheRead)} cache read / ${formatTokens(cacheCreation)} cache write`)
  }
  const reasoning = reasoningTokensFromMessages(messages)
  if (reasoning > 0) {
    lines.push(`             ${formatTokens(reasoning)} reasoning`)
  }

  lines.push('')

  lines.push(...(await planQuotaLines(model)))
  lines.push('')

  return { type: 'text', value: lines.join('\n') }
}
