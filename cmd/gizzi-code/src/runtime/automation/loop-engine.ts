import { Database } from "@/runtime/session/storage/db"
import { LoopTable } from "@/runtime/session/session.sql"
import { eq } from "drizzle-orm"
import { spawn } from "child_process"

export interface LoopLogEntry {
  iteration: number;
  output: string;
  exitCode: number;
  timestamp: string;
}

export class LoopEngine {
  static async startLoop(id: string): Promise<void> {
    const loop = Database.use((db) =>
      db.select().from(LoopTable).where(eq(LoopTable.id, id)).get()
    )

    if (!loop || loop.state !== "running") return

    const maxIterations = loop.max_iterations ?? 10
    let iteration = 0
    const logs: LoopLogEntry[] = []

    while (iteration < maxIterations) {
      // Check if loop status changed (aborted / stopped) in the DB
      const currentLoop = Database.use((db) =>
        db.select().from(LoopTable).where(eq(LoopTable.id, id)).get()
      )
      if (!currentLoop || currentLoop.state !== "running") {
        break
      }

      iteration++

      // Execute command
      const output = await this.runCommand(loop.command)

      const logEntry: LoopLogEntry = {
        iteration,
        output: output.stdout + output.stderr,
        exitCode: output.exitCode,
        timestamp: new Date().toISOString(),
      }
      logs.push(logEntry)

      // Save iteration log to DB
      Database.use((db) =>
        db
          .update(LoopTable)
          .set({
            iteration_log: logs,
            time_updated: Date.now(),
          })
          .where(eq(LoopTable.id, id))
          .run()
      )

      // Evaluate exit condition
      let shouldExit = false
      if (loop.exit_condition) {
        if (loop.exit_condition === "exit_code_zero" && output.exitCode === 0) {
          shouldExit = true
        } else if (logEntry.output.includes(loop.exit_condition)) {
          shouldExit = true
        }
      } else if (output.exitCode === 0) {
        // default exit condition: exit code zero
        shouldExit = true
      }

      if (shouldExit) {
        Database.use((db) =>
          db
            .update(LoopTable)
            .set({
              state: "succeeded",
              time_updated: Date.now(),
            })
            .where(eq(LoopTable.id, id))
            .run()
        )
        return
      }

      // Wait a brief moment before next run to avoid CPU thrashing
      await new Promise((resolve) => setTimeout(resolve, 1000))
    }

    // If we finished the loop without success, set state to failed / max_iterations
    const finalLoop = Database.use((db) =>
      db.select().from(LoopTable).where(eq(LoopTable.id, id)).get()
    )
    if (finalLoop && finalLoop.state === "running") {
      Database.use((db) =>
        db
          .update(LoopTable)
          .set({
            state: "max_iterations",
            time_updated: Date.now(),
          })
          .where(eq(LoopTable.id, id))
          .run()
      )
    }
  }

  private static runCommand(command: string): Promise<{ stdout: string; stderr: string; exitCode: number }> {
    return new Promise((resolve) => {
      let stdout = ""
      let stderr = ""
      
      const child = spawn(command, {
        shell: true,
        env: { ...process.env, FORCE_COLOR: "1" },
      })

      child.stdout?.on("data", (data) => {
        stdout += data.toString()
      })

      child.stderr?.on("data", (data) => {
        stderr += data.toString()
      })

      // A signal kill gives code null: count it as a failure, not success.
      child.on("close", (code, signal) => {
        resolve({
          stdout,
          stderr,
          exitCode: code ?? (signal ? 128 : 1),
        })
      })
    })
  }
}
