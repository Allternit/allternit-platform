/**
 * Session-scoped child process registry.
 *
 * Gizzi used to spawn sidecar/CLI/shell children with `detached: true` +
 * `unref()`, which lets them outlive the parent after Ctrl+C, SIGHUP, or a
 * desktop close. Track every session child here and kill the tree on
 * shutdown. Intentional daemons (`gizzi workflows wake jobs --background`) must not be
 * registered.
 */

import { spawnSync, type ChildProcess } from "node:child_process"
import { registerCleanup } from "@/shared/utils/cleanupRegistry"

export type Trackable = {
  pid?: number
  kill?: (signal?: NodeJS.Signals | number) => unknown
  on?: (event: string, listener: (...args: unknown[]) => void) => unknown
  once?: (event: string, listener: (...args: unknown[]) => void) => unknown
  exited?: Promise<unknown>
}

export type TrackOptions = {
  label: string
  /** Unix: child is a process-group leader (`detached: true`). Kill `-pid`. */
  group?: boolean
}

type Entry = {
  pid: number
  label: string
  group: boolean
  child?: Trackable
}

const tracked = new Map<number, Entry>()
let installed = false
let shuttingDown = false

function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

function killPid(pid: number, signal: NodeJS.Signals, group: boolean): void {
  try {
    if (process.platform === "win32") {
      spawnSync("taskkill", ["/pid", String(pid), "/t", "/f"], {
        stdio: "ignore",
        windowsHide: true,
        timeout: 5000,
      })
      return
    }
    if (group) {
      try {
        process.kill(-pid, signal)
        return
      } catch {
        // Fall through to the leader pid.
      }
    }
    process.kill(pid, signal)
  } catch {
    // Already gone.
  }
}

export function killProcessTree(pid: number, group = false): void {
  if (!pid || !Number.isFinite(pid)) return
  if (process.platform === "win32") {
    killPid(pid, "SIGKILL", false)
    return
  }
  killPid(pid, "SIGTERM", group)
  killPid(pid, "SIGKILL", group)
}

export const ProcessRegistry = {
  size(): number {
    return tracked.size
  },

  track(child: Trackable | ChildProcess, opts: TrackOptions): () => void {
    const pid = child.pid
    if (!pid) return () => {}

    const entry: Entry = {
      pid,
      label: opts.label,
      group: opts.group === true && process.platform !== "win32",
      child,
    }
    tracked.set(pid, entry)

    const untrack = () => {
      tracked.delete(pid)
    }

    const anyChild = child as Trackable
    if (typeof anyChild.once === "function") {
      anyChild.once("exit", untrack)
      anyChild.once("close", untrack)
    } else if (typeof anyChild.on === "function") {
      anyChild.on("exit", untrack)
      anyChild.on("close", untrack)
    }
    if (anyChild.exited) {
      void Promise.resolve(anyChild.exited).then(untrack, untrack)
    }

    return untrack
  },

  killAll(): void {
    shuttingDown = true
    const entries = [...tracked.values()]
    tracked.clear()
    for (const entry of entries) {
      try {
        entry.child?.kill?.("SIGTERM")
      } catch {
        // Ignore — fall through to pid kill.
      }
      killProcessTree(entry.pid, entry.group)
    }
  },

  install(): void {
    if (installed) return
    installed = true

    // Last-resort reap. SIGINT/SIGTERM are owned by serve/TUI, which call
    // killAll() themselves. Prepending signal handlers here would steal
    // bun's test runner SIGINT.
    process.on("exit", () => {
      this.killAll()
    })

    registerCleanup(async () => {
      this.killAll()
    })
  },

  /** Test helper — drop tracked pids without killing. */
  resetForTests(): void {
    tracked.clear()
    shuttingDown = false
  },

  get shuttingDown(): boolean {
    return shuttingDown
  },
}
