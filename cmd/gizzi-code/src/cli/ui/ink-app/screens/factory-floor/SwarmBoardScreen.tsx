/**
 * SwarmBoardScreen — the live Factory board inside the TUI (`/board`, or
 * `b` on the factory floor). Same rows as `gizzi board` (cli/factory/board.ts),
 * fed by the engine's event stream and each pane's screen stream.
 *
 *   ↑/↓ (k/j)  move        1-9  jump      n  next waiting
 *   Enter      attach to the bot's pane (engine view; Gizzi resumes on detach)
 *   s          steer: type a message, Enter sends it to the bot
 *   x          stop the bot (asks first)
 *   l          land a finished swarm worker (merge its branch; asks first)
 *   f          the factory floor        q/Esc  back
 */
import * as React from 'react'
import { Box, Text, useInput } from '../../ink'
import instances from '../../ink/instances.js'
import { useTerminalSize } from '../../hooks/useTerminalSize'
import { useSetAppState } from '../../state/AppState'
import { useRegisterOverlay } from '../../context/overlayContext'
import { attachArgv, passThroughSync } from '@/cli/factory/engine'
import { FactoryLive, type LiveView } from '@/cli/factory/live'
import { BOARD_KEYS, allRows, buildBoard, rowColumns, summarize, summaryText, type BoardRow, type BoardState } from '@/cli/factory/board'
import { drainReady, landSwarm, notePending, observeSwarm, openSwarms, readSwarm, steer, stopAgent, stopWorker } from '@/cli/factory/swarm'
import { truncate } from './rows'

const STATE_COLOR: Record<BoardState, 'suggestion' | 'warning' | 'success' | 'error' | 'subtle'> = {
  starting: 'subtle',
  working: 'suggestion',
  waiting: 'warning',
  idle: 'subtle',
  done: 'success',
  failed: 'error',
  stopped: 'subtle',
}

type Confirm = { text: string; run: () => Promise<string> }

