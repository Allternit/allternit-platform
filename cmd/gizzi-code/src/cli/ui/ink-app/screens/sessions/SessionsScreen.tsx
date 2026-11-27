/**
 * SessionsScreen — every live agent session on this computer in one view
 * (`/sessions`; `/dash`, `/dashboard`, `/board`, `/live` and `/factory` open
 * it too). Gizzi sessions and Factory panes running other coding tools, from
 * the engine's `GET /api/factory/sessions`, polled every 2 s.
 *
 *   ↑/↓ (k/j)  move (←/→ and h in the grid)    g  list ⇄ grid
 *   Enter      attach to the session's pane (Gizzi comes back on detach)
 *   s          steer: type a line, Enter sends it once
 *   n          new session: "<cli> <task>" → `gizzi spawn <cli> <task> --window`
 *   x          stop: type the session's name to confirm (never stops without it)
 *   l          land a finished swarm worker (merge its branch; asks first)
 *   b          the swarm board      f  the factory floor
 *   r          poll now             q/Esc  back
 *
 * The pane engine keeps each session's real terminal; this view never draws
 * another tool's full screen.
 */
import { spawn as spawnProcess } from 'node:child_process'
import * as React from 'react'
import { Box, Text, useInput } from '../../ink'
import { useTerminalSize } from '../../hooks/useTerminalSize'
import { useSetAppState } from '../../state/AppState'
import { useRegisterOverlay } from '../../context/overlayContext'
import { attachArgv, paneSessionArgs, runEngine } from '@/cli/factory/engine'
import { landSwarm, openSwarms, readSwarm, stopWorker, type SwarmRecord, type SwarmWorker } from '@/cli/factory/swarm'
import { selfArgv } from '@/cli/mission/orchestrator'
import { fetchWithPlatformToken } from '@/runtime/bots/platform-api'
import { gatewayBase } from '@/shared/constants/allternitGateway'
import { handOff } from '../factory-floor/FactoryFloorScreen'
import { clip, SessionRow, SessionTile } from './SessionTile'
import { confirmMatches, parseNewSession, spawnCommandText, steerSession, type LiveSessionRow } from './liveSessions'
import { useLiveSessions } from './useLiveSessions'

type Mode =
  | { kind: 'browse' }
  | { kind: 'steer'; row: LiveSessionRow; text: string }
  | { kind: 'new'; text: string }
  | { kind: 'stop'; row: LiveSessionRow; text: string }
  | { kind: 'land'; row: LiveSessionRow; swarm: SwarmRecord; worker: SwarmWorker }

type Status = { text: string; tone: 'error' | 'success' | 'info' }

export function swarmWorkerFor(row: LiveSessionRow, swarms: SwarmRecord[]): { swarm: SwarmRecord; worker: SwarmWorker } | null {
  for (const swarm of swarms) {
    for (const worker of swarm.workers) {
      if ((row.botId && worker.agentId === row.botId) || (row.cwd && worker.workdir === row.cwd)) return { swarm, worker }
    }
  }
  return null
}

/** Engine argv that attaches to a session's pane, or null when it has none. */
export function attachArgvFor(row: LiveSessionRow): string[] | null {
  if (!row.paneId) return null
  if (row.terminalId) return ['pane', ...paneSessionArgs(), 'terminal', 'attach', row.terminalId]
  return attachArgv({ id: row.botId ?? row.address, pane: { id: row.paneId } })
}

/** The keys the selected session takes (the footer lists the rest). */
export function keysFor(row: LiveSessionRow, isWorker: boolean): string {
  const k: string[] = []
  if (row.paneId) k.push('enter attach')
  if (row.steer !== 'none') k.push('s steer')
  k.push('x stop')
  if (isWorker) k.push('l land')
  return k.join(' · ')
}

