/**
 * Allternit Factory peer registration + inbox poller for gizzi-code.
 *
 * When a session starts, it registers itself as a local Factory peer so other
 * agents on this machine can discover and message it. Instead of a UDS push
 * listener (which is brittle across Bun/Node versions and blocks the TUI queue
 * timing), we poll the allternit-api inbox endpoint for pending envelopes and
 * inject them into the command queue as task notifications.
 */

import { writeFileSync, mkdirSync } from 'node:fs'
import { dirname } from 'node:path'
import { logForDiagnosticsNoPII } from 'src/shared/utils/diagLogs.js'
import { getCwd } from 'src/shared/utils/cwd.js'
import { errorMessage } from 'src/shared/utils/errors.js'
import { generateRequestId } from 'src/shared/utils/agentId.js'
import { enqueuePendingNotification } from 'src/shared/utils/messageQueueManager.js'
import { isFactoryPeerDisabled } from 'src/shared/utils/envUtils.js'
import type { QueuedCommand } from '@/types/textInputTypes.js'
import { Log } from 'src/shared/util/log.js'
import {
  getAllternitApiConfig,
  registerApiPeer,
  pollApiPeerInbox,
  type ApiPeerRegisterResponse,
} from '../../services/api/allternitApi.js'

let registeredPeer: ApiPeerRegisterResponse | null = null
let pollIntervalId: ReturnType<typeof setInterval> | null = null
let seenMessageIds = new Set<number>()

const RAILS_HEARTBEAT_TIMEOUT_MS = 10_000

export type RailsInboxMessageHandler = (command: QueuedCommand) => void

function formatIncomingMessage(envelope: {
  from?: string
  body?: string
}): string {
  const from = envelope.from ?? 'unknown'
  const body = envelope.body ?? ''
  return `<cross-session-message from="${from}">\n${body}\n</cross-session-message>`
}

export async function registerRailsPeer(
  sessionId: string,
): Promise<ApiPeerRegisterResponse | null> {
  // Gate is evaluated at runtime so the bundler cannot tree-shake the module.
  // Factory peer registration is default-on; set ALLTERNIT_FACTORY_PEER=0 to opt out.
  if (isFactoryPeerDisabled()) {
    return null
  }
  try {
    const config = getAllternitApiConfig()
    const cwd = getCwd()
    // Sanitize session id into a peer name: keep alphanumerics, dashes, underscores.
    const name = `gizzi-${sessionId.replace(/[^a-zA-Z0-9_-]/g, '-').slice(0, 64)}`
    const peer = await registerApiPeer(config, {
      name,
      vendor: 'gizzi',
      cwd,
    })
    registeredPeer = peer
    process.env.ALLTERNIT_FACTORY_PEER_NAME = peer.name
    process.env.ALLTERNIT_FACTORY_INBOX = peer.inbox_socket
    // The Factory peer registry marks peers dead if the inbox socket path does not
    // exist. HTTP-polling peers do not bind a real UDS server, so create a
    // placeholder file to keep the peer status active.
    try {
      mkdirSync(dirname(peer.inbox_socket), { recursive: true })
      writeFileSync(peer.inbox_socket, '')
    } catch (error) {
      logForDiagnosticsNoPII('info', 'rails_inbox_placeholder_failed', {
        error: errorMessage(error),
      })
    }
    logForDiagnosticsNoPII('info', 'rails_peer_registered', {
      peer_name: peer.name,
      inbox_socket: peer.inbox_socket,
    })
    return peer
  } catch (error) {
    logForDiagnosticsNoPII('info', 'rails_peer_registration_failed', {
      error: errorMessage(error),
    })
    return null
  }
}

export function getRegisteredRailsPeer(): ApiPeerRegisterResponse | null {
  return registeredPeer
}

