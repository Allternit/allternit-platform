/**
 * Serializes work on one checkout: an in-process promise chain plus a
 * cross-process lock directory (`mkdir` is atomic), so two gizzi sessions on
 * the same machine never interleave git operations on the same working tree.
 * A lock left by a dead process, or older than STALE_MS, is broken.
 */
import { mkdir, readFile, rm, writeFile } from "fs/promises"
import path from "path"

const chains = new Map<string, Promise<unknown>>()
const STALE_MS = 120_000
const WAIT_MS = 60_000

export class DriveBusyError extends Error {
  constructor() {
    super("Another gizzi session is still saving to this memory drive. Try again in a moment.")
    this.name = "DriveBusyError"
  }
}

function alive(pid: number): boolean {
  if (!Number.isInteger(pid) || pid <= 0) return false
  try {
    process.kill(pid, 0)
    return true
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "EPERM"
  }
}

async function acquire(lockDir: string): Promise<void> {
  await mkdir(path.dirname(lockDir), { recursive: true })
  const started = Date.now()
  let delay = 20
  for (;;) {
    try {
      await mkdir(lockDir)
      await writeFile(path.join(lockDir, "owner"), JSON.stringify({ pid: process.pid, at: Date.now() }))
      return
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error
    }
    const owner = await readFile(path.join(lockDir, "owner"), "utf8")
      .then((t) => JSON.parse(t) as { pid: number; at: number })
      .catch(() => undefined)
    const stale = owner ? Date.now() - owner.at > STALE_MS || !alive(owner.pid) : false
    if (stale) {
      await rm(lockDir, { recursive: true, force: true })
      continue
    }
    if (!owner) {
      // Owner file not written yet (or torn): give the creator a moment, then treat as stale.
      const stat = await import("fs/promises").then((fs) => fs.stat(lockDir)).catch(() => undefined)
      if (stat && Date.now() - stat.mtimeMs > 5_000) {
        await rm(lockDir, { recursive: true, force: true })
        continue
      }
    }
    if (Date.now() - started > WAIT_MS) throw new DriveBusyError()
    await new Promise((r) => setTimeout(r, delay))
    delay = Math.min(delay * 2, 250)
  }
}

/** Run `fn` while holding the lock for `key` (an absolute checkout path). */
export async function withDriveLock<T>(key: string, fn: () => Promise<T>): Promise<T> {
  const previous = chains.get(key) ?? Promise.resolve()
  const run = previous.catch(() => {}).then(async () => {
    const lockDir = `${key}.lock`
    await acquire(lockDir)
    try {
      return await fn()
    } finally {
      await rm(lockDir, { recursive: true, force: true }).catch(() => {})
    }
  })
  chains.set(key, run)
  try {
    return await run
  } finally {
    if (chains.get(key) === run) chains.delete(key)
  }
}
