/**
 * FactoryFloorScreen — the Allternit Factory floor (`/factory`, or `f` on
 * the dashboard and bots screens, or ctrl+x f from the prompt).
 *
 * Mounted by REPL when AppState.screen === 'factory'. Everything shown comes
 * from the engine (`allternit-factory agents ps`, `workspace board`); a
 * section the engine can't answer shows "—" with the engine's own message.
 *
 *   ↑/↓ (k/j)  move           Tab    bots ⇄ needs you
 *   Enter      open the bot: Terminal → attach to its pane (engine view),
 *              Hosted → its session here in Gizzi, Vendor → thread + tickets
 *   a          approve the selected needs-you node
 *   w          the live wall (engine pane view; Gizzi resumes on exit)
 *   r          reload        q/Esc  back
 */
import * as React from 'react'
import { Box, Text, useInput } from '../../ink'
import instances from '../../ink/instances.js'
import { useTerminalSize } from '../../hooks/useTerminalSize'
import { useKeybinding } from '../../keybindings/useKeybinding'
import { useSetAppState } from '../../state/AppState'
import { useRegisterOverlay } from '../../context/overlayContext'
import { useNotifications } from '../../../../../context/notifications'
import { FactoryEngineError, passThroughSync } from '@/cli/factory/engine'
import { renderDocument } from '@/cli/factory/render'
import { openBotChat } from '../bots-pane/open-bot-chat'
import { approveNode, loadFactoryFloor, loadVendorThread, openActionFor, type FactoryFloorData, type Section } from './data'
import { handleFactoryFloorKey } from './keys'
import { NOT_BUILT_MARK, buildAgentRow, nodeRowText, truncate } from './rows'

type Focus = 'bots' | 'needs'

/** Release Ink's screen, hand the terminal to the engine, take it back. */
function handOff(args: string[]): { exitCode: number } {
  const ink = instances.get(process.stdout)
  ink?.enterAlternateScreen()
  try {
    return { exitCode: passThroughSync(args) }
  } finally {
    ink?.exitAlternateScreen()
  }
}

type Failed = Extract<Section<unknown>, { ok: false }>

function Unavailable({ section }: { section: Failed }): React.ReactNode {
  return (
    <Box flexDirection="column" paddingLeft={1}>
      <Text dimColor>
        {NOT_BUILT_MARK} {section.message}
      </Text>
      {section.action && !section.notBuilt ? <Text dimColor>{'  '}{section.action}</Text> : null}
    </Box>
  )
}

