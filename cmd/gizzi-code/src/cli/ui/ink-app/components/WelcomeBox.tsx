// @ts-nocheck
/**
 * Startup header: the small Gizzi mark beside three lines of inline text
 * ("Gizzi Code vX", the model, the working directory), the way Claude Code
 * opens. Deliberately unobtrusive: no box, no large wordmark. The mark
 * blinks now and then; under prefersReducedMotion it is static.
 */
import * as React from 'react'
import { Box, Text, useAnimationFrame } from '../ink'
import { useMainLoopModel } from '../hooks/useMainLoopModel'
import { useSettings } from '../hooks/useSettings'
import { renderModelSetting } from '../utils/model/model'
import { getLogoDisplayData } from '../utils/logoV2Utils'
import { headerMarkRows } from './welcomeArt'

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
  const lines = [
    <Text key="name">
      <Text bold={true}>Gizzi Code</Text>
      <Text dimColor={true}> v{version}</Text>
    </Text>,
    <Text key="model" dimColor={true} wrap="truncate-end">{modelDisplayName}</Text>,
    <Text key="cwd" dimColor={true} wrap="truncate-start">{cwd}</Text>,
  ]

  return (
    <Box ref={animRef} flexDirection="column" paddingLeft={1} marginBottom={1}>
      {mark.map((segments, i) => (
        <Box key={i} flexDirection="row">
          <Text>
            {segments.map(([text, color, bg], j) => (
              <Text key={j} color={color || undefined} backgroundColor={bg}>
                {text}
              </Text>
            ))}
          </Text>
          <Text>{'   '}</Text>
          <Box flexShrink={1}>{lines[i]}</Box>
        </Box>
      ))}
    </Box>
  )
}

export default WelcomeBox
