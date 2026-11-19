// @ts-nocheck
import * as React from 'react'
import { Box, Text } from '../../ink'

export interface ToolUseMessageProps {
  /** Tool name, or a pre-formatted label such as `Read(src/main.ts)`. */
  name?: string
  content?: string
  /** Tool input; a `label` here replaces the raw content. */
  input?: { label?: string } & Record<string, unknown>
  status?: 'running' | 'success' | 'error'
}

/**
 * ToolUseMessage — renders a tool invocation row with a status glyph.
 * Mirrors the 'tool' rows of the ink screens.
 */
export function ToolUseMessage({ name, content, input, status = 'running' }: ToolUseMessageProps): React.ReactElement {
  const inputLabel = typeof input?.label === 'string' && input.label.trim() ? input.label.trim() : undefined
  const label = inputLabel ?? content ?? (name ? `${name}` : 'tool')
  return (
    <Box marginY={0} marginLeft={2}>
      {status === 'running' && <Text color="#d4b08c">⏺</Text>}
      {status === 'success' && <Text color="#3fb950">✓</Text>}
      {status === 'error' && <Text color="#f85149">✗</Text>}
      <Text color="#8b949e"> {label}</Text>
    </Box>
  )
}

export default ToolUseMessage
