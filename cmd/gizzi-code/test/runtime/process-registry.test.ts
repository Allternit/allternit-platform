import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { spawn } from "node:child_process"
import { ProcessRegistry } from "../../src/runtime/process-registry"

function alive(pid: number | undefined): boolean {
  if (!pid) return false
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

// Children tracked by earlier test files would skew size().
beforeEach(() => {
  ProcessRegistry.killAll()
  ProcessRegistry.resetForTests()
})

afterEach(() => {
  ProcessRegistry.killAll()
  ProcessRegistry.resetForTests()
})

describe("ProcessRegistry", () => {
  test("kills a tracked detached child on killAll", async () => {
    const child = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], {
      detached: process.platform !== "win32",
      stdio: "ignore",
    })
    expect(child.pid).toBeTruthy()
    ProcessRegistry.track(child, { label: "test-loop", group: process.platform !== "win32" })
    expect(ProcessRegistry.size()).toBe(1)
    expect(alive(child.pid)).toBe(true)

    ProcessRegistry.killAll()
    const deadline = Date.now() + 3000
    while (Date.now() < deadline && alive(child.pid)) {
      await Bun.sleep(50)
    }
    expect(alive(child.pid)).toBe(false)
    expect(ProcessRegistry.size()).toBe(0)
  })

  test("untracks a child that exits on its own", async () => {
    const child = spawn(process.execPath, ["-e", "process.exit(0)"], {
      stdio: "ignore",
    })
    ProcessRegistry.track(child, { label: "test-exit" })
    const deadline = Date.now() + 3000
    while (Date.now() < deadline && ProcessRegistry.size() > 0) {
      await Bun.sleep(20)
    }
    expect(ProcessRegistry.size()).toBe(0)
  })
})
