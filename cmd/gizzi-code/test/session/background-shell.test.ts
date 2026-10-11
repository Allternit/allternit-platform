import { describe, expect, test } from "bun:test"
import { spawn } from "child_process"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { BackgroundTask } from "../../src/runtime/session/background-task"
import { runInBackground } from "../../src/runtime/tools/builtins/bash"
import { tmpdir } from "../fixture/fixture"

function taskID(output: string): string {
  return /background_task_id: (\S+)/.exec(output)![1]
}

describe("background shell", () => {
  test("streams output and settles from the exit code", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const parent = await Session.create({})
        const command = "echo first; sleep 0.8; echo second; exit 3"
        const proc = spawn(command, { shell: true, stdio: ["ignore", "pipe", "pipe"] })
        const result = await runInBackground(proc, { sessionID: parent.id, command, description: "two lines" })
        expect(result.metadata).toMatchObject({ background: true })
        const id = taskID(result.output)

        // Live: the first line lands before the process exits.
        await Bun.sleep(700)
        expect((await BackgroundTask.get(id))?.output).toContain("first")

        const settled = await BackgroundTask.wait(id, 5_000)
        expect(settled).toMatchObject({ kind: "shell", status: "failed", error: "Exited with code 3" })
        expect(settled?.output).toBe("$ echo first; sleep 0.8; echo second; exit 3\nfirst\nsecond\n")
        await Session.remove(parent.id)
      },
    })
  })

  test("cancel kills the process and records cancelled", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const parent = await Session.create({})
        const proc = spawn("sleep 30", { shell: true, stdio: ["ignore", "pipe", "pipe"], detached: true })
        const result = await runInBackground(proc, { sessionID: parent.id, command: "sleep 30", description: "sleeper" })
        const id = taskID(result.output)
        const exited = new Promise<void>((resolve) => proc.once("exit", () => resolve()))

        expect(await BackgroundTask.cancel(id)).toMatchObject({ status: "cancelled" })
        await exited
        await Bun.sleep(50)
        // The exit after cancel does not overwrite the cancelled record.
        expect(await BackgroundTask.get(id)).toMatchObject({ status: "cancelled" })
        await Session.remove(parent.id)
      },
    })
  })
})
