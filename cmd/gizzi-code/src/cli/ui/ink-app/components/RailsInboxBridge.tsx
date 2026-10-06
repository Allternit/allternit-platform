/**
 * Factory Inbox Bridge
 *
 * Bridges Factory peer messages into the REPL via the mailbox. The mailbox is
 * polled by useMailboxBridge in REPL.tsx, which calls handleIncomingPrompt and
 * starts a new turn with the Factory envelope body.
 *
 * This lives inside the MailboxProvider so it can call useMailbox. Peer
 * registration itself still happens in app.tsx before the TUI mounts.
 */

import { useEffect } from 'react'
import { useMailbox } from '../context/mailbox'
import {
  getRegisteredRailsPeer,
  startRailsInboxListener,
  stopRailsInboxListener,
} from '@/runtime/gizzi-core/services/railsPeer'
import { Log } from '@/shared/util/log'

export function RailsInboxBridge(): null {
  const mailbox = useMailbox()

  useEffect(() => {
    const peer = getRegisteredRailsPeer()
    if (!peer) return

    Log.Default.info('tui: Factory inbox bridge mounted', { peer_name: peer.name })

    startRailsInboxListener((cmd) => {
      Log.Default.info('tui: Factory message posted to mailbox', {
        peer_name: peer.name,
        value_preview: String(cmd.value ?? '').slice(0, 80),
      })
      mailbox.send({
        id: cmd.uuid,
        source: 'system',
        content: String(cmd.value ?? ''),
        // railsPeer stamps peerName on origin, which MessageOrigin does not declare
        from: (cmd.origin as { peerName?: string } | undefined)?.peerName,
        timestamp: new Date().toISOString(),
      })
    })

    return () => {
      stopRailsInboxListener()
    }
  }, [mailbox])

  return null
}
