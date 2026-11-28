/**
 * One live session, drawn two ways: a list row (`SessionRow`) and a grid
 * tile (`SessionTile`). Pure: everything comes in as props, so render tests
 * draw them without the REPL.
 */
import * as React from 'react'
import { Box, Text } from '../../ink'
import { STATUS_COLOR, ageText, type LiveSessionRow } from './liveSessions'

export function clip(s: string, n: number): string {
  if (n <= 0) return ''
  return s.length > n ? `${s.slice(0, Math.max(0, n - 1))}…` : s
}

export interface SessionViewProps {
  row: LiveSessionRow
  lines: string[]
  selected: boolean
  width: number
  now: number
  /** The keys this session takes, shown under the selected one. */
  keys: string
}

export function SessionRow({ row, lines, selected, width, now, keys }: SessionViewProps): React.ReactNode {
  const nameW = Math.min(24, Math.max(10, Math.floor(width * 0.22)))
  const rtW = 8
  const folderW = Math.min(18, Math.max(8, Math.floor(width * 0.16)))
  const statusW = 8
  const ageW = 5
  const doingW = Math.max(8, width - nameW - rtW - folderW - statusW - ageW - 8)
  const last = lines[lines.length - 1]
  return (
    <Box flexDirection="column">
      <Box flexDirection="row">
        <Text color={selected ? 'gizzi' : 'subtle'}>{selected ? '❯ ' : '  '}</Text>
        <Text bold={selected} color={selected ? 'text' : 'subtle'}>
          {clip(row.name, nameW).padEnd(nameW)}
        </Text>
        <Text color="gizzi"> {clip(row.runtime, rtW).padEnd(rtW)}</Text>
        <Text dimColor> {clip(row.folder, folderW).padEnd(folderW)}</Text>
        <Text bold={row.status === 'blocked'} color={STATUS_COLOR[row.status]}>
          {' '}
          {row.status.padEnd(statusW)}
        </Text>
        <Text dimColor> {ageText(row.since, now).padStart(ageW - 1)}</Text>
        <Text wrap="truncate-end"> {clip(row.doing ?? last ?? '', doingW)}</Text>
      </Box>
      {selected && lines.length > 0 ? (
        <Box flexDirection="column" paddingLeft={4}>
          {lines.slice(-3).map((l, i) => (
            <Text key={i} dimColor wrap="truncate-end">
              {clip(l, width - 6)}
            </Text>
          ))}
        </Box>
      ) : null}
      {selected && keys ? (
        <Box paddingLeft={4}>
          <Text dimColor>{keys}</Text>
        </Box>
      ) : null}
    </Box>
  )
}

export function SessionTile({ row, lines, selected, width, now, keys }: SessionViewProps): React.ReactNode {
  const inner = Math.max(10, width - 4)
  const shown = lines.slice(-3)
  return (
    <Box
      flexDirection="column"
      width={width}
      borderStyle="round"
      borderColor={selected ? 'gizzi' : row.status === 'blocked' ? 'warning' : 'subtle'}
      paddingX={1}
    >
      <Box flexDirection="row" justifyContent="space-between">
        <Text bold color={selected ? 'gizzi' : 'text'}>
          {clip(row.name, inner - 10)}
        </Text>
        <Text bold={row.status === 'blocked'} color={STATUS_COLOR[row.status]}>
          {row.status}
        </Text>
      </Box>
      <Text dimColor>{clip(`${row.runtime} · ${row.folder || '—'} · ${ageText(row.since, now) || 'now'}`, inner)}</Text>
      <Text wrap="truncate-end">{clip(row.doing ?? '—', inner)}</Text>
      {[0, 1, 2].map(i => (
        <Text key={i} dimColor wrap="truncate-end">
          {clip(shown[i] ?? ' ', inner)}
        </Text>
      ))}
      {selected && keys ? <Text dimColor>{clip(keys, inner)}</Text> : null}
    </Box>
  )
}
