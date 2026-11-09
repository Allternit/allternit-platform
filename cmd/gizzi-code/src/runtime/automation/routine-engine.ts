import { Database } from "@/runtime/session/storage/db"
import { RoutineTable } from "@/runtime/session/session.sql"
import { eq } from "drizzle-orm"
import { spawn } from "child_process"

export class RoutineEngine {
  static async startRoutine(id: string): Promise<void> {
    const routine = Database.use((db) =>
      db.select().from(RoutineTable).where(eq(RoutineTable.id, id)).get()
    )

    if (!routine || routine.state !== "running") return

    const steps = [...routine.steps]
    let failed = false

    for (let i = 0; i < steps.length; i++) {
      // Check if routine state changed (aborted / paused) in DB
      const currentRoutine = Database.use((db) =>
        db.select().from(RoutineTable).where(eq(RoutineTable.id, id)).get()
      )
      if (!currentRoutine || currentRoutine.state !== "running") {
        break
      }

      steps[i].status = "running"
      Database.use((db) =>
        db
          .update(RoutineTable)
          .set({
            steps,
            time_updated: Date.now(),
          })
          .where(eq(RoutineTable.id, id))
          .run()
      )

      // Run sequential step
      const exitCode = await this.runStep(steps[i].command)

      if (exitCode === 0) {
        steps[i].status = "done"
      } else {
        steps[i].status = "failed"
        failed = true
      }

      Database.use((db) =>
        db
          .update(RoutineTable)
          .set({
            steps,
            time_updated: Date.now(),
          })
          .where(eq(RoutineTable.id, id))
          .run()
      )

      if (failed) {
        break
      }
    }

    const finalState = failed ? "failed" : "completed"
    Database.use((db) =>
      db
        .update(RoutineTable)
        .set({
          state: finalState,
          time_updated: Date.now(),
        })
        .where(eq(RoutineTable.id, id))
        .run()
    )
  }

  private static runStep(command: string): Promise<number> {
    return new Promise((resolve) => {
      const child = spawn(command, {
        shell: true,
      })

      // A signal kill gives code null: count it as a failure, not success.
      child.on("close", (code, signal) => {
        resolve(code ?? (signal ? 128 : 1))
      })
    })
  }
}
