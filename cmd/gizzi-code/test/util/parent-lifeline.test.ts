import { describe, expect, test } from "bun:test"
import { spawn } from "node:child_process"
import { mkdtempSync, readFileSync, existsSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { spawnOwnedChild } from "../../src/runtime/util/parent-lifeline"

const LIFELINE = join(import.meta.dir, "../../src/runtime/util/parent-lifeline.ts")

const alive = (pid: number) => {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

async function until(check: () => boolean, timeoutMs = 3000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    if (check()) return true
    await Bun.sleep(50)
  }
  return check()
}

describe.skipIf(process.platform === "win32")("parent lifeline", () => {
  test("owned child keeps its real pid and exit signal", async () => {
    const child = spawnOwnedChild("sleep", ["30"], { stdio: ["ignore", "pipe", "pipe"] })
    const exited = new Promise<[number | null, string | null]>((resolve) =>
      child.once("exit", (code, signal) => resolve([code, signal])),
    )
    await Bun.sleep(100)
    child.kill("SIGTERM")
    expect(await exited).toEqual([null, "SIGTERM"])
  })

  for (const stdin of ["ignore", "pipe"] as const) {
  test(`owned child and its grandchildren die when gizzi is SIGKILLed (stdin ${stdin})`, async () => {
    const dir = mkdtempSync(join(tmpdir(), "gizzi-lifeline-"))
    const gcFile = join(dir, "gc")
    const parent = spawn(
      process.execPath,
      [
        "-e",
        `const { spawnOwnedChild } = await import(${JSON.stringify(LIFELINE)});
         spawnOwnedChild("bash", ["-c", "sleep 30 & echo $! > ${gcFile}; wait"], { stdio: [${JSON.stringify(stdin)}, "ignore", "ignore"] });
         setInterval(() => {}, 1000)`,
      ],
      { stdio: "ignore" },
    )
    expect(await until(() => existsSync(gcFile) && readFileSync(gcFile, "utf8").trim() !== "", 5000)).toBe(true)
    const grandchild = Number(readFileSync(gcFile, "utf8"))
    expect(alive(grandchild)).toBe(true)

    parent.kill("SIGKILL")

    expect(await until(() => !alive(grandchild))).toBe(true)
  })
  }

  for (const [env, survives] of [["stdin", false], ["", true]] as const) {
    test(`in-process lifeline (GIZZI_PARENT_LIFELINE=${env || "unset"}): child ${survives ? "survives" : "exits"}`, async () => {
      const dir = mkdtempSync(join(tmpdir(), "gizzi-lifeline-"))
      const pidFile = join(dir, "pid")
      const childSrc = `const { onParentExit } = await import(${JSON.stringify(LIFELINE)});
        onParentExit(() => process.exit(0)); setInterval(() => {}, 1000)`
      const parent = spawn(
        process.execPath,
        [
          "-e",
          `const { spawn } = await import("node:child_process");
           const { writeFileSync } = await import("node:fs");
           const c = spawn(process.execPath, ["-e", ${JSON.stringify(childSrc)}], {
             stdio: ["pipe", "ignore", "ignore"], env: { ...process.env, GIZZI_PARENT_LIFELINE: ${JSON.stringify(env)} } });
           writeFileSync(${JSON.stringify(pidFile)}, String(c.pid));
           setInterval(() => {}, 1000)`,
        ],
        { stdio: "ignore" },
      )
      expect(await until(() => existsSync(pidFile) && readFileSync(pidFile, "utf8") !== "", 5000)).toBe(true)
      const child = Number(readFileSync(pidFile, "utf8"))
      await Bun.sleep(300)

      parent.kill("SIGKILL")

      if (survives) {
        await Bun.sleep(500)
        expect(alive(child)).toBe(true)
        process.kill(child, "SIGKILL")
      } else {
        expect(await until(() => !alive(child))).toBe(true)
      }
    })
  }

  test("exit hook runs when gizzi is SIGKILLed, and doesn't hold gizzi open", async () => {
    const dir = mkdtempSync(join(tmpdir(), "gizzi-exit-hook-"))
    const ran = join(dir, "ran")
    const parent = spawn(
      process.execPath,
      [
        "-e",
        `const { runOnGizziExit } = await import(${JSON.stringify(LIFELINE)});
         runOnGizziExit("/bin/sh", ["-c", "echo ok > ${ran}"]);
         setInterval(() => {}, 1000)`,
      ],
      { stdio: "ignore" },
    )
    await Bun.sleep(500)
    expect(existsSync(ran)).toBe(false)
    parent.kill("SIGKILL")
    expect(await until(() => existsSync(ran))).toBe(true)

    // A gizzi that just finishes still exits on its own, and the hook fires.
    const ran2 = join(dir, "ran2")
    const quick = spawn(
      process.execPath,
      [
        "-e",
        `const { runOnGizziExit } = await import(${JSON.stringify(LIFELINE)});
         runOnGizziExit("/bin/sh", ["-c", "echo ok > ${ran2}"])`,
      ],
      { stdio: "ignore" },
    )
    const code = await new Promise<number | null>((resolve) => quick.once("exit", resolve))
    expect(code).toBe(0)
    expect(await until(() => existsSync(ran2))).toBe(true)
  })
})
