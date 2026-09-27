// @ts-nocheck
/**
 * Animated startup welcome box. One rounded box: the Gizzi mascot
 * (beacon pulse + periodic blink) beside the coral GIZZI block
 * wordmark (shimmer sweep), then a welcome line, tips, and info
 * fields (Directory / Session / Model / Version). Everything collapses
 * to a static frame under prefersReducedMotion.
 */
import * as React from 'react'
import { Box, Text, useAnimationFrame } from '../ink'
import { useMainLoopModel } from '../hooks/useMainLoopModel'
import { useSettings } from '../hooks/useSettings'
import { renderModelSetting } from '../utils/model/model'
import { getLogoDisplayData } from '../utils/logoV2Utils'
import { getSessionId } from '../bootstrap/state.js'
import { useTheme } from './design-system/ThemeProvider'
import { getTheme } from '../utils/theme'
import { interpolateColor, parseRGB, toRGBColor } from './Spinner/utils'
import { CORAL, GIZZI_HEIGHT, WORDMARK_ROWS, WORDMARK_WIDTH, gizziRows } from './welcomeArt'

// Wordmark rows sit vertically centered beside the mascot.
const WORDMARK_TOP = Math.floor((GIZZI_HEIGHT - WORDMARK_ROWS.length) / 2)

const TICK_MS = 120
const BLINK_PERIOD_MS = 3800
const BLINK_LENGTH_MS = 160
const SWEEP_PERIOD_MS = 3000
const SWEEP_LENGTH_MS = 900
const SWEEP_WINDOW = 3

// Fallbacks for themes whose colors aren't rgb() strings (the ansi themes).
const CORAL_RGB = { r: 0xd9, g: 0x77, b: 0x57 }
const CORAL_BRIGHT_RGB = { r: 0xf5, g: 0x95, b: 0x75 }

function Field({ label, value }: { label: string; value: string }) {
  return (
    <Text>
      <Text dimColor={true}>{label}: </Text>
      <Text>{value}</Text>
    </Text>
  )
}

function WordmarkRow({
  text,
  sweepCenter,
  base,
  shimmer,
}: {
  text: string
  sweepCenter: number | null
  base: { r: number; g: number; b: number }
  shimmer: { r: number; g: number; b: number }
}) {
  if (sweepCenter === null) {
    return <Text color={CORAL}>{text}</Text>
  }
  return (
    <Text>
      {text.split('').map((ch, i) => {
        const intensity = Math.max(0, 1 - Math.abs(i - sweepCenter) / SWEEP_WINDOW)
        const color =
          ch === ' ' || intensity <= 0
            ? CORAL
            : toRGBColor(interpolateColor(base, shimmer, intensity))
        return (
          <Text key={i} color={color}>
            {ch}
          </Text>
        )
      })}
    </Text>
  )
}

export function WelcomeBox(): React.ReactNode {
  const model = useMainLoopModel()
  const modelDisplayName = renderModelSetting(model)
  const { version, cwd } = getLogoDisplayData()
  const sessionId = getSessionId()
  const settings = useSettings()
  const reducedMotion = settings.prefersReducedMotion ?? false
  const [themeName] = useTheme()
  const theme = getTheme(themeName)
  const coralRGB = parseRGB(theme.gizzi) ?? CORAL_RGB
  const coralBrightRGB = parseRGB(theme.gizziShimmer) ?? CORAL_BRIGHT_RGB
  const [animRef, time] = useAnimationFrame(reducedMotion ? null : TICK_MS)
  const t = reducedMotion ? 0 : time
  const beaconColor = reducedMotion
    ? CORAL
    : toRGBColor(
        interpolateColor(coralRGB, coralBrightRGB, (Math.sin(t / 600) + 1) / 2),
      )
  const blinking = !reducedMotion && t % BLINK_PERIOD_MS > BLINK_PERIOD_MS - BLINK_LENGTH_MS
  const sweepPhase = t % SWEEP_PERIOD_MS
  const sweepCenter =
    !reducedMotion && sweepPhase < SWEEP_LENGTH_MS
      ? -SWEEP_WINDOW + ((WORDMARK_WIDTH + SWEEP_WINDOW * 2) * sweepPhase) / SWEEP_LENGTH_MS
      : null

  const mascot = gizziRows({ beaconColor, blinking })

  return (
    <Box ref={animRef} flexDirection="column" borderStyle="round" borderColor={CORAL} paddingX={1} width="100%">
      <Box flexDirection="column" marginBottom={1}>
        {mascot.map((segments, i) => (
          <Box key={i} flexDirection="row">
            <Text>
              {segments.map(([text, color, bg], j) => (
                <Text key={j} color={color || undefined} backgroundColor={bg} bold={bg !== undefined && color === CORAL}>
                  {text}
                </Text>
              ))}
            </Text>
            {i >= WORDMARK_TOP && i < WORDMARK_TOP + WORDMARK_ROWS.length && (
              <Text>{'   '}</Text>
            )}
            {i >= WORDMARK_TOP && i < WORDMARK_TOP + WORDMARK_ROWS.length && (
              <WordmarkRow
                text={WORDMARK_ROWS[i - WORDMARK_TOP]}
                sweepCenter={sweepCenter}
                base={coralRGB}
                shimmer={coralBrightRGB}
              />
            )}
          </Box>
        ))}
      </Box>
      <Text bold={true}>Welcome to Gizzi Code!</Text>
      <Text dimColor={true}>/help for commands · /model to switch brains · shift+tab for permission modes</Text>
      <Text> </Text>
      <Field label="Directory" value={cwd} />
      <Field label="Session" value={sessionId ?? ''} />
      <Field label="Model" value={modelDisplayName} />
      <Field label="Version" value={version} />
    </Box>
  )
}

export default WelcomeBox
