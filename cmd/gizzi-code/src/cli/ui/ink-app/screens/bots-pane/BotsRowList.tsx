// @ts-nocheck
/**
 * BotsRowList — presentational roster list for the /bots pane (Phase B5).
 *
 * Pure with respect to app/runtime state: rows come in as props, terminal
 * width is a prop (not useTerminalSize) so the component renders in tests.
 * The screen (BotsPaneScreen) owns data loading, selection, and keys.
 */
import * as React from 'react'
import { Box, Text } from '../../ink'
import type { BotRosterRow } from '@/runtime/bots/bot-roster.js'
import { EMPTY_BOTS_MESSAGE, buildBotRowSegments } from './rows.js'

export function BotsRowList({
  rows,
  selectedIndex,
}: {
  rows: BotRosterRow[]
  selectedIndex: number
}): React.ReactNode {
  if (rows.length === 0) {
    return (
      <Box flexDirection="column" marginTop={1}>
        <Text dimColor>{EMPTY_BOTS_MESSAGE}</Text>
      </Box>
    )
  }
  return (
    <Box flexDirection="column" marginTop={1}>
      {rows.map((row, i) => {
        const isSelected = i === selectedIndex
        const s = buildBotRowSegments(row)
        return (
          <Box key={row.name} flexDirection="row" paddingLeft={1}>
            <Text color={s.glyphColor}>{s.glyph} </Text>
            <Text
              bold={isSelected}
              color={isSelected ? 'text' : 'subtle'}
              wrap="truncate-end"
            >
              {s.identity}
            </Text>
            {s.description ? (
              <Text dimColor wrap="truncate-end">
                {'  '}
                {s.description}
              </Text>
            ) : null}
            {s.model ? <Text dimColor>{s.model}</Text> : null}
            {s.badge ? (
              <Text color="warning" bold>
                {' '}
                {s.badge}
              </Text>
            ) : null}
          </Box>
        )
      })}
    </Box>
  )
}

export default BotsRowList
