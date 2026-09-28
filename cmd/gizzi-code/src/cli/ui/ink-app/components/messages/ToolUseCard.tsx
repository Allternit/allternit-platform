// @ts-nocheck
import * as React from 'react'
import { Box, Text } from '../../ink'
import { useAnimationFrame } from '../../ink/hooks/use-animation-frame'
import { useAppStateMaybeOutsideOfProvider } from '../../state/AppState'
import { OrbMark } from '../Spinner/OrbMark'
import { orbFrameAt, orbStateForTool } from '../Spinner/allternitOrbFrames'
import type { ToolUseBlockParam } from '@allternit/gizzi-sdk/providers/allternit/resources/index.mjs'

type Props = {
  param: ToolUseBlockParam
  toolName: string
  isQueued: boolean
  isResolved: boolean
  isError: boolean
  /** The tool's arguments, drawn right after its name: "Update(src/math.ts)". */
  args?: React.ReactNode
  children: React.ReactNode
}

export function ToolUseCard({
  param,
  toolName,
  isQueued,
  isResolved,
  isError,
  args,
  children,
}: Props): React.ReactNode {
  const tasks = useAppStateMaybeOutsideOfProvider(s => s.tasks) ?? {}
  const backgroundTask = Object.values(tasks).find(
    t =>
      t &&
      typeof t === 'object' &&
      'parentToolUseID' in t &&
      (t as { parentToolUseID?: string }).parentToolUseID === param.id,
  ) as { id?: string; status?: string } | undefined

  // Status is the AllternitOrb: live while the tool runs (the tool's own
  // motion — searching, writing, working), settled when it's done. Only a
  // queued or failed tool adds a word.
  const state = isError ? 'error' : isResolved ? 'done' : isQueued ? 'queued' : 'running'

  return (
    <Box
      flexDirection="column"
      borderStyle="single"
      borderColor={isError ? 'red' : isResolved ? 'inactive' : 'gizzi'}
      paddingX={1}
      paddingY={0}
      width="100%"
    >
      <Box flexDirection="row" justifyContent="space-between" gap={1}>
        <Box flexDirection="row" flexShrink={1}>
          <Text bold={true} wrap="truncate">
            {toolName}
          </Text>
          {args}
        </Box>
        <ToolOrb state={state} toolName={toolName} />
      </Box>
      {backgroundTask && (
        <Text dimColor wrap="truncate">
          ↳ background task: {backgroundTask.id?.slice(0, 8) ?? 'unknown'} (
          {backgroundTask.status ?? 'running'})
        </Text>
      )}
      {children}
    </Box>
  )
}

function ToolOrb({ state, toolName }: { state: 'running' | 'done' | 'queued' | 'error'; toolName: string }): React.ReactNode {
  const [ref, time] = useAnimationFrame(state === 'running' ? 90 : null)
  if (state === 'running') {
    const frame = orbFrameAt(orbStateForTool(toolName), time)
    return (
      <Box ref={ref} flexShrink={0} width={3}>
        <Text color="text">{frame.left}</Text>
        <Text color="gizzi">{frame.core}</Text>
        <Text color="text">{frame.right}</Text>
      </Box>
    )
  }
  if (state === 'done') return <Box ref={ref} flexShrink={0}><OrbMark /></Box>
  return (
    <Box ref={ref} flexDirection="row" flexShrink={0} gap={1}>
      <Text dimColor={state === 'queued'} color={state === 'error' ? 'error' : undefined}>
        {state === 'error' ? 'error' : 'queued'}
      </Text>
      <OrbMark coreColor={state === 'error' ? 'error' : 'inactive'} />
    </Box>
  )
}