export function SwarmBoardScreen(): React.ReactNode {
  const { rows: termRows, columns } = useTerminalSize()
  const setAppState = useSetAppState()
  useRegisterOverlay('board', true)

  const liveRef = React.useRef<FactoryLive | null>(null)
  const [view, setView] = React.useState<LiveView>({ connected: false, error: null, agents: [], lastLine: {}, since: {} })
  const [, setClock] = React.useState(0)
  const [selected, setSelected] = React.useState(0)
  const [prompt, setPrompt] = React.useState<string | null>(null)
  const [confirm, setConfirm] = React.useState<Confirm | null>(null)
  const [status, setStatus] = React.useState<{ text: string; tone: 'error' | 'success' | 'info' } | null>(null)
  const [busy, setBusy] = React.useState<string | null>(null)

  React.useEffect(() => {
    const live = new FactoryLive({ lines: true })
    liveRef.current = live
    const off = live.subscribe(setView)
    void live.start()
    // Elapsed column only; the data comes from the engine's streams.
    const tick = setInterval(() => setClock(c => c + 1), 1000)
    return () => {
      clearInterval(tick)
      off()
      live.stop()
    }
  }, [])

  const swarms = openSwarms().map(s => (view.connected ? observeSwarm(s, view.agents) : s))
  const groups = buildBoard({ agents: view.agents, swarms, lastLine: view.lastLine, since: view.since })
  const rows = allRows(groups)
  const sel = Math.min(selected, Math.max(0, rows.length - 1))
  const row: BoardRow | undefined = rows[sel]

  // Deliver queued tasks/steers once a worker's pane is free.
  React.useEffect(() => {
    if (view.connected) for (const s of openSwarms()) drainReady(s, view.agents)
  }, [view])

  const exit = () => setAppState(prev => (prev.screen === 'board' ? { ...prev, screen: 'prompt' } : prev))

  const act = (label: string, fn: () => Promise<string>) => {
    setBusy(`${label}…`)
    fn()
      .then(text => setStatus({ text, tone: 'success' }))
      .catch((err: { fact?: string; action?: string | null; message?: string }) =>
        setStatus({ text: `${err.fact ?? err.message}${err.action ? ` — ${err.action}` : ''}`, tone: 'error' }),
      )
      .finally(() => {
        setBusy(null)
        liveRef.current?.refresh()
      })
  }

  const attach = (r: BoardRow) => {
    if (!r.attachable) {
      setStatus({ text: `${r.address} has no pane to attach to`, tone: 'error' })
      return
    }
    const ink = instances.get(process.stdout)
    ink?.enterAlternateScreen()
    try {
      const code = passThroughSync(r.agent ? attachArgv(r.agent) : ['agents', 'attach', r.address])
      if (code !== 0) setStatus({ text: `attach ${r.address} exited with code ${code}`, tone: 'error' })
    } catch (err) {
      setStatus({ text: (err as Error).message, tone: 'error' })
    } finally {
      ink?.exitAlternateScreen()
      liveRef.current?.refresh()
    }
  }

  useInput((input, key) => {
    if (busy) return
    if (prompt !== null) {
      if (key.return) {
        const text = prompt.trim()
        setPrompt(null)
        if (text && row) {
          const target = row
          act(`steering ${target.address}`, async () => {
            const r = await steer(target.address, text, target.swarm?.root)
            if (r.state === 'queued' && target.swarm && target.worker) notePending(target.swarm, target.worker.bot)
            return `→ ${target.address}: ${r.state}${r.detail ? ` (${r.detail})` : ''}`
          })
        }
      } else if (key.escape) setPrompt(null)
      else if (key.backspace || key.delete) setPrompt(p => (p ?? '').slice(0, -1))
      else if (input && !key.ctrl && !key.meta) setPrompt(p => (p ?? '') + input.replace(/[\x00-\x1f]/g, ''))
      return
    }
    if (confirm) {
      const c = confirm
      setConfirm(null)
      if (input === 'y' || input === 'Y') act(c.text.replace(/\?$/, ''), c.run)
      else setStatus({ text: 'cancelled', tone: 'info' })
      return
    }
    if (key.escape || input === 'q') return exit()
    if (key.upArrow || input === 'k') return setSelected(Math.max(0, sel - 1))
    if (key.downArrow || input === 'j') return setSelected(Math.min(Math.max(0, rows.length - 1), sel + 1))
    if (/^[1-9]$/.test(input)) return setSelected(Math.min(Math.max(0, rows.length - 1), Number(input) - 1))
    if (input === 'f') return setAppState(prev => ({ ...prev, screen: 'factory' }))
    if (input === 'n') {
      for (let k = 1; k <= rows.length; k++) {
        const j = (sel + k) % rows.length
        if (rows[j]!.state === 'waiting') return setSelected(j)
      }
      return
    }
    if (!row) return
    if (key.return) return attach(row)
    if (input === 's') {
      setStatus(null)
      return setPrompt('')
    }
    if (input === 'x') {
      const r = row
      return setConfirm({
        text: `Stop ${r.address}?`,
        run: async () => {
          if (r.swarm && r.worker) {
            await stopWorker(readSwarm(r.swarm.id) ?? r.swarm, r.worker.bot)
            return `■ stopped ${r.address}; its worktree stays at ${r.worker.workdir} (gizzi swarm land ${r.swarm.id} --worker ${r.worker.bot} merges it)`
          }
          if (r.agent) await stopAgent(r.agent)
          return `■ stopped ${r.address}`
        },
      })
    }
    if (input === 'l') {
      if (!row.swarm || !row.worker) return setStatus({ text: `${row.address} isn't a swarm worker; land its branch by hand`, tone: 'error' })
      const sw = row.swarm
      const bot = row.worker.bot
      return setConfirm({
        text: `Land ${row.address} (merge ${row.worker.branch} into ${sw.baseBranch})?`,
        run: async () => {
          const st = (await landSwarm(readSwarm(sw.id) ?? sw, { bots: [bot] }))[0]
          if (!st) return 'nothing to land'
          return `${st.bot}: ${st.outcome ?? st.action}${st.detail ? ` (${st.detail})` : st.outcome === 'skipped' ? ` (${st.reason})` : ''}`
        },
      })
    }
  })

  const width = Math.max(40, columns - 4)
  const sum = summarize(rows)
  const budget = Math.max(2, Math.floor((termRows - 8) / 2))
  let index = -1

  return (
    <Box flexDirection="column" width={columns} height={termRows} paddingX={1}>
      <Box flexDirection="row" justifyContent="space-between">
        <Text bold color="gizzi">
          Gizzi · Factory board · {summaryText(sum)}
        </Text>
        <Text color={view.connected ? 'success' : 'warning'}>{view.connected ? '⟳ live' : '○ offline'}</Text>
      </Box>
      {!view.connected && view.error ? <Text color="warning" wrap="truncate-end">{view.error}</Text> : null}
      <Text dimColor wrap="truncate-end">
        {'  #   BOT              BRAIN'.padEnd(24 + Math.min(30, Math.max(10, Math.floor(width * 0.24))))}
        {'STATE      AGE      COST  BRANCH'}
      </Text>
      {view.connected && rows.length === 0 ? (
        <Text dimColor> Nothing is running. Start a swarm: /swarm &lt;task&gt;, or gizzi swarm "&lt;task&gt;" --workers 2</Text>
      ) : null}
      {groups.map(g => (
        <Box key={g.title} flexDirection="column">
          <Text dimColor wrap="truncate-end">─ {truncate(g.title, width - 2)}</Text>
          {g.rows.map(r => {
            index++
            if (index >= budget) return null
            const c = rowColumns(r, index, width)
            const on = index === sel
            return (
              <Box key={r.key} flexDirection="column">
                <Box flexDirection="row">
                  <Text color={on ? 'gizzi' : 'subtle'}>{on ? '❯ ' : '  '}</Text>
                  <Text bold={on} wrap="truncate-end">
                    {c.num} {c.bot} {c.brain}{' '}
                  </Text>
                  <Text color={STATE_COLOR[r.state]}>{c.state}</Text>
                  <Text> {c.age} {c.cost} </Text>
                  <Text dimColor wrap="truncate-end">{c.branch}</Text>
                </Box>
                {r.lastLine ? (
                  <Text color={r.state === 'waiting' ? 'warning' : undefined} dimColor={r.state !== 'waiting'} wrap="truncate-end">
                    {'      └ '}
                    {truncate(r.lastLine, width - 8)}
                  </Text>
                ) : null}
              </Box>
            )
          })}
        </Box>
      ))}
      {rows.length > budget ? <Text dimColor>  +{rows.length - budget} more — gizzi board in a bigger terminal</Text> : null}
      <Box flexGrow={1} />
      {prompt !== null ? (
        <Text>
          <Text color="gizzi">steer {row?.address} › </Text>
          {prompt}
          <Text inverse> </Text>
        </Text>
      ) : confirm ? (
        <Text color="warning">{confirm.text} (y/n)</Text>
      ) : status ? (
        <Text color={status.tone === 'error' ? 'error' : status.tone === 'success' ? 'success' : undefined} wrap="truncate-end">
          {truncate(status.text, columns - 2)}
        </Text>
      ) : null}
      <Box flexDirection="row" justifyContent="space-between">
        <Text dimColor wrap="truncate-end">{busy ?? `${BOARD_KEYS.replace(' · q quit', '')} · f floor · q/Esc back`}</Text>
        <Text dimColor>/board</Text>
      </Box>
    </Box>
  )
}

export default SwarmBoardScreen
