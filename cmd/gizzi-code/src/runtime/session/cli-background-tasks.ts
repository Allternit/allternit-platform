import { createHash } from "node:crypto"
import fs from "node:fs/promises"
import os from "node:os"
import path from "node:path"
import { BackgroundTask } from "./background-task"

/** The vendor owns execution; this bridge observes it without executing tools again. */
export class CliBackgroundTasks {
  private tasks = new Map<string, { id: string; outputFile?: string; output: string; active: boolean }>()
  private calls = new Map<string, { description: string; kind?: "shell" | "subagent" }>()
  private timer?: ReturnType<typeof setInterval>
  private polling = false
  private closed = false

  constructor(
    private sessionID: string,
    private stopTask: (vendorID: string) => Promise<void>,
  ) {}

  get active() {
    return [...this.tasks.values()].some((task) => task.active)
  }

  private async ensure(vendorID: string, description: string, kind: string) {
    if (!/^[\w-]{1,128}$/.test(vendorID)) return
    const known = this.tasks.get(vendorID)
    if (known) return known
    const id = `cli_${createHash("sha256").update(`${this.sessionID}\0${vendorID}`).digest("hex").slice(0, 32)}`
    let row = await BackgroundTask.get(id)
    if (!row)
      row = await BackgroundTask.create({
        id,
        parentSessionID: this.sessionID,
        kind,
        description: description.slice(0, 1000),
      })
    const task: {
      id: string
      outputFile?: string
      output: string
      active: boolean
    } = { id, output: row.output ?? "", active: row.status === "running" }
    this.tasks.set(vendorID, task)
    if (task.active)
      BackgroundTask.onCancel(id, async () => {
        await this.stopTask(vendorID)
        task.active = false
      })
    if (!this.timer) {
      this.timer = setInterval(() => void this.poll(), 500)
      this.timer.unref?.()
    }
    return task
  }

  async observe(evt: Record<string, any>) {
    if (this.closed) return
    if (evt.type === "assistant" && Array.isArray(evt.message?.content)) {
      for (const part of evt.message.content) {
        if (part?.type !== "tool_use") continue
        this.calls.set(String(part.id), {
          description: String(part.input?.description ?? part.input?.command ?? part.name),
          kind: part.name === "Bash" ? "shell" : ["Agent", "Task"].includes(part.name) ? "subagent" : undefined,
        })
      }
    }
    if (evt.type === "user") {
      const content = evt.message?.content ?? evt.content
      if (!Array.isArray(content)) return
      for (const part of content) {
        const call = this.calls.get(String(part?.tool_use_id))
        if (part?.type !== "tool_result" || part.is_error || !call?.kind) continue
        const text =
          typeof part.content === "string"
            ? part.content
            : Array.isArray(part.content)
              ? part.content.map((block: any) => (block?.type === "text" ? String(block.text ?? "") : "")).join("\n")
              : ""
        const match =
          /Command running in background with ID: ([\w-]+)\. Output is being written to: (.+?\.output)(?:\.|\n|$)/.exec(
            text,
          )
        const agentMatch = text.startsWith("Async agent launched successfully.")
          ? /(?:^|\n)agentId: ([\w-]+)/.exec(text)
          : null
        const metadata = evt.tool_use_result
        const vendorID =
          call.kind === "shell"
            ? (metadata?.backgroundTaskId ?? match?.[1])
            : metadata?.status === "async_launched"
              ? metadata.agentId
              : agentMatch?.[1]
        if (typeof vendorID !== "string") continue
        const task = await this.ensure(vendorID, call.description, call.kind)
        if (task && match?.[1] === vendorID) task.outputFile = match[2]
        if (task && call.kind === "subagent") {
          const file = metadata?.outputFile ?? /(?:^|\n)output_file: ([^\n]+)/.exec(text)?.[1]
          if (typeof file === "string") task.outputFile = file
        }
      }
      await this.poll()
      return
    }
    if (evt.type !== "system" || !["task_started", "task_progress", "task_notification"].includes(evt.subtype)) return
    const vendorID = evt.task_id
    if (typeof vendorID !== "string") return
    const call = this.calls.get(String(evt.tool_use_id))
    const kind = /bash|shell/.test(String(evt.task_type)) || call?.kind === "shell" ? "shell" : "subagent"
    const task = await this.ensure(
      vendorID,
      String(evt.description ?? call?.description ?? `CLI task ${vendorID}`),
      kind,
    )
    if (!task || !task.active) return
    if (typeof evt.output_file === "string") task.outputFile = evt.output_file
    await this.readOutput(vendorID, task)
    if (evt.subtype === "task_progress" && !task.outputFile && typeof evt.summary === "string") {
      task.output = BackgroundTask.tail(evt.summary)
      await BackgroundTask.progress(task.id, task.output)
    }
    if (evt.subtype !== "task_notification") return
    // Never interpret an unknown provider status as success.
    if (!["completed", "failed", "stopped"].includes(evt.status)) return
    task.active = false
    const output = task.output || String(evt.summary ?? "")
    if (evt.status === "completed") await BackgroundTask.complete(task.id, output)
    else if (evt.status === "failed")
      await BackgroundTask.fail(task.id, String(evt.summary ?? "CLI task failed"), output)
    else await BackgroundTask.recordCancellation(task.id, "Stopped by CLI", output)
  }

  private async readOutput(
    vendorID: string,
    task: { id: string; outputFile?: string; output: string; active: boolean },
  ) {
    if (!task.active || !task.outputFile) return
    // Only read the CLI's task output, never an arbitrary path named in a tool result.
    try {
      const file = await fs.realpath(task.outputFile)
      const roots = await Promise.all(
        [os.tmpdir(), "/tmp", process.env.CLAUDE_CODE_TMPDIR]
          .filter((root): root is string => !!root)
          .map((root) => fs.realpath(root).catch(() => "")),
      )
      if (
        path.basename(file) !== `${vendorID}.output` ||
        path.basename(path.dirname(file)) !== "tasks" ||
        !roots.some((root) => root && file.startsWith(root + path.sep))
      )
        return
      const handle = await fs.open(file, "r")
      let output: string
      try {
        const size = (await handle.stat()).size
        const buf = Buffer.alloc(Math.min(size, BackgroundTask.MAX_LIVE_OUTPUT))
        const { bytesRead } = await handle.read(buf, 0, buf.length, Math.max(0, size - buf.length))
        output = buf.subarray(0, bytesRead).toString("utf8")
      } finally {
        await handle.close()
      }
      if (output !== task.output) {
        await BackgroundTask.progress(task.id, output)
        task.output = output
      }
    } catch {
      /* Output files may not exist until the first write. */
    }
  }

  async poll() {
    if (this.polling || this.closed) return
    this.polling = true
    try {
      for (const [id, task] of this.tasks) await this.readOutput(id, task)
    } finally {
      this.polling = false
    }
  }

  async close(reason = "The CLI exited before reporting a task outcome.") {
    if (this.timer) clearInterval(this.timer)
    await this.poll()
    this.closed = true
    for (const task of this.tasks.values()) {
      if (!task.active) continue
      task.active = false
      await BackgroundTask.interrupt(task.id, reason, task.output)
    }
  }
}
