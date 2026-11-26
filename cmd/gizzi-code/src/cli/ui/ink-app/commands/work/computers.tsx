import * as React from 'react'
import { useState } from 'react'
import { Pane } from '../../components/design-system/Pane'
import { BlockTitle } from '../../components/BlockTitle'
import { Box, Text, useInput } from '../../ink'
import type { LocalJSXCommandCall } from '../../types/command.js'
import { bytes, formatPorts, formatServices, uptime, type ConsoleRow } from '@/cli/work/console'
import { loadDetail, loadRows } from '@/cli/work/console-read'
import { attentionReasons, ATTENTION_GROUP, flatRows, groupRows, isAsleep, meterLine } from '@/cli/work/console-view'
import { useLoaded } from './WorkPane'
import { EmptyLine } from '../../components/EmptyLine'

/**
 * /computers: the Computers console in the terminal. Grouped list (Needs
 * attention first) with status and meters from the last sample; enter opens a
 * computer (o overview, s services, p ports); r refreshes. Read-only: an asleep
 * computer shows its last sample and is never woken. Waking, sleeping and
 * snapshots stay in `gizzi computers …`, which asks first.
 */
type Tab = 'overview' | 'services' | 'ports'

function Detail({ row, onBack }: { row: ConsoleRow; onBack: () => void }): React.ReactNode {
  const { data, error, reload } = useLoaded(() => loadDetail(row))
  const [tab, setTab] = useState<Tab>('overview')
  useInput((input, key) => {
    if (key.escape) return onBack()
    if (input === 'r') return reload()
    if (input === 'o') setTab('overview')
    if (input === 's') setTab('services')
    if (input === 'p') setTab('ports')
  })
  const snap = data?.snapshot
  return (
    <Pane>
      <Box>
        <Text bold>{row.name}</Text>
        <Text dimColor> · {data?.asleep ? 'asleep, last sample' : (row.status ?? 'unknown')}</Text>
      </Box>
      {error ? <Text color="warning">{error}</Text> : null}
      {!data && !error ? <Text dimColor>Loading…</Text> : null}
      {data && tab === 'overview' ? (
        <Box flexDirection="column">
          {snap ? (
            <>
              <Text>{snap.os.name} {snap.os.version} ({snap.os.arch}) · up {uptime(snap.uptimeSec)}</Text>
              <Text>{meterLine(snap)}</Text>
              <Text>Memory {bytes(snap.memory.usedBytes)} of {bytes(snap.memory.totalBytes)}</Text>
              {snap.disk.map(d => (
                <Text key={d.mount}>Disk {d.mount} {bytes(d.usedBytes)} of {bytes(d.totalBytes)}</Text>
              ))}
              <Text dimColor>Sampled {snap.sampledAt}</Text>
            </>
          ) : (
            <Text dimColor>No health sample yet.</Text>
          )}
          {attentionReasons(row).map(r => (
            <Text key={r} color="warning">⚠ {r}</Text>
          ))}
          {data.asleep ? <Text dimColor>Asleep. Wake it with: gizzi computers wake "{row.name}"</Text> : null}
        </Box>
      ) : null}
      {data && tab === 'services' ? <Text>{data.asleep ? 'Asleep: services are not available until it wakes.' : formatServices(data.services.slice(0, 20))}</Text> : null}
      {data && tab === 'ports' ? <Text>{data.asleep ? 'Asleep: ports are not available until it wakes.' : formatPorts(data.ports.slice(0, 20))}</Text> : null}
      <Box marginTop={1}>
        <Text dimColor>o overview · s services · p ports · r refresh · esc back</Text>
      </Box>
    </Pane>
  )
}

export function ComputersPane({ onDone }: { onDone: () => void }): React.ReactNode {
  const { data, error, reload } = useLoaded(loadRows, 30_000)
  const [index, setIndex] = useState(0)
  const [open, setOpen] = useState<ConsoleRow | undefined>()
  const groups = React.useMemo(() => groupRows(data ?? []), [data])
  const rows = flatRows(groups)
  const at = Math.min(index, Math.max(0, rows.length - 1))
  useInput((input, key) => {
    if (open) return
    if (key.escape) return onDone()
    if (key.upArrow) return setIndex(Math.max(0, at - 1))
    if (key.downArrow) return setIndex(Math.min(rows.length - 1, at + 1))
    if (input === 'r') return reload()
    if (key.return && rows[at]) setOpen(rows[at])
  })
  if (open) return <Detail row={open} onBack={() => setOpen(undefined)} />
  let n = -1
  return (
    <Pane>
      <Box>
        <BlockTitle title="COMPUTERS" />
        {data ? <Text dimColor> · {data.length}</Text> : null}
      </Box>
      {error ? <Text color="warning">{error}</Text> : null}
      {!data && !error ? <Text dimColor>Loading…</Text> : null}
      {data && data.length === 0 ? <EmptyLine>No computers yet. Link one with: gizzi computers link</EmptyLine> : null}
      {groups.map(g => (
        <Box key={g.name} flexDirection="column" marginTop={1}>
          <Text bold color={g.name === ATTENTION_GROUP ? 'warning' : undefined}>{g.name}</Text>
          {g.rows.map(r => {
            n += 1
            const selected = n === at
            return (
              <Box key={r.id}>
                <Text color={selected ? 'suggestion' : undefined}>{selected ? '❯ ' : '  '}</Text>
                <Text wrap="truncate-end">
                  {r.name}
                  <Text dimColor> · {isAsleep(r) ? 'asleep · last sample' : (r.status ?? 'unknown')} · {meterLine(r.lastSample)}</Text>
                </Text>
              </Box>
            )
          })}
        </Box>
      ))}
      <Box marginTop={1}>
        <Text dimColor>↑/↓ choose · enter open · r refresh · esc close</Text>
      </Box>
    </Pane>
  )
}

export const call: LocalJSXCommandCall = async (onDone, context) => {
  if (context.options?.isNonInteractiveSession) {
    const list = await loadRows().catch((e: Error) => e)
    onDone(
      list instanceof Error
        ? list.message
        : groupRows(list)
            .map(g => `${g.name}\n${g.rows.map(r => `  ${r.name} · ${isAsleep(r) ? 'asleep' : (r.status ?? 'unknown')} · ${meterLine(r.lastSample)}`).join('\n')}`)
            .join('\n') || 'No computers yet.',
      { display: 'system' },
    )
    return null
  }
  return <ComputersPane onDone={() => onDone(undefined, { display: 'skip' })} />
}
