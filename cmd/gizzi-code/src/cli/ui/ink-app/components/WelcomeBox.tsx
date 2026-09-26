// @ts-nocheck
/**
 * Animated startup welcome box. One rounded box: the Architectural
 * Sentinel (beacon pulse + periodic blink) beside the GIZZI block
 * wordmark (coral shimmer sweep), then a welcome line, tips, and info
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
import { interpolateColor, toRGBColor } from './Spinner/utils'
import {
  CORAL,
  CORAL_BRIGHT,
  SAND,
  WORDMARK_ROWS,
  WORDMARK_WIDTH,
  sentinelRows,
} from './welcomeArt'

const TICK_MS = 120
const BLINK_PERIOD_MS = 3800
const BLINK_LENGTH_MS = 160
const SWEEP_PERIOD_MS = 3000
const SWEEP_LENGTH_MS = 900
const SWEEP_WINDOW = 3

// parseRGB() only understands rgb() strings, so keep these as literals.
const SAND_RGB = { r: 0xd4, g: 0xb0, b: 0x8c }
const CORAL_RGB = { r: 0xd9, g: 0x77, b: 0x57 }
const CORAL_BRIGHT_RGB = { r: 0xf0, g: 0x98, b: 0x78 }
const SHIMMER_RGB = { r: 0xf2, g: 0xd9, b: 0xb8 }

function Field({ label, value }: { label: string; value: string }) {
  return (
    <Text>
      <Text dimColor={true}>{label}: </Text>
      <Text>{value}</Text>
    </Text>
  )
}

function WordmarkRow({ text, sweepCenter }: { text: string; sweepCenter: number | null }) {
  if (sweepCenter === null) {
    return <Text color={SAND}>{text}</Text>
  }
  return (
    <Text>
      {text.split('').map((ch, i) => {
        const intensity = Math.max(0, 1 - Math.abs(i - sweepCenter) / SWEEP_WINDOW)
        const color =
          ch === ' ' || intensity <= 0
            ? SAND
            : toRGBColor(interpolateColor(SAND_RGB, SHIMMER_RGB, intensity))
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
  const [animRef, time] = useAnimationFrame(reducedMotion ? null : TICK_MS)
  const t = reducedMotion ? 0 : time
  const beaconColor = reducedMotion
    ? CORAL
    : toRGBColor(
        interpolateColor(CORAL_RGB, CORAL_BRIGHT_RGB, (Math.sin(t / 600) + 1) / 2),
      )
  const blinking = !reducedMotion && t % BLINK_PERIOD_MS > BLINK_PERIOD_MS - BLINK_LENGTH_MS
  const sweepPhase = t % SWEEP_PERIOD_MS
  const sweepCenter =
    !reducedMotion && sweepPhase < SWEEP_LENGTH_MS
      ? -SWEEP_WINDOW + ((WORDMARK_WIDTH + SWEEP_WINDOW * 2) * sweepPhase) / SWEEP_LENGTH_MS
      : null

  const mascot = sentinelRows({ beaconColor, blinking })

  return (
    <Box ref={animRef} flexDirection="column" borderStyle="round" borderColor={SAND} paddingX={1} width="100%">
      <Box flexDirection="column" marginBottom={1}>
        {mascot.map((segments, i) => (
          <Box key={i} flexDirection="row">
            <Text>
              {segments.map(([text, color], j) => (
                <Text key={j} color={color}>
                  {text}
                </Text>
              ))}
            </Text>
            {i >= 1 && i <= WORDMARK_ROWS.length && (
              <Text>{'   '}</Text>
            )}
            {i >= 1 && i <= WORDMARK_ROWS.length && (
              <WordmarkRow text={WORDMARK_ROWS[i - 1]} sweepCenter={sweepCenter} />
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