export function FactoryFloorScreen(): React.ReactNode {
  const { rows: termRows, columns } = useTerminalSize()
  const setAppState = useSetAppState()
  const { addNotification } = useNotifications()

  const [data, setData] = React.useState<FactoryFloorData | null>(null)
  const [loading, setLoading] = React.useState(true)
  const [tick, setTick] = React.useState(0)
  const [focus, setFocus] = React.useState<Focus>('bots')
  const [botIndex, setBotIndex] = React.useState(0)
  const [needIndex, setNeedIndex] = React.useState(0)
  const [busy, setBusy] = React.useState<string | null>(null)
  const [status, setStatus] = React.useState<{ text: string; tone: 'error' | 'success' } | null>(null)
  const [vendor, setVendor] = React.useState<{ address: string; text: string } | null>(null)

  useRegisterOverlay('factory', true)

  const exit = React.useCallback(() => {
    setAppState(prev => (prev.screen === 'factory' ? { ...prev, screen: 'prompt' } : prev))
  }, [setAppState])
  const refresh = React.useCallback(() => setTick(t => t + 1), [])

  React.useEffect(() => {
    let cancelled = false
    setLoading(true)
    loadFactoryFloor().then(next => {
      if (cancelled) return
      setData(next)
      setLoading(false)
    })
    return () => {
      cancelled = true
    }
  }, [tick])

  const agents = data?.agents.ok ? data.agents.data : []
  const board = data?.board.ok ? data.board.data : null
  const needs = board ? board.summary.needsYou : []
  const selectedAgent = agents[Math.min(botIndex, agents.length - 1)]
  const selectedNeed = needs[Math.min(needIndex, needs.length - 1)]

  const browsing = !busy && !vendor
  useKeybinding('factory:exit', exit, { context: 'Factory', isActive: browsing })

  const fail = (err: unknown) => {
    const text =
      err instanceof FactoryEngineError
        ? err.action
          ? `${err.fact} — ${err.action}`
          : err.fact
        : (err as Error)?.message || String(err)
    setStatus({ text, tone: 'error' })
  }

  const openWall = () => {
    setStatus(null)
    try {
      const { exitCode } = handOff(['agents', 'wall'])
      if (exitCode !== 0) setStatus({ text: `The live wall exited with code ${exitCode}`, tone: 'error' })
    } catch (err) {
      fail(err)
    }
    refresh()
  }

  const openSelected = () => {
    if (!selectedAgent) return
    const row = buildAgentRow(selectedAgent)
    const action = openActionFor(selectedAgent, row.address)
    setStatus(null)
    if (action.kind === 'attach') {
      try {
        const { exitCode } = handOff(['agents', 'attach', action.address])
        if (exitCode !== 0) setStatus({ text: `attach ${action.address} exited with code ${exitCode}`, tone: 'error' })
      } catch (err) {
        fail(err)
      }
      refresh()
    } else if (action.kind === 'session') {
      setBusy(`opening ${action.name}…`)
      openBotChat(action.name)
        .then(outcome => {
          setBusy(null)
          if (outcome.notice) {
            addNotification({ key: 'factory-open', text: outcome.notice, color: 'warning', priority: 'immediate', timeoutMs: 8000 })
          }
          exit()
        })
        .catch(err => {
          setBusy(null)
          fail(err)
        })
    } else if (action.kind === 'vendor') {
      setBusy(`reading ${action.address}…`)
      loadVendorThread(action.address).then(section => {
        setBusy(null)
        setVendor({
          address: action.address,
          text: section.ok ? renderDocument(section.data) : `${NOT_BUILT_MARK} ${(section as Failed).message}`,
        })
      })
    } else {
      setStatus({ text: action.reason, tone: 'error' })
    }
  }

  const approveSelected = () => {
    if (focus !== 'needs' || !selectedNeed) return
    setBusy(`approving ${selectedNeed.nodeId}…`)
    approveNode(selectedNeed).then(result => {
      setBusy(null)
      setStatus({ text: result.message, tone: result.ok ? 'success' : 'error' })
      if (result.ok) refresh()
    })
  }

  useInput((input, key) => {
    if (busy) return
    if (vendor) {
      if (key.escape || input === 'q' || key.return) setVendor(null)
      return
    }
    const listLen = focus === 'bots' ? agents.length : needs.length
    const setIndex = focus === 'bots' ? setBotIndex : setNeedIndex
    handleFactoryFloorKey(input, key, {
      moveUp: () => setIndex(i => Math.max(0, i - 1)),
      moveDown: () => setIndex(i => Math.min(Math.max(0, listLen - 1), i + 1)),
      switchFocus: () => setFocus(f => (f === 'bots' ? 'needs' : 'bots')),
      openSelected: () => {
        if (focus === 'bots') openSelected()
      },
      approveSelected,
      openWall,
      refresh,
      exit,
    })
  })

  const width = Math.max(20, columns - 4)
  const header = (title: string, active: boolean, count: number | null) => (
    <Text bold color={active ? 'gizzi' : 'text'}>
      {title}
      {count === null ? '' : ` (${count})`}
    </Text>
  )

  const rowsBudget = Math.max(3, Math.floor((termRows - 12) / 2))

  return (
    <Box flexDirection="column" width={columns} height={termRows} paddingX={1}>
      <Box flexDirection="row" justifyContent="space-between">
        <Text bold color="gizzi">
          Gizzi · Factory floor
        </Text>
        <Text dimColor>
          {loading
            ? 'reading the engine…'
            : board
              ? `${truncate(board.campaign.title, 40)} · proven ${board.summary.proven.k}/${board.summary.proven.n}`
              : `${agents.length} bot${agents.length === 1 ? '' : 's'}`}
        </Text>
      </Box>

      {vendor ? (
        <Box flexDirection="column" marginTop={1}>
          <Text bold>{vendor.address} · thread and tickets</Text>
          <Text>{vendor.text}</Text>
          <Text dimColor>Enter/q/Esc back</Text>
        </Box>
      ) : (
        <>
          <Box flexDirection="column" marginTop={1}>
            {header('Bots', focus === 'bots', data?.agents.ok ? agents.length : null)}
            {!data ? null : !data.agents.ok ? (
              <Unavailable section={data.agents as Failed} />
            ) : agents.length === 0 ? (
              <Text dimColor> No bots are running. Start a team with `gizzi agents up`.</Text>
            ) : (
              agents.slice(0, rowsBudget).map((a, i) => {
                const r = buildAgentRow(a)
                const sel = focus === 'bots' && i === botIndex
                return (
                  <Box key={a.id} flexDirection="row" paddingLeft={1}>
                    <Text color={sel ? 'gizzi' : 'subtle'}>{sel ? '❯ ' : '  '}</Text>
                    <Text bold={sel} color={sel ? 'text' : 'subtle'} wrap="truncate-end">
                      {truncate(r.address, 24).padEnd(24)}
                    </Text>
                    <Text color="gizzi">{' '}{truncate(r.badge, 30).padEnd(30)}</Text>
                    <Text color={r.stateColor}>{' '}{r.state.padEnd(10)}</Text>
                    <Text dimColor wrap="truncate-end">
                      {' '}
                      {truncate(r.node, Math.max(8, width - 80))}
                    </Text>
                    <Text>{'  '}{r.proof}</Text>
                  </Box>
                )
              })
            )}
            {data?.agents.ok && agents.length > rowsBudget ? (
              <Text dimColor>{'  '}+{agents.length - rowsBudget} more — `gizzi agents ps`</Text>
            ) : null}
          </Box>

          <Box flexDirection="column" marginTop={1}>
            {header('Needs you', focus === 'needs', board ? needs.length : null)}
            {!data ? null : !data.board.ok ? (
              <Unavailable section={data.board as Failed} />
            ) : needs.length === 0 ? (
              <Text dimColor>{'  '}Nothing is waiting on you.</Text>
            ) : (
              needs.slice(0, rowsBudget).map((n, i) => {
                const sel = focus === 'needs' && i === needIndex
                return (
                  <Box key={`${n.dagId}/${n.nodeId}`} paddingLeft={1}>
                    <Text color={sel ? 'gizzi' : 'subtle'}>{sel ? '❯ ' : '  '}</Text>
                    <Text bold={sel} color="warning" wrap="truncate-end">
                      {truncate(nodeRowText(n), width - 4)}
                    </Text>
                  </Box>
                )
              })
            )}
          </Box>

          {board ? (
            <Box flexDirection="column" marginTop={1}>
              {header('Board', false, null)}
              <Text>
                {'  '}
                <Text dimColor>Now </Text>
                {board.summary.now.length === 0 ? NOT_BUILT_MARK : truncate(board.summary.now.map(n => n.title).join(' · '), width - 8)}
              </Text>
              <Text>
                {'  '}
                <Text dimColor>Next </Text>
                {board.summary.next.length === 0 ? NOT_BUILT_MARK : truncate(board.summary.next.map(n => n.title).join(' · '), width - 9)}
              </Text>
            </Box>
          ) : null}
        </>
      )}

      <Box flexDirection="column" marginTop={1}>
        {status ? (
          <Text color={status.tone === 'error' ? 'error' : 'success'} wrap="truncate-end">
            {truncate(status.text, columns - 2)}
          </Text>
        ) : null}
        <Box flexDirection="row" justifyContent="space-between">
          <Text dimColor>
            {busy ?? '↑/↓ select · Tab bots/needs you · Enter open · a approve · w live wall · r reload · q/Esc back'}
          </Text>
          <Text dimColor>/factory</Text>
        </Box>
      </Box>
    </Box>
  )
}

export default FactoryFloorScreen
