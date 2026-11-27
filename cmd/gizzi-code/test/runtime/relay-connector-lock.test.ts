import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { existsSync, mkdirSync, mkdtempSync, rmSync, utimesSync, writeFileSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import { RelayConnector } from "../../src/runtime/integrations/relay-connector"

// One cloud relay per machine: gizzi takes ~/.config/allternit/relay.lock only
// when no live process holds it, and yields to Desktop.
describe("RelayConnector relay.lock", () => {
  let dir: string
  let file: string

  beforeEach(() => {
    dir = mkdtempSync(path.join(os.tmpdir(), "relay-lock-"))
    file = path.join(dir, "allternit", "relay.lock")
  })
  afterEach(() => rmSync(dir, { recursive: true, force: true }))

  function holdWith(pid: number, owner: "gizzi" | "desktop") {
    mkdirSync(path.dirname(file), { recursive: true })
    writeFileSync(file, JSON.stringify({ pid, owner, startedAt: new Date().toISOString() }))
  }

  function deadPid(): number {
    const child = Bun.spawnSync(["true"])
    expect(RelayConnector.pidAlive(child.pid)).toBe(false)
    return child.pid
  }

  test("takes a free lock and writes this pid, owner and start time", () => {
    expect(RelayConnector.acquireLock("gizzi", file)).toBe(true)
    const held = RelayConnector.readLock(file)
    expect(held?.pid).toBe(process.pid)
    expect(held?.owner).toBe("gizzi")
    expect(Number.isNaN(Date.parse(held!.startedAt))).toBe(false)
  })

  test("a live process holding the lock blocks it", () => {
    holdWith(process.ppid, "desktop")
    expect(RelayConnector.acquireLock("gizzi", file)).toBe(false)
    expect(RelayConnector.liveHolder(file)?.owner).toBe("desktop")
    expect(RelayConnector.readLock(file)?.pid).toBe(process.ppid)
  })

  test("a lock left by a dead pid is taken", () => {
    const stale = deadPid()
    holdWith(stale, "gizzi")
    expect(RelayConnector.liveHolder(file)).toBeUndefined()
    expect(RelayConnector.acquireLock("gizzi", file)).toBe(true)
    expect(RelayConnector.readLock(file)?.pid).toBe(process.pid)
  })

  test("an old lock that never parsed is taken", () => {
    mkdirSync(path.dirname(file), { recursive: true })
    writeFileSync(file, "not json")
    const old = new Date(Date.now() - 60_000)
    utimesSync(file, old, old)
    expect(RelayConnector.acquireLock("gizzi", file)).toBe(true)
    expect(RelayConnector.readLock(file)?.pid).toBe(process.pid)
  })

  test("release removes this process's lock and leaves another holder's alone", () => {
    expect(RelayConnector.acquireLock("gizzi", file)).toBe(true)
    RelayConnector.releaseLock(file)
    expect(existsSync(file)).toBe(false)

    holdWith(process.ppid, "desktop")
    RelayConnector.releaseLock(file)
    expect(RelayConnector.readLock(file)?.pid).toBe(process.ppid)
  })
})
