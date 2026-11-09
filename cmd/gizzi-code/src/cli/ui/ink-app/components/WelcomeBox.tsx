// @ts-nocheck
/**
 * Startup header, in a rounded box drawn in the mascot's face-panel brown:
 * the Gizzi mascot, then (top-aligned beside it) the GIZZI CODE braille
 * wordmark with the version, a /help hint, and labelled Directory, Session
 * and Model rows.
 *
 * In terminals that show inline images (iTerm2 / WezTerm OSC 1337, Kitty /
 * Ghostty graphics) the mascot is the approved mascot image, 8 rows tall.
 * Elsewhere (Apple Terminal, tmux, basic TERMs) it is the same mascot in half
 * blocks, 18 columns by 8 rows, and it idles: the beacon pulses and the eyes
 * blink about every 4 s. The idle animation stops for good on the first key
 * press or once a turn starts, and never runs under reduced motion, CI,
 * NO_COLOR or a non-TTY. Under 60 columns or 20 rows the mascot shrinks to
 * 3 rows and the wordmark becomes bold "Gizzi Code"; under 36 columns the
 * mascot is dropped.
 *
 * `headerArt` (setting) / GIZZI_HEADER_ART (env) picks auto | image | blocks
 * | none.
 */
import * as React from 'react'
import { useState, useSyncExternalStore } from 'react'
import { Box, Text, useAnimationFrame, useInput } from '../ink'
import { useMainLoopModel } from '../hooks/useMainLoopModel'
import { useSettings } from '../hooks/useSettings'
import { useTheme } from './design-system/ThemeProvider'
import { getTheme } from '../utils/theme'
import { useTerminalSize } from '../hooks/useTerminalSize'
import { renderModelSetting } from '../utils/model/model'
import { getLogoDisplayData } from '../utils/logoV2Utils'
import { type ActiveBotChat, getActiveBotChat, getSessionId, onActiveBotChatChange } from '../bootstrap/state'
import { inlineImageBlock } from '../ink/inlineImage'
import { CORAL, VISOR, type ArtRow, type MascotSize, mascotRows, wordmarkRows, WORDMARK_VERSION_ROW } from './welcomeArt'
import { GIZZI_MASCOT_ASPECT, GIZZI_MASCOT_PNG } from './gizziMascotImage'

export type HeaderArt = 'auto' | 'image' | 'blocks' | 'none'

const MID_ROWS = 8
const MID_COLS = 18
// A terminal cell is about twice as tall as wide.
const IMAGE_COLS = Math.min(MID_COLS, Math.round(GIZZI_MASCOT_ASPECT * MID_ROWS * 2))

const TICK_MS = 200
const BLINK_PERIOD_MS = 4000
const BLINK_MS = 200
const PULSE_PERIOD_MS = 2400
const GLOW = 'rgb(255,175,135)'

/** The header art mode: env beats the setting; unknown values mean auto. */
export function headerArtMode(env: NodeJS.ProcessEnv, setting: string | undefined): HeaderArt {
  const v = (env.GIZZI_HEADER_ART || setting || 'auto').toLowerCase()
  return v === 'image' || v === 'blocks' || v === 'none' ? v : 'auto'
}

/** Whether the idle animation may run at all. */
export function headerCanAnimate(env: NodeJS.ProcessEnv, isTTY: boolean, reducedMotion: boolean): boolean {
  const on = (v: string | undefined) => v !== undefined && v !== '' && v !== '0' && v !== 'false'
  return isTTY && !reducedMotion && !on(env.GIZZI_REDUCED_MOTION) && !on(env.CI) && env.NO_COLOR === undefined
}

/** Mascot size for the terminal: mid (8 rows), tiny (3 rows) or none. */
export function headerMascotSize(columns: number, rows: number): MascotSize | null {
  if (columns < 36) return null
  if (columns < 60 || rows < 20) return 'tiny'
  return 'mid'
}

/**
 * The idle frame at time `t`: the beacon/nose coral eases toward the lighter
 * coral and back over PULSE_PERIOD_MS (stepped, so a 256-color terminal
 * changes in whole steps), and the eyes close for one tick every
 * BLINK_PERIOD_MS.
 */
