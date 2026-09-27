// @ts-nocheck
/**
 * Startup header: the Gizzi Code lockup (small Gizzi mark + GIZZI CODE block
 * wordmark, both three rows tall) with the version inline after it, and one
 * dim line for the model and working directory. Sized like Claude Code's
 * header, no box. The mark blinks now and then; static under
 * prefersReducedMotion.
 */
import * as React from 'react'
import { Box, Text, useAnimationFrame } from '../ink'
import { useMainLoopModel } from '../hooks/useMainLoopModel'
import { useSettings } from '../hooks/useSettings'
import { renderModelSetting } from '../utils/model/model'
import { getLogoDisplayData } from '../utils/logoV2Utils'
import { headerMarkRows, wordmarkRows } from './welcomeArt'

const TICK_MS = 200
const BLINK_PERIOD_MS = 5200
const BLINK_LENGTH_MS = 180

export function WelcomeBox(): React.ReactNode {
  const model = useMainLoopModel()
  const modelDisplayName = renderModelSetting(model)
  const { version, cwd } = getLogoDisplayData()
  const settings = useSettings()
  const reducedMotion = settings.prefersReducedMotion ?? false
  const [animRef, time] = useAnimationFrame(reducedMotion ? null : TICK_MS)
  const blinking = !reducedMotion && time % BLINK_PERIOD_MS > BLINK_PERIOD_MS - BLINK_LENGTH_MS
  const mark = headerMarkRows({ blinking })
  const wordmark = wordmarkRows()
  const art = (segments, key) =>
    segments.map(([text, color, bg], j) => (
      <Text key={`${key}-${j}`} color={color || undefined} backgroundColor={bg}>
        {text}
      </Text>
    ))

  return (
    <Box ref={animRef} flexDirection="column" paddingLeft={1} marginBottom={1}>
      {mark.map((segments, i) => (
        <Box key={i} flexDirection="row">
          <Text>
            {art(segments, `m${i}`)}
            {'  '}
            {art(wordmark[i], `w${i}`)}
            {i === mark.length - 1 && <Text dimColor={true}>{`  v${version}`}</Text>}
          </Text>
        </Box>
      ))}
      <Box flexDirection="row">
        <Box flexShrink={0}>
          <Text dimColor={true}>{`${modelDisplayName} · `}</Text>
        </Box>
        <Box flexShrink={1}>
          <Text dimColor={true} wrap="truncate-start">{cwd}</Text>
        </Box>
      </Box>
    </Box>
  )
}

export default WelcomeBox
