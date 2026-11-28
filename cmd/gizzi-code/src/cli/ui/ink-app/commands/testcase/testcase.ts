// @ts-nocheck
import type { LocalCommandCall } from '../../../types/command.js'
import { getDirectConnectServerUrl, getSessionId } from '../../bootstrap/state.js'

export function parseTestcaseArgs(args: string): { label: 'good' | 'bad'; note?: string } {
  const text = args.trim()
  const m = /^(good|bad)\b\s*(.*)$/is.exec(text)
  if (m) return { label: m[1].toLowerCase() as 'good' | 'bad', note: m[2].trim() || undefined }
  return { label: 'bad', note: text || undefined }
}

export const call: LocalCommandCall = async (args) => {
  const { label, note } = parseTestcaseArgs(args)
  const base = getDirectConnectServerUrl() ?? 'http://localhost:4096'
  try {
    const res = await fetch(new URL('/v1/eval/cases', base), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'x-allternit-device-platform': 'tui' },
      body: JSON.stringify({ session_id: getSessionId(), label, ...(note ? { note } : {}) }),
    })
    const body: any = await res.json().catch(() => ({}))
    if (!res.ok) return { type: 'text', value: body.message ?? "Couldn't save, try again." }
    const lines = [`Saved test case ${body.case.id} (${body.case.suite})`]
    lines.push(body.self_test?.agrees_with_label ? 'Self-test: agrees' : 'Needs a check: the saved checks do not match your label')
    return { type: 'text', value: lines.join('\n') }
  } catch {
    return { type: 'text', value: "Couldn't reach your computer, try again." }
  }
}