export function idleFrame(t: number): { pulse: number; blinking: boolean } {
  const phase = (t % PULSE_PERIOD_MS) / PULSE_PERIOD_MS
  const pulse = Math.round(((1 - Math.cos(phase * 2 * Math.PI)) / 2) * 3) / 3
  return { pulse, blinking: t % BLINK_PERIOD_MS >= BLINK_PERIOD_MS - BLINK_MS }
}

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

/**
 * The name slot: the GIZZI CODE braille dot-matrix wordmark (2 rows) with the
 * version on row WORDMARK_VERSION_ROW, or bold "Gizzi Code" on one row when the
 * terminal is small. Font and coral G are WORDMARK_FONT / WORDMARK_CORAL_G.
 */
export function Wordmark({ version, compact }: { version: string; compact: boolean }): React.ReactNode {
  const ver = <Text dimColor={true}>v{version}</Text>
  if (compact) {
    return (
      <Text wrap="truncate-end">
        <Text bold={true}>Gizzi Code</Text> {ver}
      </Text>
    )
  }
  return wordmarkRows().map((segments, i) => (
    <Text key={i} wrap="truncate-end">
      {segments.map(([t, color], j) => <Text key={j} color={color}>{t}</Text>)}
      {i === WORDMARK_VERSION_ROW && <>{'  '}{ver}</>}
    </Text>
  ))
}

function ArtRows({ rows }: { rows: ArtRow[] }): React.ReactNode {
  return rows.map((segments, i) => (
    <Text key={i}>
      {segments.map(([t, color, bg], j) => (
        <Text key={j} color={color || undefined} backgroundColor={bg}>{t}</Text>
      ))}
    </Text>
  ))
}

/** Half-block mascot; idles while `animate` is on. */
function BlockMascot({ size, animate }: { size: MascotSize; animate: boolean }): React.ReactNode {
  const [ref, time] = useAnimationFrame(animate ? TICK_MS : null)
  const { pulse, blinking } = animate ? idleFrame(time) : { pulse: 0, blinking: false }
  const [themeName] = useTheme()
  // The glow is a light coral that stays coral in 256 colors (216); the
  // themes' gizziShimmer rounds to the body's sand there. ansi themes keep
  // their own shimmer. Middle steps snap to one swap so 256-color terminals
  // never show near-identical shades.
  const glow = String(getTheme(themeName).gizzi).startsWith('rgb(') ? GLOW : 'gizziShimmer'
  const coral = pulse >= 0.5 ? glow : CORAL
  return (
    <Box ref={ref} flexDirection="column">
      <ArtRows rows={mascotRows(size, { coral, blinking })} />
    </Box>
  )
}

export function WelcomeBox({ animate = true }: { animate?: boolean }): React.ReactNode {
  const botChat = useSyncExternalStore(onActiveBotChatChange, getActiveBotChat)
  const model = useMainLoopModel()
  const { version, cwd } = getLogoDisplayData()
  const settings = useSettings()
  const { columns, rows: termRows } = useTerminalSize()
  const mode = headerArtMode(process.env, settings.headerArt)
  const canAnimate = headerCanAnimate(process.env, !!process.stdout.isTTY, settings.prefersReducedMotion ?? false)

  // Latch: once the user types or a turn starts, the header is still for good.
  const [stopped, setStopped] = useState(false)
  const live = canAnimate && animate && !stopped
  useInput(() => setStopped(true), { isActive: live })

  const size = mode === 'none' ? null : headerMascotSize(columns, termRows)
  const image =
    size === 'mid' && (mode === 'auto' || mode === 'image')
      ? inlineImageBlock('gizzi-mascot-header', GIZZI_MASCOT_PNG, IMAGE_COLS, MID_ROWS, mode === 'image')
      : null
  const rows = welcomeRows(cwd, getSessionId(), renderModelSetting(model), botChat)

  return (
    <Box flexDirection="row" width="100%" borderStyle="round" borderColor={VISOR} paddingX={1} marginBottom={1}>
      {size !== null && (
        <Box flexDirection="column" flexShrink={0} width={size === 'mid' ? MID_COLS : undefined} alignItems="center" marginRight={3}>
          {image !== null ? image.map((row, i) => <Text key={i}>{row}</Text>) : <BlockMascot size={size} animate={live} />}
        </Box>
      )}
      <Box flexDirection="column" flexGrow={1} flexShrink={1} flexBasis={0}>
        <Wordmark version={version} compact={size !== 'mid'} />
        <Text dimColor={true} wrap="truncate-end">Send /help for help information.</Text>
        <Text> </Text>
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
