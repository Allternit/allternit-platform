import { Database } from "@/runtime/session/storage/db"
import { LoopTable } from "@/runtime/session/session.sql"
import { eq } from "drizzle-orm"
import { spawn } from "child_process"
import { armStats, noiseFloor, type MetricDirection } from "./arm-stats"

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
    if (loop.metric) return this.runMeasured(id)

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

  /** Last number printed by the instrument; null when it failed or printed none. */
  private static async measure(command: string, cwd?: string): Promise<number | null> {
    const out = await this.runCommand(command, cwd)
    if (out.exitCode !== 0) return null
    const nums = out.stdout.replace(/\u001b\[[0-9;]*[A-Za-z]/g, "").match(/-?\d+(?:\.\d+)?(?:[eE][-+]?\d+)?/g)
    if (!nums) return null
    const v = Number(nums[nums.length - 1])
    return Number.isFinite(v) ? v : null
  }

  private static async git(args: string, cwd?: string): Promise<{ ok: boolean; out: string }> {
    const r = await this.runCommand(`git ${args}`, cwd)
    return { ok: r.exitCode === 0, out: r.stdout.trim() }
  }

  private static async rounds(command: string, n: number, cwd?: string): Promise<Array<number | null>> {
    const xs: Array<number | null> = []
    for (let i = 0; i < n; i++) xs.push(await this.measure(command, cwd))
    return xs
  }

  /**
   * Dry run: runs only the instrument, once, and reports what a real run
   * would do. Changes nothing in the tree or the loop record.
   */
  static async dryRun(id: string): Promise<{ outcome: "NOOP" | "BLOCKED"; value: number | null; note: string }> {
    const loop = Database.use((db) => db.select().from(LoopTable).where(eq(LoopTable.id, id)).get())
    if (!loop?.metric) return { outcome: "BLOCKED", value: null, note: "loop has no metric; nothing to measure" }
    const cwd = loop.cwd ?? undefined
    const clean = await this.git("status --porcelain", cwd)
    if (!clean.ok) return { outcome: "BLOCKED", value: null, note: "not a git work tree; revert is impossible" }
    if (clean.out) return { outcome: "BLOCKED", value: null, note: "working tree has uncommitted changes; a real run would refuse to start" }
    const value = await this.measure(loop.metric.command, cwd)
    if (value === null) return { outcome: "BLOCKED", value, note: "the instrument failed or printed no number" }
    return { outcome: "NOOP", value, note: `instrument works (${value}); a real run would run ${loop.metric.noiseRuns} A/A rounds, then iterate` }
  }

  /**
   * Measured loop: A/A noise, baseline, then per iteration: change, instrument
   * n times, keep or revert, ledger row. Stops when the change stays inside
   * the noise for `stallLimit` iterations in a row.
   */
  private static async runMeasured(id: string): Promise<void> {
    const read = () => Database.use((db) => db.select().from(LoopTable).where(eq(LoopTable.id, id)).get())
    const save = (set: Partial<typeof LoopTable.$inferInsert>) =>
      Database.use((db) => db.update(LoopTable).set({ ...set, time_updated: Date.now() }).where(eq(LoopTable.id, id)).run())
    const loop = read()
    if (!loop?.metric) return
    const m = loop.metric
    const cwd = loop.cwd ?? undefined
    const direction: MetricDirection = m.direction
    const n = Math.max(1, m.minPairs)
    const stallLimit = m.stallLimit ?? 3
    const maxIterations = loop.max_iterations ?? 10
    const ledger = [...(loop.ledger ?? [])]
    const blocked = (why: string) => {
      ledger.push({ iteration: ledger.length + 1, hypothesis: loop.command, delta: null, ci: null, n: 0, noise: 0, verdict: "INCONCLUSIVE", why, outcome: "BLOCKED", control: [], treatment: [], timestamp: new Date().toISOString() })
      save({ ledger, state: "blocked", last_outcome: "BLOCKED" })
    }

    // The tree must be clean: a rejected change is undone with git, so
    // anything uncommitted would be lost or mixed into the attempt.
    const status = await this.git("status --porcelain", cwd)
    if (!status.ok) return blocked("not a git work tree; a rejected change cannot be reverted")
    if (status.out) return blocked("working tree has uncommitted changes; commit or stash them first")

    // Noise floor from unchanged runs.
    const aa = await this.rounds(m.command, Math.max(2, m.noiseRuns), cwd)
    if (aa.filter((x) => x !== null).length < 2) return blocked("the instrument failed on the unchanged tree")
    const noise = noiseFloor(aa)
    let control = await this.rounds(m.command, n, cwd)
    if (control.some((x) => x === null) && control.every((x) => x === null)) return blocked("the instrument failed for the baseline")

    // Rejects are read before new attempts: the previous hypotheses are
    // exposed to the change command through the environment.
    let stalled = 0
    for (let iteration = ledger.length + 1; iteration <= maxIterations; iteration++) {
      if (read()?.state !== "running") return
      const head = await this.git("rev-parse HEAD", cwd)
      const before = await this.untracked(cwd)
      const rejected = ledger.filter((r) => r.verdict !== "KEEP").map((r) => `${r.verdict}: ${r.hypothesis} (${r.why})`)
      const step = await this.runCommandWithEnv(loop.command, cwd, { LOOP_ITERATION: String(iteration), LOOP_REJECTED: JSON.stringify(rejected) })
      const dirty = (await this.git("status --porcelain", cwd)).out
      const moved = (await this.git("rev-parse HEAD", cwd)).out !== head.out
      if (step.exitCode !== 0 || (!dirty && !moved)) {
        ledger.push({ iteration, hypothesis: loop.command, delta: null, ci: null, n: 0, noise, verdict: "INCONCLUSIVE", why: step.exitCode !== 0 ? `change step exited ${step.exitCode}` : "the change step changed nothing", outcome: "NOOP", control, treatment: [], timestamp: new Date().toISOString() })
        stalled++
        await this.revert(head.out, before, cwd)
        save({ ledger, last_outcome: "NOOP" })
        if (stalled >= stallLimit) break
        continue
      }
      const treatment = await this.rounds(m.command, n, cwd)
      const res = armStats({ control, treatment, direction, keepBar: m.keepBar, noise, minPairs: m.minPairs, seed: iteration })
      const kept = res.verdict === "KEEP"
      if (!kept) await this.revert(head.out, before, cwd)
      else {
        control = treatment
        if (!(await this.commitKept(iteration, cwd))) return blocked("could not commit the kept change; stopped so it is not reverted by the next attempt")
      }
      stalled = res.delta > noise ? 0 : stalled + 1
      ledger.push({ iteration, hypothesis: loop.command, delta: res.delta, ci: res.ci, n: res.pairs, noise, verdict: res.verdict, why: res.why, outcome: "ACTED", control, treatment, timestamp: new Date().toISOString() })
      save({ ledger, last_outcome: "ACTED" })
      if (stalled >= stallLimit) break
    }
    if (read()?.state === "running") {
      const last = ledger[ledger.length - 1]
      save({ state: ledger.some((r) => r.verdict === "KEEP") ? "succeeded" : last?.outcome === "BLOCKED" ? "blocked" : "max_iterations" })
    }
  }

  private static async untracked(cwd?: string): Promise<string[]> {
    const r = await this.git("ls-files --others --exclude-standard", cwd)
    return r.out ? r.out.split("\n").filter(Boolean) : []
  }

  /**
   * Undo an attempt: revert commits made since `head`, restore tracked files,
   * and remove only the untracked paths the change step created (those not in
   * `before`). Untracked files that existed before the attempt are never touched.
   */
  private static async revert(head: string, before: string[], cwd?: string): Promise<void> {
    await this.git("checkout -- .", cwd)
    const keep = new Set(before)
    const created = (await this.untracked(cwd)).filter((f) => !keep.has(f))
    for (const f of created) {
      const quoted = "'" + f.replace(/'/g, "'\\''") + "'"
      await this.git(`clean -fd -- ${quoted}`, cwd)
    }
    const now = (await this.git("rev-parse HEAD", cwd)).out
    if (head && now !== head) await this.git(`revert --no-edit ${head}..HEAD`, cwd)
  }

  /** Commit a kept change so the next attempt starts from a clean tree. */
  private static async commitKept(iteration: number, cwd?: string): Promise<boolean> {
    if (!(await this.git("status --porcelain", cwd)).out) return true
    await this.git("add -A", cwd)
    // A fresh runtime may have no git identity; without one the commit fails
    // and the next revert would throw the kept change away.
    const named = (await this.git("config user.email", cwd)).out
    const who = named ? "" : `-c user.name="Allternit Loop" -c user.email=loop@allternit.local `
    const done = await this.git(`${who}commit -q -m "loop: keep attempt ${iteration}"`, cwd)
    return done.ok
  }

  private static runCommandWithEnv(command: string, cwd: string | undefined, env: Record<string, string>) {
    return this.runCommand(command, cwd, env)
  }

  private static runCommand(command: string, cwd?: string, extraEnv?: Record<string, string>): Promise<{ stdout: string; stderr: string; exitCode: number }> {
    return new Promise((resolve) => {
      let stdout = ""
      let stderr = ""
      
      const child = spawn(command, {
        shell: true,
        cwd,
        env: { ...process.env, FORCE_COLOR: "1", ...extraEnv },
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
