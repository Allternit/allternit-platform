import * as React from 'react'
import { CacheCountdown } from './CacheCountdown'
import { useSyncExternalStore } from 'react'
import { getActiveBotChat, onActiveBotChatChange } from '../../bootstrap/state'
import { Box, Text } from '../../ink'
import { useMainLoopModel } from '../../hooks/useMainLoopModel'
import { useAppState } from '../../state/AppState'
import type { Message } from '../../types/message.js'
import { calculateContextPercentages, getContextWindowForModel } from '../../utils/context.js'
import { getDisplayedEffortLevel, modelSupportsEffort } from '../../utils/effort.js'
import { formatTokens } from '../../utils/format.js'
import { renderModelSetting } from '../../utils/model/model'
import { getCurrentUsage } from '../../utils/tokens.js'

/**
 * Second footer line, laid out like Kimi Code's: the model and effort on the
 * left, context-window use on the right ("context: 12% (24k/200k)"). It is
 * real from the start: 0% before the first reply, then the last reply's
 * input tokens. In a /bots chat the turns run in the bot's platform session,
 * so the line names the bot and its model instead; this terminal's effort and
 * context don't apply there.
 */
export function FooterContextLine({ messages }: { messages: Message[] }): React.ReactNode {
  const botChat = useSyncExternalStore(onActiveBotChatChange, getActiveBotChat)
  const model = useMainLoopModel()
  const effortValue = useAppState(s => s.effortValue)
  const total = getContextWindowForModel(model)
  const usage = getCurrentUsage(messages)
  const used = usage
    ? usage.input_tokens + usage.cache_creation_input_tokens + usage.cache_read_input_tokens
    : 0
  const percent = calculateContextPercentages(usage, total).used ?? 0
  const effort = modelSupportsEffort(model) ? getDisplayedEffortLevel(model, effortValue) : null

  if (botChat) {
    return (
      <Box flexGrow={1}>
        <Text dimColor={true} wrap="truncate-end">
          bot: {botChat.botName} · {botChat.model || 'platform default model'}
        </Text>
      </Box>
    )
  }

  return (
    <Box flexGrow={1} flexDirection="column">
    <Box justifyContent="space-between" gap={1}>
      <Text dimColor={true} wrap="truncate-end">
        {renderModelSetting(model)}
        {effort ? `  effort: ${effort}` : ''}
      </Text>
      <Box flexShrink={0}>
        <Text dimColor={true}>
          context: {percent}% ({formatTokens(used)}/{formatTokens(total)})
        </Text>
      </Box>
    </Box>
    <CacheCountdown messages={messages} model={model} />
    </Box>
  )
}
