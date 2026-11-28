import { toString as qrToString } from 'qrcode'
import * as React from 'react'
import { useEffect, useState } from 'react'
import { Pane } from '../../components/design-system/Pane'
import { Box, Text } from '../../ink'
import { useKeybinding } from '../../keybindings/useKeybinding'
import { fetchPaneSessions, getLiveSession, liveLinks, onLiveSessionChange, paneSessionLine, type LiveLinks, type PaneSession } from '../../services/liveSession'
import type { LocalJSXCommandCall } from '../../types/command'

function useLive() {
  const [live, setLive] = useState(() => ({ ...getLiveSession() }))
  useEffect(() => onLiveSessionChange(() => setLive({ ...getLiveSession() })), [])
  return live
}

type Reach = { reachable: boolean; via?: 'gizzi' | 'desktop' }

/** Whether a phone or the web can reach this computer: the gizzi engine's cloud relay or Desktop holds it. */
function useReachability(): Reach | null {
  const [reach, setReach] = useState<Reach | null>(null)
  useEffect(() => {
    let alive = true
    const check = () =>
      void import('@/runtime/integrations/relay-connector')
        .then(({ RelayConnector }) => RelayConnector.reachability())
        .catch(() => ({ reachable: false }))
        .then(next => {
          if (alive) setReach(next)
        })
    check()
    const timer = setInterval(check, 5_000)
    return () => {
      alive = false
      clearInterval(timer)
    }
  }, [])
  return reach
}

function ReachabilityLine(): React.ReactNode {
  const reach = useReachability()
  if (!reach) return <Text dimColor>Checking whether your devices can reach this computer…</Text>
  if (reach.reachable) {
    return (
      <Text color="success">
        Reachable from your devices
        <Text dimColor>{reach.via === 'desktop' ? ' (through Allternit Desktop)' : ' (through gizzi serve)'}</Text>
      </Text>
    )
  }
  return (
    <Text color="warning">
      Not reachable from other devices: open Allternit Desktop, or run `gizzi pair` then `gizzi serve`
    </Text>
  )
}

/** Other CLIs running in panes on this computer, with who can reach each one. */
export function PaneSessionList({ rows }: { rows: PaneSession[] }): React.ReactNode {
  if (rows.length === 0) return null
  return (
    <Box flexDirection="column" marginTop={1}>
      <Text bold>Other sessions on this computer</Text>
      {rows.map(r => (
        <Text key={r.address} color={r.status === 'blocked' ? 'warning' : undefined}>
          {paneSessionLine(r)}
        </Text>
      ))}
    </Box>
  )
}

function usePaneSessions(): PaneSession[] {
  const [rows, setRows] = useState<PaneSession[]>([])
  useEffect(() => {
    let alive = true
    const load = () =>
      void fetchPaneSessions()
        .then(next => alive && setRows(next))
        .catch(() => alive && setRows([]))
    load()
    const timer = setInterval(load, 5_000)
    return () => {
      alive = false
      clearInterval(timer)
    }
  }, [])
  return rows
}

export function LiveRemote({ onDone }: { onDone: () => void }): React.ReactNode {
  const live = useLive()
  const [links, setLinks] = useState<LiveLinks | null>(null)
  const [qr, setQr] = useState('')
  const panes = usePaneSessions()
  useKeybinding('confirm:no', onDone, { context: 'Confirmation' })

  useEffect(() => {
    if (!live.sessionID) return
    void liveLinks(live.sessionID).then(async next => {
      setLinks(next)
      if (next.phone) setQr(await qrToString(next.phone, { type: 'utf8', errorCorrectionLevel: 'L' }))
    })
  }, [live.sessionID])

  if (!live.sessionID) {
    return (
      <Pane>
        <Text color="warning">This session is not live yet: the Allternit engine is not reachable at {live.engineURL}.</Text>
        <Text dimColor>Open Allternit Desktop (or run `gizzi serve`) and it connects within 15 seconds.</Text>
        <ReachabilityLine />
        <PaneSessionList rows={panes} />
        <Text dimColor>(press esc to close)</Text>
      </Pane>
    )
  }

  return (
    <Pane>
      <Box marginBottom={1}>
        <Text bold>Continue this session anywhere</Text>
        <Text dimColor> · {live.connected ? 'live' : 'reconnecting…'} · {live.sessionID}</Text>
      </Box>
      <Box marginBottom={1}>
        <ReachabilityLine />
      </Box>
      {links?.phone ? (
        <>
          {qr
            .split('\n')
            .filter(line => line.length > 0)
            .map((line, i) => (
              <Text key={i}>{line}</Text>
            ))}
          <Box marginTop={1}>
            <Text dimColor>Phone: </Text>
            <Text color="ide">{links.phone}</Text>
          </Box>
          <Box>
            <Text dimColor>Web:   </Text>
            <Text color="ide">{links.web}</Text>
          </Box>
        </>
      ) : links ? (
        <Text color="warning">Sign in with `gizzi login` to open this session on your phone or the web.</Text>
      ) : (
        <Text dimColor>Making the link…</Text>
      )}
      <Box>
        <Text dimColor>Desktop: </Text>
        <Text color="ide">/desktop</Text>
        <Text dimColor> (it is already in Desktop's session list)</Text>
      </Box>
      <PaneSessionList rows={panes} />
      <Box marginTop={1}>
        <Text dimColor>Messages, Stop and approvals from any of them reach this terminal at once. (press esc to close)</Text>
      </Box>
    </Pane>
  )
}

export const call: LocalJSXCommandCall = async onDone => {
  return <LiveRemote onDone={() => onDone()} />
}
