// @ts-nocheck
/**
 * Startup header, sized like Claude Code's: three lines of text. The first is
 * the GIZZI CODE lockup (Brand/Gizzi/lockup: mark + wordmark) as an image
 * scaled to one text row, then the version. Terminals that can't show inline
 * images (Apple Terminal, tmux) get the name set as type instead: bold caps
 * with the G in coral for the wordmark's core. Then the model and the working
 * directory. No box, no animation.
 */
import * as React from 'react'
import { Box, Text } from '../ink'
import { useMainLoopModel } from '../hooks/useMainLoopModel'
import { renderModelSetting } from '../utils/model/model'
import { getLogoDisplayData } from '../utils/logoV2Utils'
import { useTheme } from './design-system/ThemeProvider'
import { inlineImagePlaceholder } from '../ink/inlineImage'
import { CORAL } from './welcomeArt'
import {
  GIZZI_LOCKUP_ASPECT,
  GIZZI_LOCKUP_PNG_DARK_INK,
  GIZZI_LOCKUP_PNG_LIGHT_INK,
} from './gizziLockupImage'

// A terminal cell is about twice as tall as wide, so a one-row image spans
// aspect * 2 columns.
const LOCKUP_COLS = Math.round(GIZZI_LOCKUP_ASPECT * 2)

export function WelcomeBox(): React.ReactNode {
  const model = useMainLoopModel()
  const modelDisplayName = renderModelSetting(model)
  const { version, cwd } = getLogoDisplayData()
  const [themeName] = useTheme()
  const lightTheme = String(themeName).startsWith('light')
  const lockup = inlineImagePlaceholder(
    lightTheme ? 'gizzi-lockup-dark-ink' : 'gizzi-lockup-light-ink',
    lightTheme ? GIZZI_LOCKUP_PNG_DARK_INK : GIZZI_LOCKUP_PNG_LIGHT_INK,
    LOCKUP_COLS,
  )

  return (
    <Box flexDirection="column" paddingLeft={1} marginBottom={1}>
      <Text>
        {lockup !== null ? (
          <Text>{lockup}</Text>
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
}

export default WelcomeBox
