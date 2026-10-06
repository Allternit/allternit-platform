#!/usr/bin/env bun
/**
 * End-to-end smoke test for the Allternit Factory peer registry + cross-session messaging.
 *
 * Registers two peers on a local allternit-api, starts UDS inbox listeners,
 * lists peers, sends a message from peer-a to peer-b by name, and asserts
 * delivery.
 */

import { createServer } from 'net'
import { mkdir, rm } from 'fs/promises'

const API_URL = process.env.ALLTERNIT_API_URL || 'http://127.0.0.1:8015'
const USER_ID = process.env.ALLTERNIT_USER_ID || 'e2e-test-user'
const DATA_DIR =
  process.env.ALLTERNIT_DATA_DIR ||
  '~/Desktop/allternit-workspace/allternit-test'

interface PeerRegisterResponse {
  peer_id: string
  name: string
  inbox_socket: string
}

interface PeerListResponse {
  peers: Array<{
    peer_id: string
    name: string
    cwd: string
    vendor: string
    inbox_socket: string
    status: string
    registered_at: string
    last_heartbeat_at: string
  }>
}

interface SendResponse {
  delivered: boolean
  error?: string
}

async function apiFetch(path: string, init: RequestInit = {}): Promise<Response> {
  return fetch(`${API_URL}${path}`, {
    ...init,
    headers: {
      'Content-Type': 'application/json',
      'x-allternit-user-id': USER_ID,
      ...(init.headers || {}),
    },
  })
}

async function registerPeer(name: string): Promise<PeerRegisterResponse> {
  const res = await apiFetch('/api/factory/peers', {
    method: 'POST',
    body: JSON.stringify({ name, vendor: 'e2e', cwd: process.cwd() }),
  })
  if (!res.ok) {
    throw new Error(`registerPeer failed: ${res.status} ${await res.text()}`)
  }
  return (await res.json()) as PeerRegisterResponse
}

async function listPeers(): Promise<PeerListResponse> {
  const res = await apiFetch('/api/factory/peers')
  if (!res.ok) {
    throw new Error(`listPeers failed: ${res.status} ${await res.text()}`)
  }
  return (await res.json()) as PeerListResponse
}

async function sendToPeer(name: string, body: string, from?: string): Promise<SendResponse> {
  const res = await apiFetch(`/api/factory/peers/${encodeURIComponent(name)}/send`, {
    method: 'POST',
    body: JSON.stringify({ body, ...(from ? { from } : {}) }),
  })
  if (!res.ok) {
    throw new Error(`sendToPeer failed: ${res.status} ${await res.text()}`)
  }
  return (await res.json()) as SendResponse
}

function startInboxServer(socketPath: string): Promise<string[]> {
  return new Promise((resolve, reject) => {
    const messages: string[] = []
    const server = createServer((socket) => {
      let buffer = ''
      socket.on('data', (chunk) => {
        buffer += chunk.toString('utf-8')
        let idx: number
        while ((idx = buffer.indexOf('\n')) !== -1) {
          const line = buffer.slice(0, idx).trim()
          buffer = buffer.slice(idx + 1)
          if (line) messages.push(line)
        }
      })
    })
    server.on('error', reject)
    server.listen(socketPath, () => {
      resolve(messages)
    })
  })
}

async function cleanupSocket(path: string): Promise<void> {
  try {
    await rm(path)
  } catch {
    // ignore
  }
}

async function main() {
  console.log(`API: ${API_URL}`)
  console.log(`Data dir: ${DATA_DIR}`)

  // Ensure peers data dir exists.
  await mkdir(`${DATA_DIR}/.allternit/peers/inbox`, { recursive: true }).catch(() => {})

  // Register peers.
  const peerA = await registerPeer('e2e-peer-a')
  const peerB = await registerPeer('e2e-peer-b')
  console.log(`Registered peer-a: ${peerA.peer_id}`)
  console.log(`Registered peer-b: ${peerB.peer_id}`)

  // Start UDS inbox listeners.
  await cleanupSocket(peerA.inbox_socket)
  await cleanupSocket(peerB.inbox_socket)
  const inboxA = await startInboxServer(peerA.inbox_socket)
  const inboxB = await startInboxServer(peerB.inbox_socket)

  // Heartbeat to mark active.
  await apiFetch(`/api/factory/peers/${encodeURIComponent(peerA.name)}/heartbeat`, {
    method: 'POST',
  })
  await apiFetch(`/api/factory/peers/${encodeURIComponent(peerB.name)}/heartbeat`, {
    method: 'POST',
  })

  // List peers.
  const listed = await listPeers()
  const foundA = listed.peers.find((p) => p.name === peerA.name)
  const foundB = listed.peers.find((p) => p.name === peerB.name)
  if (!foundA || !foundB) {
    throw new Error('ListPeers did not return registered peers')
  }
  console.log(`Listed ${listed.peers.length} peer(s)`)
  console.log(`  peer-a status: ${foundA.status}`)
  console.log(`  peer-b status: ${foundB.status}`)

  // Send message from peer-a to peer-b by name.
  const sendResult = await sendToPeer(peerB.name, 'hello from peer-a', peerA.name)
  if (!sendResult.delivered) {
    throw new Error(`Send failed: ${sendResult.error}`)
  }
  console.log(`Message delivered from ${peerA.name} to ${peerB.name}`)

  // Allow socket write to flush.
  await new Promise((r) => setTimeout(r, 200))

  if (inboxB.length === 0) {
    throw new Error('peer-b inbox did not receive the message')
  }
  const envelope = JSON.parse(inboxB[0])
  console.log(`peer-b received: ${envelope.body}`)
  if (envelope.from !== peerA.name) {
    throw new Error(`Expected from ${peerA.name}, got ${envelope.from}`)
  }
  if (envelope.body !== 'hello from peer-a') {
    throw new Error(`Expected body "hello from peer-a", got ${envelope.body}`)
  }

  console.log('\n✅ Factory peer e2e test passed')
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
