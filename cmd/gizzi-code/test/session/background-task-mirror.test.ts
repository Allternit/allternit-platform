import { describe, expect, test } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { BackgroundTask } from "../../src/runtime/session/background-task"
import { BackgroundTaskMirror } from "../../src/runtime/session/background-task-mirror"
import { SessionTasks } from "../../src/runtime/session/session-tasks"
import { mirrorStatus, mirrorTaskId } from "../../src/cli/ui/ink-app/components/TaskMirrorBridge"
import { tmpdir } from "../fixture/fixture"

describe("background task mirror (TUI-owned tasks)", () => {
  test("maps TUI task status and ids", () => {
    expect(mirrorStatus("pending")).toBe("queued")
    expect(mirrorStatus("running")).toBe("running")
    expect(mirrorStatus("completed")).toBe("completed")
    expect(mirrorStatus("failed")).toBe("failed")
    expect(mirrorStatus("killed")).toBe("cancelled")
    expect(mirrorTaskId("b123")).toBe("tui_b123")
  })

  test("mirrored rows appear in the session's tasks and a surface's cancel wins", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const parent = await Session.create({})
        expect(BackgroundTaskMirror.sessionExists(parent.id)).toBe(true)
        expect(BackgroundTaskMirror.sessionExists("ses_missing")).toBe(false)

        BackgroundTaskMirror.start({
          id: "tui_b1",
          parentSessionID: parent.id,
          kind: "shell",
          description: "npm run dev",
          status: "running",
          created: 1_000,
        })
        // A repeated start is a no-op.
        BackgroundTaskMirror.start({ id: "tui_b1", parentSessionID: parent.id, kind: "shell", description: "x", status: "running", created: 2 })
        expect(BackgroundTaskMirror.update("tui_b1", { status: "running", output: "ready\n" })).toBe(true)

        const items = await SessionTasks.list(parent.id)
        expect(items).toHaveLength(1)
        expect(items[0]).toMatchObject({ id: "tui_b1", kind: "shell", status: "running", background: true, output: "ready\n" })

        // A surface stops it: the daemon settles the row as cancelled.
        await BackgroundTask.cancel("tui_b1")
        expect(BackgroundTaskMirror.statuses(["tui_b1", "tui_none"])).toEqual({ "tui_b1": "cancelled" })
        // The TUI's own later "killed → cancelled"/"completed" write cannot overwrite it.
        expect(BackgroundTaskMirror.update("tui_b1", { status: "completed", finished: 3_000 })).toBe(false)
        expect((await BackgroundTask.get("tui_b1"))?.status).toBe("cancelled")
        await Session.remove(parent.id)
      },
    })
  })
})
