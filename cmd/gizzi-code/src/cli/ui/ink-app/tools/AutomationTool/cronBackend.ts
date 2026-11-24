/**
 * The TUI's way to this computer's scheduler (Automation.CronBackend).
 *
 * The Ink TUI does not run the gizzi server, so its own CronService is never
 * started. The scheduler that runs tasks is the CronService inside the local
 * engine (`gizzi serve` or Desktop's runtime, runtime/server/server.ts), which
 * keeps its jobs in `<data>/gizzi-cron.db` and re-reads due jobs from that
 * database on every tick. So:
 *   - running: GET <engine>/cron/status answers with running: true
 *     (engine = GIZZI_ENGINE_URL, default 127.0.0.1:4096, the same engine the
 *     live terminal session attaches to);
 *   - create/list/get/update/delete: this process opens the same database
 *     through CronService (never start()ed here), the way bot-routines.ts
 *     does for the jobs daemon;
 *   - run: POST <engine>/cron/jobs/:id/run, so the run executes in the engine,
 *     where the executors live.
 * An engine that is not on this machine has another database, so it counts as
 * no scheduler here.
 */
import { join } from 'path'
import type { Automation } from '@/runtime/primitives/automation'
import type { CronRun } from '@/runtime/automation/cron/types'
import { engineURL } from '../../services/liveSession.js'

const PROBE_TIMEOUT_MS = 1_500

/** The database the engine's scheduler uses (server.ts). */
export async function engineCronDbPath(): Promise<string> {
  const { Global } = await import('@/runtime/context/global')
  return join(Global.Path.data, 'gizzi-cron.db')
}

function isLoopback(url: string): boolean {
  try {
    const host = new URL(url).hostname
    return host === '127.0.0.1' || host === 'localhost' || host === '::1' || host === '[::1]'
  } catch {
    return false
  }
}

/** True when the engine on this machine answers and its scheduler is running. */
export async function engineSchedulerRunning(base: string = engineURL()): Promise<boolean> {
  if (!isLoopback(base)) return false
  try {
    const res = await fetch(`${base}/cron/status`, { signal: AbortSignal.timeout(PROBE_TIMEOUT_MS) })
    if (!res.ok) return false
    const status = (await res.json()) as { running?: unknown }
    return status?.running === true
  } catch {
    return false
  }
}

let openedDbPath: string | null = null

/**
 * The backend, or undefined when the engine's scheduler isn't running (the
 * core then answers `no_scheduler` with the start command). `dbPath` is for
 * tests.
 */
export async function tuiCronBackend(input: { dbPath?: string } = {}): Promise<Automation.CronBackend | undefined> {
  const base = engineURL()
  if (!(await engineSchedulerRunning(base))) return undefined
  const { CronService } = await import('@/runtime/automation/cron/service')
  const dbPath = input.dbPath ?? (await engineCronDbPath())
  if (openedDbPath !== dbPath) {
    CronService.initialize({ dbPath })
    openedDbPath = dbPath
  }
  return {
    running: () => true,
    create: inputJob => CronService.create(inputJob),
    list: () => CronService.list(),
    get: id => CronService.get(id),
    update: (id, patch) => CronService.update(id, patch),
    delete: id => CronService.delete(id),
    async run(id): Promise<CronRun> {
      // The engine answers when the run ends; the core never waits on it.
      const res = await fetch(`${base}/cron/jobs/${encodeURIComponent(id)}/run`, { method: 'POST' })
      if (!res.ok) throw new Error(`The scheduler refused the run (HTTP ${res.status}).`)
      return (await res.json()) as CronRun
    },
  }
}
