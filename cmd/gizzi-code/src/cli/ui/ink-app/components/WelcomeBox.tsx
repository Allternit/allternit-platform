// @ts-nocheck
/**
 * Startup header, sized like Claude Code's. In terminals that show inline
 * images (iTerm2, WezTerm, Ghostty, Kitty): the Gizzi mark (Brand/Gizzi/mark)
 * three rows tall on the left; beside it, the GIZZI CODE wordmark
 * (Brand/Gizzi/wordmark) one text row tall with the version, then the model
 * and the working directory. Elsewhere (Apple Terminal, tmux) the mark is
 * the mascot in its own colors, drawn in quadrant blocks eight columns by
 * four rows, and the name is set as type: bold caps, coral G for the
 * wordmark's core. No box, no animation.
 */
import * as React from 'react'
import { Box, Text } from '../ink'
import { useMainLoopModel } from '../hooks/useMainLoopModel'
import { renderModelSetting } from '../utils/model/model'
import { getLogoDisplayData } from '../utils/logoV2Utils'
import { useTheme } from './design-system/ThemeProvider'
import { inlineImageBlock, inlineImagePlaceholder } from '../ink/inlineImage'
import { CORAL, textMarkRows } from './welcomeArt'
import {
  GIZZI_MARK_ASPECT,
  GIZZI_MARK_PNG_DARK_INK,
  GIZZI_MARK_PNG_LIGHT_INK,
  GIZZI_WORDMARK_ASPECT,
  GIZZI_WORDMARK_PNG_DARK_INK,
  GIZZI_WORDMARK_PNG_LIGHT_INK,
} from './gizziLockupImage'

// A terminal cell is about twice as tall as wide: an image `rows` tall spans
// aspect * rows * 2 columns.
const MARK_ROWS = 3
const MARK_COLS = Math.round(GIZZI_MARK_ASPECT * MARK_ROWS * 2)
const WORDMARK_COLS = Math.round(GIZZI_WORDMARK_ASPECT * 2)

export function WelcomeBox(): React.ReactNode {
  const model = useMainLoopModel()
  const modelDisplayName = renderModelSetting(model)
  const { version, cwd } = getLogoDisplayData()
  const [themeName] = useTheme()
  const light = String(themeName).startsWith('light')
  const ink = light ? 'dark-ink' : 'light-ink'
  const mark = inlineImageBlock(
    `gizzi-mark-${ink}`,
    light ? GIZZI_MARK_PNG_DARK_INK : GIZZI_MARK_PNG_LIGHT_INK,
    MARK_COLS,
    MARK_ROWS,
  )
  const wordmark = inlineImagePlaceholder(
    `gizzi-wordmark-${ink}`,
    light ? GIZZI_WORDMARK_PNG_DARK_INK : GIZZI_WORDMARK_PNG_LIGHT_INK,
    WORDMARK_COLS,
  )

  const text = (
    <Box flexDirection="column" flexShrink={1}>
      <Text>
        {wordmark !== null ? (
          <Text>{wordmark}</Text>
        ) : (
          <>
            <Text bold={true} color={CORAL}>G</Text>
            <Text bold={true}>IZZI CODE</Text>
          </>
        )}
        <Text dimColor={true}> v{version}</Text>
      </Text>
      <Text dimColor={true} wrap="truncate-end">{modelDisplayName}</Text>
      <Text dimColor={true} wrap="truncate-start">{cwd}</Text>
    </Box>
  )

  return (
    <Box flexDirection="row" paddingLeft={1} marginBottom={1}>
      <Box flexDirection="column" flexShrink={0} marginRight={2}>
        {mark !== null
          ? mark.map((row, i) => <Text key={i}>{row}</Text>)
          : textMarkRows().map((segments, i) => (
              <Text key={i}>
                {segments.map(([t, color, bg], j) => (
                  <Text key={j} color={color || undefined} backgroundColor={bg}>{t}</Text>
                ))}
              </Text>
            ))}
      </Box>
      {text}
    </Box>
  )
}

export default WelcomeBox
