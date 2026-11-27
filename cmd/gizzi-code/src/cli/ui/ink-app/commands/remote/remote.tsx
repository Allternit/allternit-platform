import { toString as qrToString } from 'qrcode'
import * as React from 'react'
import { useEffect, useState } from 'react'
import { Pane } from '../../components/design-system/Pane'
import { Box, Text } from '../../ink'
import { useKeybinding } from '../../keybindings/useKeybinding'
import { getLiveSession, liveLinks, onLiveSessionChange, type LiveLinks } from '../../services/liveSession'
import type { LocalJSXCommandCall } from '../../types/command'
import { remoteSubcommand } from './subcommand'

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

export function LiveRemote({ onDone }: { onDone: () => void }): React.ReactNode {
  const live = useLive()
  const [links, setLinks] = useState<LiveLinks | null>(null)
  const [qr, setQr] = useState('')
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
        <Text color="ide">/remote desktop</Text>
        <Text dimColor> (it is already in Desktop's session list)</Text>
      </Box>
      <Box>
        <Text dimColor>More: </Text>
        <Text color="ide">/remote mobile</Text>
        <Text dimColor> get the app · </Text>
        <Text color="ide">/remote native</Text>
        <Text dimColor> pick up another CLI's session · </Text>
        <Text color="ide">/remote session</Text>
        <Text dimColor> remote-mode URL</Text>
      </Box>
      <Box marginTop={1}>
        <Text dimColor>Messages, Stop and approvals from any of them reach this terminal at once. (press esc to close)</Text>
      </Box>
    </Pane>
  )
}

export const call: LocalJSXCommandCall = async (onDone, context, args) => {
  // `/remote session|desktop|mobile|native` (were /session, /desktop, /mobile, /native).
  const sub = remoteSubcommand(args)
  if (sub?.view === 'session') return (await import('../session/session')).call(onDone, context, sub.rest)
  if (sub?.view === 'mobile') return (await import('../mobile/mobile')).call(onDone)
  if (sub) {
    const { call: runSub } = sub.view === 'desktop' ? await import('../desktop/desktop') : await import('../native/native')
    const result = await runSub(sub.rest, context)
    if (result.type === 'text') onDone(result.value, { display: 'system' })
    else onDone(undefined, { display: 'skip' })
    return null
  }
  return <LiveRemote onDone={() => onDone()} />
}
