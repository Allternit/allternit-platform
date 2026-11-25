import * as React from 'react'
import { Box, Text } from '../ink'
import { useAnimationFrame } from '../ink/hooks/use-animation-frame'
import { useSettings } from '../hooks/useSettings'
import { DOT_FPS, dotBandFrame, dotMode, dotProgressText, type ProgressDetailLite } from '@/shared/util/dot-progress'

/**
 * One braille line under a running generation tool: a light band sweeping the
 * dots at about 10 fps, the real stage label, seconds, and a percent only when the
 * report has a real fraction. Still (no clock, no seconds) under NO_COLOR, a non-TTY,
 * or the prefersReducedMotion setting. The caller drops it when the tool finishes.
 */
export function DotProgressLine({ detail, width = 20 }: { detail: ProgressDetailLite; width?: number }): React.ReactNode {
  const reducedMotion = useSettings().prefersReducedMotion ?? false
  const animate = dotMode(reducedMotion) === 'animate'
  const [ref, time] = useAnimationFrame(animate ? Math.round(1000 / DOT_FPS) : null)
  const [start] = React.useState(() => time)
  const elapsed = animate ? Math.max(0, (time - start) / 1000) : undefined
  return (
    <Box ref={ref} flexDirection="row" gap={1}>
      <Text color="gizzi">{dotBandFrame(width, elapsed, detail.fraction)}</Text>
      <Text dimColor wrap="truncate">{dotProgressText(detail, elapsed)}</Text>
    </Box>
  )
}
