import { formatTotalCost } from '../../cost-tracker'
import { currentLimits } from '../../services/claudeAiLimits'
import type { LocalCommandCall } from '../../types/command'
import { isClaudeAISubscriber } from '../../utils/auth'

/** The cost text: printed by /cost outside the terminal UI, shown in the /status Cost tab. */
export function costSummaryText(): string {
  if (isClaudeAISubscriber()) {
    let value: string

    if (currentLimits.isUsingOverage) {
      value =
        'You are currently using your overages to power your Gizzi Code usage. We will automatically switch you back to your subscription rate limits when they reset'
    } else {
      value =
        'You are currently using your subscription to power your Gizzi Code usage'
    }

    if (process.env.USER_TYPE === 'ant') {
      value += `\n\n[ANT-ONLY] Showing cost anyway:\n ${formatTotalCost()}`
    }
    return value
  }
  return formatTotalCost()
}

export const call: LocalCommandCall = async () => ({ type: 'text', value: costSummaryText() })
