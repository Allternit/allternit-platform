import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { useEffect, useRef } from 'react'
import { desktopPetSettingsPath } from './desktopPet'

/**
 * Drives the Allternit Desktop pet from this terminal session through its
 * loopback hook port (surfaces/allternit-desktop/src/main/pet-hook-server.ts).
 * Desktop writes the port and token to `desktop-pet-hook.json` beside its
 * pet settings; without that file (Desktop not installed or never started)
 * nothing is sent. Every send is fire-and-forget with a 1 s cap, so a slow
 * or closed Desktop never holds up a turn. GIZZI_DESKTOP_PET_HOOK=0 turns it off.
 */
export type DesktopPetSignal = 'thinking' | 'tool' | 'tool-done' | 'waiting' | 'turn-done' | 'failed' | 'idle'

export function desktopPetHookPath(): string {
  return join(dirname(desktopPetSettingsPath()), 'desktop-pet-hook.json')
}

function readHook(): { url: string; token: string } | undefined {
  try {
    const saved = JSON.parse(readFileSync(desktopPetHookPath(), 'utf8')) as { url?: unknown; token?: unknown }
    if (typeof saved.url !== 'string' || typeof saved.token !== 'string') return undefined
    // Only ever loopback: the file is ours, but never send the token anywhere else.
    if (!/^http:\/\/(127\.0\.0\.1|localhost):\d+$/.test(saved.url)) return undefined
    return { url: saved.url, token: saved.token }
  } catch {
    return undefined
  }
}

function post(path: 'state' | 'bubble', body: Record<string, unknown>): void {
  if (process.env.GIZZI_DESKTOP_PET_HOOK === '0') return
  const hook = readHook()
  if (!hook) return
  void fetch(`${hook.url}/${path}`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${hook.token}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ ...body, client: 'gizzi' }),
    signal: AbortSignal.timeout(1000),
  }).catch(() => {})
}

export function reportDesktopPet(signal: DesktopPetSignal): void {
  post('state', { state: signal })
}

export function sayOnDesktopPet(text: string): void {
  post('bubble', { text })
}

/** What this session is doing, from the REPL's own state. */
export function desktopPetSignalFor(s: { isLoading: boolean; toolRunning: boolean; waiting: boolean }): DesktopPetSignal {
  if (s.waiting) return 'waiting'
  if (!s.isLoading) return 'idle'
  return s.toolRunning ? 'tool' : 'thinking'
}

/**
 * Report this session's activity to the Desktop pet on every change: thinking,
 * running a tool, waiting for a permission answer, and a wave when a turn ends.
 * Ongoing states are refreshed every minute (Desktop expires them after two),
 * and the session goes idle on exit.
 */
export function useDesktopPetHook(s: { isLoading: boolean; toolRunning: boolean; waiting: boolean }): void {
  const signal = desktopPetSignalFor(s)
  const last = useRef<DesktopPetSignal>('idle')
  useEffect(() => {
    const prev = last.current
    if (signal === prev) return
    last.current = signal
    if (signal === 'idle' && prev !== 'waiting') reportDesktopPet('turn-done')
    else reportDesktopPet(signal)
  }, [signal])
  useEffect(() => {
    if (signal === 'idle') return
    const timer = setInterval(() => reportDesktopPet(signal), 60_000)
    timer.unref?.()
    return () => clearInterval(timer)
  }, [signal])
  useEffect(() => () => { if (last.current !== 'idle') reportDesktopPet('idle') }, [])
}
