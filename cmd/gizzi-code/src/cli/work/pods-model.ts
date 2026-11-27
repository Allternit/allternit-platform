/**
 * Pods in the terminal: the same grouping the Office view draws, read from
 * allternit-api `GET /api/v1/floor/state` (`{ v: 2, mainBotId, pods, bots }`).
 * A pod is the Chief, a project or the Team; each lists its bots with their
 * state. Bots carry only an id on this route, so the view shows ids.
 */
export type PodBot = { id: string; name?: string; status?: string; lastText?: string; needs?: Record<string, number>; updatedAt?: number }
export type Pod = { id: string; name: string; color?: string; kind?: string; botIds?: string[] }
export type FloorState = { v?: number; mainBotId?: string | null; pods?: Pod[]; bots?: PodBot[] }

export const NO_PODS = "No pods yet. Pods group your bots by project once you have bots working."
export const PODS_SIGNED_OUT = "Sign in to see your pods: run gizzi login."

export function podError(e: unknown): string {
  const m = e instanceof Error ? e.message : String(e)
  return /\b40[13]\b/.test(m) ? PODS_SIGNED_OUT : m
}

export function botsOf(state: FloorState, pod: Pod): PodBot[] {
  const byId = new Map((state.bots ?? []).map(b => [b.id, b]))
  return (pod.botIds ?? []).map(id => byId.get(id) ?? { id })
}

export function needsCount(b: PodBot): number {
  return Object.values(b.needs ?? {}).reduce((a, n) => a + (n || 0), 0)
}

/** "3 bots · 1 working · 2 need you" */
export function podSummary(state: FloorState, pod: Pod): string {
  const bots = botsOf(state, pod)
  const working = bots.filter(b => b.status === 'working' || b.status === 'talking').length
  const needs = bots.reduce((a, b) => a + needsCount(b) + (b.status === 'waiting' && !needsCount(b) ? 1 : 0), 0)
  const parts = [`${bots.length} ${bots.length === 1 ? 'bot' : 'bots'}`]
  if (working) parts.push(`${working} working`)
  if (needs) parts.push(`${needs} ${needs === 1 ? 'needs' : 'need'} you`)
  return parts.join(' · ')
}

export function podRow(state: FloorState, pod: Pod): string {
  const kind = pod.kind === 'chief' ? 'chief' : pod.kind === 'team' ? 'team' : 'project'
  return `${pod.name.padEnd(24)} ${kind.padEnd(8)} ${podSummary(state, pod)}`
}

export function podDetail(state: FloorState, pod: Pod): string[] {
  const bots = botsOf(state, pod)
  if (!bots.length) return ['No bots in this pod.']
  return bots.map(b => {
    const n = needsCount(b)
    const mark = b.id === state.mainBotId ? ' (chief of staff)' : ''
    const tail = [b.status ?? 'idle', n ? `${n} need you` : '', b.lastText ? `"${b.lastText.slice(0, 60)}"` : ''].filter(Boolean).join(' · ')
    return `${b.name ?? b.id}${mark}  ${tail}`
  })
}
