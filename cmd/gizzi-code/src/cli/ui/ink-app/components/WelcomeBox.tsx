// @ts-nocheck
/**
 * Startup header in a rounded coral box, like Kimi Code's: the Gizzi mark
 * beside the name, version and a /help hint, then labelled Directory,
 * Session and Model rows. In terminals that show inline images (iTerm2,
 * WezTerm, Ghostty, Kitty) the mark is the Brand/Gizzi/mark image three rows
 * tall and the name is the GIZZI CODE wordmark image one row tall. Elsewhere
 * (Apple Terminal, tmux) the mark is the mascot in its own colors, quadrant
 * blocks eight columns by four rows, and the name is set as type: bold caps,
 * coral G for the wordmark's core. No animation.
 */
import * as React from 'react'
import { useSyncExternalStore } from 'react'
import { Box, Text } from '../ink'
import { useMainLoopModel } from '../hooks/useMainLoopModel'
import { renderModelSetting } from '../utils/model/model'
import { getLogoDisplayData } from '../utils/logoV2Utils'
import { type ActiveBotChat, getActiveBotChat, getSessionId, onActiveBotChatChange } from '../bootstrap/state'
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

type WelcomeRow = [label: string, value: string, wrap: 'truncate-start' | 'truncate-end']

/**
 * The labelled rows. In a /bots chat the turns run on the bot's pinned model
 * in its platform session, so the box names the bot and that model instead of
 * this terminal's model (same as the footer).
 */
export function welcomeRows(cwd: string, sessionId: string, modelDisplayName: string, botChat: ActiveBotChat | null): WelcomeRow[] {
  const rows: WelcomeRow[] = [
    ['Directory', cwd, 'truncate-start'],
    ['Session', sessionId, 'truncate-end'],
  ]
  if (botChat) {
    rows.push(['Bot', botChat.botName, 'truncate-end'], ['Model', botChat.model || 'platform default model', 'truncate-end'])
  } else {
    rows.push(['Model', modelDisplayName, 'truncate-end'])
  }
  return rows
}

export function WelcomeBox(): React.ReactNode {
  const botChat = useSyncExternalStore(onActiveBotChatChange, getActiveBotChat)
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

  const title = (
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
      <Text dimColor={true}>Send /help for help information.</Text>
    </Box>
  )

  const rows = welcomeRows(cwd, getSessionId(), modelDisplayName, botChat)

  return (
    <Box flexDirection="column" width="100%" borderStyle="round" borderColor={CORAL} paddingX={2} marginBottom={1}>
      <Box flexDirection="row">
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
        {title}
      </Box>
      <Box flexDirection="column" marginTop={1}>
        {rows.map(([label, value, wrap]) => (
          <Box key={label} flexDirection="row">
            <Box width={11} flexShrink={0}>
              <Text bold={true}>{label}:</Text>
            </Box>
            <Text wrap={wrap}>{value}</Text>
          </Box>
        ))}
      </Box>
    </Box>
  )
}

export default WelcomeBox
