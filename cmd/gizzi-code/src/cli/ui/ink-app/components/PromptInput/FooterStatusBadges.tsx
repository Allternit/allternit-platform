// @ts-nocheck
import * as React from 'react'
import { Box, Text } from '../../ink'
import { getCwdState } from '../../bootstrap/state.js'
import { formatTokens } from '../../utils/format.js'
import { truncateStartToWidth } from '../../utils/truncate.js'
import { useSessionMetrics } from '../../hooks/useSessionMetrics'

const MAX_CWD_WIDTH = 24

export function FooterStatusBadges({ messages }: { messages?: import('../../types/message.js').Message[] }): React.ReactNode {
  const metrics = useSessionMetrics(messages)
  const cwd = getCwdState()
  const cwdDisplay = truncateStartToWidth(cwd, MAX_CWD_WIDTH)

  // Always real: 0% before the first response, then live usage.
  const contextText = `context: ${metrics.contextPercent ?? 0}% (${formatTokens(metrics.contextUsed ?? 0)}/${formatTokens(metrics.contextTotal)})`

  // Two rows, right-aligned: cwd + model on top, context bottom-right.
  // Single-row crowds the context readout off screen on narrow terminals.
  return (
    <Box flexDirection="column" alignItems="flex-end" flexShrink={0}>
      <Box gap={1}>
        <Text dimColor wrap="truncate">{cwdDisplay}</Text>
        <Text color="magenta" wrap="truncate">{metrics.model}</Text>
      </Box>
      <Text dimColor wrap="truncate">{contextText}</Text>
    </Box>
  )
}
