import * as React from 'react'
import { Box, Text } from '../ink'
import { useTerminalSize } from '../hooks/useTerminalSize'
import { gizziPetRows, GIZZI_PET_WIDTH } from '../pet/gizziSprite'

/** Below this width the pixel Gizzi is dropped and only the copy shows. */
const MIN_COLUMNS_FOR_ART = 60

// Static frame: computed once, no animation, no timers.
const IDLE_ROWS = gizziPetRows('idle')

/**
 * Empty-state line for Gizzi Code: the idle pixel Gizzi beside the copy.
 * Text-only on narrow terminals. Decorative art, copy unchanged.
 */
export function EmptyLine({ children }: { children: React.ReactNode }): React.ReactNode {
  const { columns } = useTerminalSize()
  if (columns < MIN_COLUMNS_FOR_ART) {
    return <Text dimColor={true}>{children}</Text>
  }
  return (
    <Box flexDirection="row" alignItems="center">
      <Box flexDirection="column" width={GIZZI_PET_WIDTH} flexShrink={0} marginRight={2}>
        {IDLE_ROWS.map((row, i) => (
          <Text key={i}>
            {row.map(([t, color, bg, inverse], j) => (
              <Text key={j} color={color || undefined} backgroundColor={bg} inverse={inverse}>{t}</Text>
            ))}
          </Text>
        ))}
      </Box>
      <Box flexShrink={1}>
        <Text dimColor={true}>{children}</Text>
      </Box>
    </Box>
  )
}
