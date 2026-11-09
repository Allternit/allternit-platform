import { toString as qrToString } from 'qrcode'
import * as React from 'react'
import { useEffect, useState } from 'react'
import { Pane } from '../../components/design-system/Pane'
import { Box, Text } from '../../ink'
import { useKeybinding } from '../../keybindings/useKeybinding'
import { getLiveSession, liveLinks, onLiveSessionChange, type LiveLinks } from '../../services/liveSession'
import type { LocalJSXCommandCall } from '../../types/command'

function useLive() {
  const [live, setLive] = useState(() => ({ ...getLiveSession() }))
  useEffect(() => onLiveSessionChange(() => setLive({ ...getLiveSession() })), [])
  return live
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
      <Box marginTop={1}>
        <Text dimColor>Messages, Stop and approvals from any of them reach this terminal at once. (press esc to close)</Text>
      </Box>
    </Pane>
  )
}

export const call: LocalJSXCommandCall = async onDone => {
  return <LiveRemote onDone={() => onDone()} />
}
