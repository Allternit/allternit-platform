/**
 * Task Mirror Bridge
 *
 * When the TUI conversation is a runtime session (`-s ses_…`, bot canonical
 * chats), its background shells and agents are mirrored into the shared
 * background-task table under that session so the product surfaces' Tasks
 * pane lists them next to the daemon's own tasks. A Stop from a surface
 * settles the row as cancelled; this bridge notices and stops the task here,
 * where it actually runs.
 *
 * Plain TUI sessions (random UUIDs, no runtime row) are left alone.
 */

import { useEffect } from 'react'
import { useAppStateStore } from '../state/AppState'
import { getSessionId } from '../bootstrap/state'
import { getTaskOutput } from '../utils/task/diskOutput'
import { stopTask } from '../tasks/stopTask'
import { isTerminalTaskStatus, type TaskStateBase, type TaskStatus } from '../Task'
import { Log } from '@/shared/util/log'

const MIRRORED_TYPES: Record<string, string> = { local_bash: 'shell', local_agent: 'subagent' }
const SYNC_DEBOUNCE_MS = 500
const POLL_MS = 2_000
const OUTPUT_BYTES = 64_000

type MirrorStatus = 'queued' | 'running' | 'completed' | 'failed' | 'cancelled'

export function mirrorStatus(status: TaskStatus): MirrorStatus {
  switch (status) {
    case 'pending':
      return 'queued'
    case 'running':
      return 'running'
    case 'completed':
      return 'completed'
    case 'failed':
      return 'failed'
    case 'killed':
      return 'cancelled'
  }
}

// Must carry BackgroundTask.EXTERNAL_ID_PREFIX so daemon restarts leave the row to this process.
export function mirrorTaskId(taskId: string): string {
  return `tui_${taskId}`
}

export function TaskMirrorBridge(): null {
  const store = useAppStateStore()

  useEffect(() => {
    let disposed = false
    let syncTimer: ReturnType<typeof setTimeout> | null = null
    let syncing = false
    // mirror id -> last status written by this process
    const written = new Map<string, MirrorStatus>()
    const sessionChecks = new Map<string, boolean>()

    async function mirror() {
      const { BackgroundTaskMirror } = await import('@/runtime/session/background-task-mirror')
      return BackgroundTaskMirror
    }

    async function runtimeSession(): Promise<string | null> {
      const sessionId = String(getSessionId())
      if (!sessionId.startsWith('ses_')) return null
      if (!sessionChecks.has(sessionId)) {
        sessionChecks.set(sessionId, (await mirror()).sessionExists(sessionId))
      }
      return sessionChecks.get(sessionId) ? sessionId : null
    }

    function mirroredTasks(): TaskStateBase[] {
      return (Object.values(store.getState().tasks ?? {}) as TaskStateBase[]).filter(
        (task) => !!task && task.type in MIRRORED_TYPES,
      )
    }

    async function sync(includeLiveOutput: boolean) {
      if (syncing || disposed) return
      syncing = true
      try {
        const sessionId = await runtimeSession()
        if (!sessionId) return
        const Mirror = await mirror()
        for (const task of mirroredTasks()) {
          const id = mirrorTaskId(task.id)
          const status = mirrorStatus(task.status)
          if (!written.has(id)) {
            Mirror.start({
              id,
              parentSessionID: sessionId,
              kind: MIRRORED_TYPES[task.type],
              description: task.description,
              status,
              created: task.startTime,
            })
          }
          const terminal = isTerminalTaskStatus(task.status)
          // Unseen ids always write once, so the row gets its output.
          const changed = written.get(id) !== status
          if (!changed && !(includeLiveOutput && !terminal && task.type === 'local_bash')) continue
          const output = await getTaskOutput(task.id, OUTPUT_BYTES)
          Mirror.update(id, {
            status,
            output,
            ...(terminal ? { finished: task.endTime ?? Date.now() } : {}),
            ...(task.status === 'failed' ? { error: (task as { error?: string }).error ?? 'Failed' } : {}),
          })
          written.set(id, status)
        }
      } catch (error) {
        Log.Default.warn('tui: task mirror sync failed', { error: String(error) })
      } finally {
        syncing = false
      }
    }

    // A surface's Stop settles the shared row as cancelled; stop it here too.
    async function honorCancels() {
      const running = mirroredTasks().filter((task) => task.status === 'running')
      if (running.length === 0 || !(await runtimeSession())) return
      const statuses = (await mirror()).statuses(running.map((task) => mirrorTaskId(task.id)))
      for (const task of running) {
        if (statuses[mirrorTaskId(task.id)] !== 'cancelled') continue
        Log.Default.info('tui: stopping task cancelled from a surface', { taskId: task.id })
        await stopTask(task.id, { getAppState: store.getState, setAppState: store.setState }).catch(() => undefined)
      }
    }

    const unsubscribe = store.subscribe(() => {
      if (syncTimer) return
      syncTimer = setTimeout(() => {
        syncTimer = null
        void sync(false)
      }, SYNC_DEBOUNCE_MS)
    })
    const poll = setInterval(() => {
      void sync(true)
      void honorCancels().catch((error) => Log.Default.warn('tui: task cancel check failed', { error: String(error) }))
    }, POLL_MS)
    void sync(false)

    return () => {
      disposed = true
      unsubscribe()
      clearInterval(poll)
      if (syncTimer) clearTimeout(syncTimer)
    }
  }, [store])

  return null
}