/** Stop one session. Only called after the typed confirm matched. */
async function stopSession(row: LiveSessionRow, worker: { swarm: SwarmRecord; worker: SwarmWorker } | null): Promise<string> {
  if (worker) {
    await stopWorker(readSwarm(worker.swarm.id) ?? worker.swarm, worker.worker.bot)
    return `■ stopped ${row.name}; its worktree stays at ${worker.worker.workdir}`
  }
  if (row.botId && row.botId.startsWith('local:')) {
    await runEngine(['agents', 'down', row.botId.slice('local:'.length)], { timeoutMs: 30000 })
    return `■ stopped ${row.name}`
  }
  if (row.terminalId) {
    const url = `${gatewayBase()}/terminal/${encodeURIComponent(row.terminalId)}/close`
    const init: RequestInit = { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}', signal: AbortSignal.timeout(8000) }
    const res = (await fetchWithPlatformToken(url, init)) ?? (await fetch(url, init))
    if (!res.ok) throw new Error(`The terminal refused to close (HTTP ${res.status})`)
    return `■ stopped ${row.name}`
  }
  throw new Error(`${row.name} runs in ${row.app ?? 'another terminal'}; stop it there`)
}

let spawnAvailable: boolean | null = null

/** Whether this gizzi has `gizzi spawn` (its help lists the command). */
function hasSpawn(): Promise<boolean> {
  if (spawnAvailable !== null) return Promise.resolve(spawnAvailable)
  const [bin, ...args] = selfArgv()
  return new Promise(resolve => {
    let out = ''
    const child = spawnProcess(bin!, [...args, '--help'], { stdio: ['ignore', 'pipe', 'pipe'], timeout: 10000 })
    child.stdout?.on('data', d => (out += String(d)))
    child.stderr?.on('data', d => (out += String(d)))
    child.on('error', () => resolve((spawnAvailable = false)))
    child.on('close', () => resolve((spawnAvailable = /\bspawn <cli>/.test(out))))
  })
}

function runSpawn(cli: string, prompt: string): Promise<string> {
  const [bin, ...args] = selfArgv()
  return new Promise((resolve, reject) => {
    let out = ''
    const argv = [...args, 'spawn', cli, ...(prompt ? [prompt] : []), '--window']
    const child = spawnProcess(bin!, argv, { stdio: ['ignore', 'pipe', 'pipe'], timeout: 60000 })
    child.stdout?.on('data', d => (out += String(d)))
    child.stderr?.on('data', d => (out += String(d)))
    child.on('error', reject)
    child.on('close', code => {
      const last = out.trim().split('\n').filter(Boolean).pop() ?? ''
      if (code === 0) resolve(`started ${cli}${last ? ` — ${clip(last, 80)}` : ''}; it shows here once it is running`)
      else reject(new Error(last || `gizzi spawn exited with code ${code}`))
    })
  })
}