export function startRailsInboxListener(
  onMessage?: RailsInboxMessageHandler,
): void {
  if (!registeredPeer) return

  const peerName = registeredPeer.name
  const config = getAllternitApiConfig()

  function dispatch(envelope: { from?: string; body?: string; message_id?: number }): void {
    Log.Default.info('rails_inbox_envelope_received', {
      from: envelope.from,
      message_id: envelope.message_id,
      body_preview: String(envelope.body ?? '').slice(0, 80),
    })
    const command: QueuedCommand = {
      value: formatIncomingMessage(envelope),
      mode: 'task-notification',
      priority: 'later',
      // generateRequestId returns "<type>-<ts>@<agent>" (not UUID-shaped); the
      // runtime only uses uuid as an opaque id string.
      uuid: generateRequestId('rails-inbox', String(envelope.message_id ?? 'unknown')) as QueuedCommand['uuid'],
      skipSlashCommands: true,
      isMeta: false,
      // MessageOrigin has no peerName field; the bridge reads it back structurally
      origin: { kind: 'rails_peer', peerName: String(envelope.from ?? 'unknown') } as QueuedCommand['origin'],
    }
    if (onMessage) {
      onMessage(command)
    } else {
      enqueuePendingNotification(command)
    }
  }

  async function pollOnce(): Promise<void> {
    try {
      const inbox = await pollApiPeerInbox(config, peerName, 20)
      for (const msg of inbox.messages) {
        if (seenMessageIds.has(msg.id)) continue
        seenMessageIds.add(msg.id)
        const body =
          typeof msg.payload === 'object' && msg.payload !== null
            ? String((msg.payload as Record<string, unknown>).body ?? '')
            : ''
        const from =
          typeof msg.payload === 'object' && msg.payload !== null
            ? String((msg.payload as Record<string, unknown>).from ?? msg.from)
            : msg.from
        dispatch({ from, body, message_id: msg.id })
      }
    } catch (error) {
      logForDiagnosticsNoPII('info', 'rails_inbox_poll_error', {
        error: errorMessage(error),
      })
    }
  }

  // Poll immediately, then every 2 seconds.
  pollOnce().catch(() => {
    // Next interval retries; inbox polling must never crash the host process.
  })
  pollIntervalId = setInterval(() => {
    pollOnce().catch(() => {
    // Next interval retries; inbox polling must never crash the host process.
  })
  }, 2_000)

  // Best-effort heartbeat while the session runs.
  const heartbeatInterval = setInterval(() => {
    if (!registeredPeer) {
      clearInterval(heartbeatInterval)
      return
    }
    heartbeatRailsPeer(registeredPeer.name).catch(() => {
      // Best-effort liveness signal; the next interval retries.
    })
  }, 30_000)

  // Stop polling and heartbeat when the process exits.
  process.once('exit', () => {
    clearInterval(heartbeatInterval)
    stopRailsInboxListener()
  })

  logForDiagnosticsNoPII('info', 'rails_inbox_polling_started', { peer_name: peerName })
}

export function stopRailsInboxListener(): void {
  if (pollIntervalId) {
    clearInterval(pollIntervalId)
    pollIntervalId = null
  }
}

async function heartbeatRailsPeer(name: string): Promise<void> {
  try {
    const config = getAllternitApiConfig()
    const res = await fetch(
      `${config.baseUrl}/api/factory/peers/${encodeURIComponent(name)}/heartbeat`,
      {
        method: 'POST',
        // Best-effort liveness ping: never let a hung gateway hold the
        // heartbeat (and its interval slot) open indefinitely.
        signal: AbortSignal.timeout(RAILS_HEARTBEAT_TIMEOUT_MS),
        headers: {
          'Content-Type': 'application/json',
          'x-allternit-user-id': config.userId,
          // Only send credentials when a real token exists — never fall
          // back to a hardcoded dev token.
          ...(config.token
            ? { Authorization: `Bearer ${config.token}` }
            : {}),
        },
      },
    )
    if (!res.ok) {
      throw new Error(`heartbeat failed: ${res.status}`)
    }
  } catch (error) {
    logForDiagnosticsNoPII('info', 'rails_peer_heartbeat_failed', {
      error: errorMessage(error),
    })
  }
}
