import { spawn } from 'node:child_process'
import { cloudInferenceToken, cloudOrigin } from '@/runtime/providers/discovery/allternit-cloud.js'
import { getSessionId } from '../../bootstrap/state.js'
import type { LocalCommandCall } from '../../types/command.js'

export type ShareTurn = { role: 'user' | 'assistant'; text: string; tools?: string[] }

type Block = { type?: string; text?: unknown; name?: unknown }

/**
 * Privacy boundary of a shared link: user/assistant text and tool NAMES only.
 * Tool inputs/outputs, reasoning, images and meta/system messages never leave.
 */
export function buildShareSnapshot(messages: readonly unknown[]): ShareTurn[] {
  const out: ShareTurn[] = []
  for (const raw of messages) {
    const m = raw as { type?: string; isMeta?: boolean; isVirtual?: boolean; message?: { content?: unknown } }
    if (!m || (m.type !== 'user' && m.type !== 'assistant') || m.isMeta || m.isVirtual) continue
    const content = m.message?.content
    const blocks: Block[] = typeof content === 'string' ? [{ type: 'text', text: content }] : Array.isArray(content) ? content : []
    const text = blocks
      .filter(b => b?.type === 'text' && typeof b.text === 'string')
      .map(b => (b.text as string).trim())
      .filter(Boolean)
      .join('\n\n')
    const tools =
      m.type === 'assistant'
        ? blocks.filter(b => b?.type === 'tool_use' && typeof b.name === 'string').map(b => b.name as string)
        : []
    if (!text && tools.length === 0) continue
    out.push({ role: m.type, text, ...(tools.length ? { tools } : {}) })
  }
  return out
}

function titleOf(snapshot: ShareTurn[]): string {
  const first = snapshot.find(t => t.role === 'user' && t.text)?.text.replace(/\s+/g, ' ') ?? 'Conversation'
  return first.length > 80 ? `${first.slice(0, 79)}…` : first
}

function copy(text: string): Promise<boolean> {
  const cmd = process.platform === 'darwin' ? ['pbcopy'] : process.platform === 'win32' ? ['clip'] : ['xclip', '-selection', 'clipboard']
  return new Promise(resolve => {
    try {
      const p = spawn(cmd[0]!, cmd.slice(1), { stdio: ['pipe', 'ignore', 'ignore'] })
      p.on('error', () => resolve(false))
      p.on('close', code => resolve(code === 0))
      p.stdin.on('error', () => resolve(false))
      p.stdin.end(text)
    } catch {
      resolve(false)
    }
  })
}

const SIGN_IN = 'Sign in to Allternit Cloud to share (run `gizzi login`), then run /share again.'

function failure(status: number): string {
  if (status === 401 || status === 403) return 'Allternit Cloud rejected your sign-in. Run `gizzi login` and try again.'
  if (status === 413) return 'This conversation is too large to share.'
  return `Could not reach Allternit Cloud (HTTP ${status}). Try again in a moment.`
}

export const call: LocalCommandCall = async (args, context) => {
  const off = args.trim().toLowerCase() === 'off'
  const token = await cloudInferenceToken().catch(() => undefined)
  if (!token) return { type: 'text', value: SIGN_IN }
  const base = `${cloudOrigin()}/api/v2/thread-shares`
  const headers = { Authorization: `Bearer ${token}`, Accept: 'application/json' }
  const sessionId = String(getSessionId())
  try {
    if (off) {
      const found = await fetch(`${base}?sessionId=${encodeURIComponent(sessionId)}`, { headers, signal: AbortSignal.timeout(15000) })
      if (found.status === 404) return { type: 'text', value: 'This conversation has no shared link.' }
      if (!found.ok) return { type: 'text', value: failure(found.status) }
      const { token: shareToken } = (await found.json()) as { token: string }
      const del = await fetch(`${base}/${encodeURIComponent(shareToken)}`, { method: 'DELETE', headers, signal: AbortSignal.timeout(15000) })
      if (!del.ok && del.status !== 404) return { type: 'text', value: failure(del.status) }
      return { type: 'text', value: 'Link turned off' }
    }
    const snapshot = buildShareSnapshot(context.messages)
    if (snapshot.length === 0) return { type: 'text', value: 'Nothing to share yet.' }
    const res = await fetch(base, {
      method: 'POST',
      headers: { ...headers, 'Content-Type': 'application/json' },
      body: JSON.stringify({ sessionId, title: titleOf(snapshot), snapshot }),
      signal: AbortSignal.timeout(20000),
    })
    if (!res.ok) return { type: 'text', value: failure(res.status) }
    const { url } = (await res.json()) as { url: string }
    const copied = await copy(url)
    return { type: 'text', value: `${url}${copied ? ' (copied)' : ''}\nShared: text and tool names only. /share off turns the link off.` }
  } catch {
    return { type: 'text', value: 'Could not reach Allternit Cloud. Check your connection and try again.' }
  }
}