export function SessionsScreen(): React.ReactNode {
  const { rows: termRows, columns } = useTerminalSize()
  const setAppState = useSetAppState()
  useRegisterOverlay('sessions', true)
  const { rows, lines, error, loaded, refresh } = useLiveSessions()

  const [grid, setGrid] = React.useState(false)
  const [selected, setSelected] = React.useState(0)
  const [mode, setMode] = React.useState<Mode>({ kind: 'browse' })
  const [status, setStatus] = React.useState<Status | null>(null)
  const [busy, setBusy] = React.useState<string | null>(null)
  const [now, setNow] = React.useState(Date.now())

  // Ages tick once a second; the list itself only changes on a poll.
  React.useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [])

  // Keep the selection on the same session when the list reorders.
  const selectedAddress = React.useRef<string | null>(null)
  const index = (() => {
    const at = selectedAddress.current ? rows.findIndex(r => r.address === selectedAddress.current) : -1
    return at >= 0 ? at : Math.min(selected, Math.max(0, rows.length - 1))
  })()
  const row: LiveSessionRow | undefined = rows[index]
  selectedAddress.current = row?.address ?? null

  const swarms = openSwarms()
  const workerOf = (r: LiveSessionRow) => swarmWorkerFor(r, swarms)

  const exit = () => setAppState(prev => (prev.screen === 'sessions' ? { ...prev, screen: 'prompt' } : prev))
  const select = (i: number) => {
    const next = Math.max(0, Math.min(Math.max(0, rows.length - 1), i))
    selectedAddress.current = rows[next]?.address ?? null
    setSelected(next)
  }

  const act = (label: string, fn: () => Promise<string>) => {
    setBusy(`${label}…`)
    fn()
      .then(text => setStatus({ text, tone: 'success' }))
      .catch((err: Error) => setStatus({ text: err?.message || String(err), tone: 'error' }))
      .finally(() => {
        setBusy(null)
        refresh()
      })
  }

  const attach = (r: LiveSessionRow) => {
    const argv = attachArgvFor(r)
    if (!argv) {
      setStatus({ text: `${r.name} runs in ${r.app ?? 'another terminal'}; it has no pane to attach to`, tone: 'error' })
      return
    }
    setStatus(null)
    try {
      const { exitCode } = handOff(argv)
      if (exitCode !== 0) setStatus({ text: `attach ${r.name} exited with code ${exitCode}`, tone: 'error' })
    } catch (err) {
      setStatus({ text: (err as Error).message, tone: 'error' })
    }
    refresh()
  }

  const startNew = (line: string) => {
    const parsed = parseNewSession(line)
    if (!parsed) return
    act(`starting ${parsed.cli}`, async () => {
      if (!(await hasSpawn())) return `Run this in a terminal: ${spawnCommandText(parsed.cli, parsed.prompt)}`
      return runSpawn(parsed.cli, parsed.prompt)
    })
  }

  const cols = grid ? Math.max(1, Math.floor((columns - 2) / 38)) : 1

  useInput((input, key) => {
    if (busy) return
    if (mode.kind === 'steer' || mode.kind === 'new' || mode.kind === 'stop') {
      if (key.escape) {
        setMode({ kind: 'browse' })
        setStatus({ text: 'cancelled', tone: 'info' })
        return
      }
      if (key.return) {
        const text = mode.text.trim()
        setMode({ kind: 'browse' })
        if (mode.kind === 'steer' && text) {
          const target = mode.row
          act(`steering ${target.name}`, async () => {
            const r = await steerSession(target, text)
            return `→ ${target.name}: ${r.state}`
          })
        } else if (mode.kind === 'new' && text) startNew(text)
        else if (mode.kind === 'stop') {
          const target = mode.row
          if (!confirmMatches(target, text)) {
            setStatus({ text: `Not stopped: type ${target.name} exactly to stop it`, tone: 'error' })
            return
          }
          act(`stopping ${target.name}`, () => stopSession(target, workerOf(target)))
        }
        return
      }
      if (key.backspace || key.delete) {
        setMode({ ...mode, text: mode.text.slice(0, -1) })
        return
      }
      if (input && !key.ctrl && !key.meta) setMode({ ...mode, text: mode.text + input.replace(/[\x00-\x1f]/g, '') })
      return
    }
    if (mode.kind === 'land') {
      const m = mode
      setMode({ kind: 'browse' })
      if (input === 'y' || input === 'Y') {
        act(`landing ${m.row.name}`, async () => {
          const st = (await landSwarm(readSwarm(m.swarm.id) ?? m.swarm, { bots: [m.worker.bot] }))[0]
          if (!st) return 'nothing to land'
          return `${st.bot}: ${st.outcome ?? st.action}${st.detail ? ` (${st.detail})` : st.outcome === 'skipped' ? ` (${st.reason})` : ''}`
        })
      } else setStatus({ text: 'cancelled', tone: 'info' })
      return
    }

    if (key.escape || input === 'q') return exit()
    if (key.upArrow || input === 'k') return select(index - cols)
    if (key.downArrow || input === 'j') return select(index + cols)
    if (grid && (key.leftArrow || input === 'h')) return select(index - 1)
    if (grid && key.rightArrow) return select(index + 1)
    if (input === 'g') return setGrid(g => !g)
    if (input === 'r') return refresh()
    if (input === 'b') return setAppState(prev => ({ ...prev, screen: 'board' }))
    if (input === 'f') return setAppState(prev => ({ ...prev, screen: 'factory' }))
    if (input === 'n') {
      setStatus(null)
      return setMode({ kind: 'new', text: '' })
    }
    if (!row) return
    if (key.return) return attach(row)
    if (input === 's') {
      if (row.steer === 'none') return setStatus({ text: `${row.name} can't be steered from here`, tone: 'error' })
      setStatus(null)
      return setMode({ kind: 'steer', row, text: '' })
    }
    if (input === 'x') {
      setStatus(null)
      return setMode({ kind: 'stop', row, text: '' })
    }
    if (input === 'l') {
      const w = workerOf(row)
      if (!w) return setStatus({ text: `${row.name} isn't a swarm worker; land its branch by hand`, tone: 'error' })
      return setMode({ kind: 'land', row, swarm: w.swarm, worker: w.worker })
    }
  })

  const width = Math.max(30, columns - 2)
  const counts = { working: 0, blocked: 0, idle: 0, done: 0 }
  for (const r of rows) counts[r.status]++
  const listBudget = Math.max(3, termRows - 9)
  const tileW = grid ? Math.floor(width / cols) : width
  const gridBudget = Math.max(1, Math.floor((termRows - 6) / 8)) * cols
  const start = grid
    ? Math.max(0, Math.floor(index / cols) * cols - Math.max(0, gridBudget - cols))
    : Math.max(0, index - listBudget + 4)
  const visible = rows.slice(start, start + (grid ? gridBudget : listBudget))

  const view = (r: LiveSessionRow, i: number) => {
    const sel = start + i === index
    const props = { row: r, lines: lines[r.address] ?? [], selected: sel, width: tileW, now, keys: sel ? keysFor(r, !!workerOf(r)) : '' }
    return grid ? <SessionTile key={r.address} {...props} /> : <SessionRow key={r.address} {...props} />
  }

  const promptLine =
    mode.kind === 'steer'
      ? `steer ${mode.row.name} › ${mode.text}▌  (Enter send · Esc cancel)`
      : mode.kind === 'new'
        ? `new session › ${mode.text}▌  (type "<cli> <task>", e.g. codex fix the tests · Esc cancel)`
        : mode.kind === 'stop'
          ? `Type ${mode.row.name} to stop it › ${mode.text}▌  (Esc cancel)`
          : mode.kind === 'land'
            ? `Land ${mode.row.name} (merge ${mode.worker.branch} into ${mode.swarm.baseBranch})? y/n`
            : null

  return (
    <Box flexDirection="column" width={columns} height={termRows} paddingX={1}>
      <Box flexDirection="row" justifyContent="space-between">
        <Text bold color="gizzi">
          Gizzi · Sessions · {rows.length} live
          {counts.blocked ? <Text color="warning"> · {counts.blocked} blocked</Text> : null}
          {counts.working ? <Text color="suggestion"> · {counts.working} working</Text> : null}
        </Text>
        <Text dimColor>{grid ? 'grid' : 'list'} · g to switch</Text>
      </Box>
      {error ? (
        <Text color="warning" wrap="truncate-end">
          {clip(error, width)}
        </Text>
      ) : null}
      <Box flexDirection="column" marginTop={1} flexGrow={1}>
        {!loaded ? (
          <Text dimColor>reading the engine…</Text>
        ) : rows.length === 0 ? (
          <Text dimColor>No live sessions. Press n to start one, or run `gizzi spawn claude`.</Text>
        ) : grid ? (
          <Box flexDirection="row" flexWrap="wrap">
            {visible.map(view)}
          </Box>
        ) : (
          visible.map(view)
        )}
      </Box>
      <Box flexDirection="column">
        {promptLine ? <Text color={mode.kind === 'stop' ? 'warning' : 'gizzi'}>{clip(promptLine, width)}</Text> : null}
        {status ? (
          <Text color={status.tone === 'error' ? 'error' : status.tone === 'success' ? 'success' : 'subtle'} wrap="truncate-end">
            {clip(status.text, width)}
          </Text>
        ) : null}
        <Box flexDirection="row" justifyContent="space-between">
          <Text dimColor wrap="truncate-end">
            {busy ?? '↑/↓ select · g grid · enter attach · s steer · n new · x stop · b board · f floor · r reload · q back'}
          </Text>
          <Text dimColor>/sessions</Text>
        </Box>
      </Box>
    </Box>
  )
}

export default SessionsScreen
