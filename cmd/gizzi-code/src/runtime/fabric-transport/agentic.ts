/**
 * Agentic job kind (consumer-packaged Cowork P2.2): the worker runs a
 * bounded model-agent loop for a claimed job instead of plain shell steps.
 *
 * - Model access goes through the EXISTING model router — POST
 *   `{ALLTERNIT_API_URL}/v1/chat/completions` with the operator key the
 *   desktop injected at spawn. No new LLM path.
 * - Tools: `fs_read` / `fs_write` (confined to the granted trusted folders;
 *   default-deny outside — P2.3) and `bash` (existing Sandbox.wrap posture).
 * - Per-committed-step checkpointing via the canonical run checkpoint
 *   endpoint so lease-failover machinery replays correctly.
 * - Budget caps: max steps and max tokens from the job payload (with
 *   conservative defaults); exceeding either ends the loop with a typed,
 *   honest result.
 */

import { spawn } from "node:child_process"
import { Sandbox } from "../integrations/shell/sandbox"

export interface AgenticToolResult {
  ok: boolean
  output: string
}

export interface AgenticDeps {
  apiBase: string
  operatorKey: string | null
  model: string
  grants: string[]
  runStep?: (command: string, cwd: string, sessionId: string) => Promise<{ code: number; output: string }>
  checkpoint: (stepIndex: number, cursor: Record<string, unknown>) => Promise<void>
  complete: (success: boolean, summary: string, outputs: Record<string, unknown>) => Promise<void>
  /** P3.1: attach a rendered deliverable to this run (worker bearer auth). */
  createDeliverable?: (input: {
    name: string
    template: string
    title?: string
    markdown: string
  }) => Promise<{ ok: boolean; detail: string }>
  log: (level: "info" | "warn" | "error", event: string, fields?: Record<string, unknown>) => void
  fetchImpl?: typeof fetch
  /**
   * The bot's own instructions (identity, standing instructions, memory)
   * when the job targets a bot principal (spec P4.1). Prepended to the
   * worker rules, which still bind: grants and the step budget don't move.
   */
  system?: string
  maxSteps?: number
  maxTokens?: number
}

interface ChatMessage {
  role: "system" | "user" | "assistant" | "tool"
  content: string
  tool_calls?: Array<{
    id: string
    type: "function"
    function: { name: string; arguments: string }
  }>
  tool_call_id?: string
}

const TOOL_DEFS = [
  {
    type: "function",
    function: {
      name: "fs_read",
      description: "Read a text file. Only works inside granted folders.",
      parameters: {
        type: "object",
        properties: { path: { type: "string" } },
        required: ["path"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "fs_write",
      description: "Write a text file. Only works inside granted folders.",
      parameters: {
        type: "object",
        properties: {
          path: { type: "string" },
          content: { type: "string" },
        },
        required: ["path", "content"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "deliverable",
      description:
        "Render a finished office document (report .docx / sheet .xlsx / deck .pptx) " +
        "from markdown and attach it to this run for preview/export.",
      parameters: {
        type: "object",
        properties: {
          name: { type: "string" },
          template: { type: "string", enum: ["report", "sheet", "deck"] },
          title: { type: "string" },
          markdown: { type: "string" },
        },
        required: ["name", "template", "markdown"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "bash",
      description: "Run a shell command in the sandboxed worker posture.",
      parameters: {
        type: "object",
        properties: { command: { type: "string" } },
        required: ["command"],
      },
    },
  },
]

/** Resolve `p` against the grant roots; null when outside every grant. */
export function confineToGrants(p: string, grants: string[]): string | null {
  if (grants.length === 0) return null
  const path = require("node:path") as typeof import("node:path")
  const fs = require("node:fs") as typeof import("node:fs")
  const resolved = path.resolve(p)
  for (const grant of grants) {
    const root = path.resolve(grant)
    if (resolved === root || resolved.startsWith(root + path.sep)) {
      try {
        // Refuse symlinks that escape the grant.
        const real = fs.realpathSync.native(resolved)
        const realRoot = fs.realpathSync.native(root)
        if (real === realRoot || real.startsWith(realRoot + path.sep)) return resolved
      } catch {
        // New files under an existing grant root are fine.
        if (resolved.startsWith(root + path.sep)) return resolved
      }
      return null
    }
  }
  return null
}

export function parseGrants(raw: string | undefined | null): string[] {
  if (!raw) return []
  try {
    const parsed = JSON.parse(raw)
    return Array.isArray(parsed) ? parsed.filter((g): g is string => typeof g === "string") : []
  } catch {
    return []
  }
}

async function executeTool(
  name: string,
  argsJson: string,
  deps: AgenticDeps,
  written: string[],
): Promise<AgenticToolResult> {
  let args: Record<string, unknown> = {}
  try {
    args = JSON.parse(argsJson) as Record<string, unknown>
  } catch {
    return { ok: false, output: "invalid tool arguments (not JSON)" }
  }
  const fs = await import("node:fs/promises")
  if (name === "fs_read" || name === "fs_write") {
    const target = confineToGrants(String(args.path ?? ""), deps.grants)
    if (!target) {
      return {
        ok: false,
        output:
          `refused: ${String(args.path ?? "")} is outside the granted folders ` +
          `(default-deny; grants: ${deps.grants.join(", ") || "none"})`,
      }
    }
    try {
      if (name === "fs_read") {
        return { ok: true, output: (await fs.readFile(target, "utf8")).slice(0, 20000) }
      }
      await fs.mkdir(require("node:path").dirname(target), { recursive: true })
      await fs.writeFile(target, String(args.content ?? ""), "utf8")
      written.push(target)
      return { ok: true, output: `wrote ${target} (${String(args.content ?? "").length} bytes)` }
    } catch (e) {
      return { ok: false, output: `fs error: ${(e as Error).message}` }
    }
  }
  if (name === "deliverable") {
    if (!deps.createDeliverable) {
      return { ok: false, output: "deliverable tool unavailable in this worker" }
    }
    const template = String(args.template ?? "report")
    if (!["report", "sheet", "deck"].includes(template)) {
      return { ok: false, output: `unknown template: ${template}` }
    }
    const result = await deps.createDeliverable({
      name: String(args.name ?? "deliverable"),
      template,
      title: typeof args.title === "string" ? args.title : undefined,
      markdown: String(args.markdown ?? ""),
    })
    return { ok: result.ok, output: result.detail }
  }
  if (name === "bash") {
    const command = String(args.command ?? "")
    if (!command) return { ok: false, output: "empty command" }
    const { code, output } = await deps.runStep(command, process.cwd(), "agentic")
    return { ok: code === 0, output: output.slice(0, 20000) }
  }
  return { ok: false, output: `unknown tool: ${name}` }
}

export async function runAgenticLoop(task: string, deps: AgenticDeps): Promise<void> {
  const fetchImpl = deps.fetchImpl ?? fetch
  const maxSteps = deps.maxSteps ?? 8
  const maxTokens = deps.maxTokens ?? 20_000
  const written: string[] = []
  let tokens = 0
  let steps = 0

  if (!deps.operatorKey) {
    await deps.complete(false, "agentic job refused: no operator model key in worker env", {})
    return
  }

  const messages: ChatMessage[] = [
    {
      role: "system",
      content:
        (deps.system?.trim() ? `${deps.system.trim()}\n\n# Worker rules\n\n` : "") +
        "You are an agentic worker executing a delegated task. Use tools to inspect and " +
        "modify files. You may only write inside the granted folders. Finish with a short " +
        `summary of what you did. Granted folders: ${deps.grants.join(", ") || "none"}.`,
    },
    { role: "user", content: task },
  ]

  const sandboxedRun = async (command: string): Promise<{ code: number; output: string }> => {
    const wrapped = await Sandbox.wrap({
      command,
      shell: "/bin/bash",
      cwd: process.cwd(),
      sessionID: "agentic",
      policy: { allowNetwork: false, allowWritePaths: [], allowedDomains: [] },
    })
    const bin = wrapped?.bin ?? "/bin/bash"
    const shellArgs = wrapped?.args ?? ["-c", command]
    return new Promise((resolve) => {
      const child = spawn(bin, shellArgs, { cwd: process.cwd() })
      let output = ""
      child.stdout.on("data", (d) => (output += String(d)))
      child.stderr.on("data", (d) => (output += String(d)))
      child.on("close", (code) => resolve({ code: code ?? 1, output }))
    })
  }
  const runStep = deps.runStep ?? sandboxedRun

  for (steps = 1; steps <= maxSteps; steps++) {
    const res = await fetchImpl(`${deps.apiBase}/v1/chat/completions`, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        authorization: `Bearer ${deps.operatorKey}`,
      },
      body: JSON.stringify({
        model: deps.model,
        messages,
        tools: TOOL_DEFS,
        temperature: 0.2,
      }),
    })
    if (!res.ok) {
      await deps.complete(false, `model router returned ${res.status}`, { steps, tokens })
      return
    }
    const data = (await res.json()) as {
      usage?: { total_tokens?: number }
      choices?: Array<{
        message?: ChatMessage & { content?: string | null }
        finish_reason?: string | null
      }>
    }
    tokens += data.usage?.total_tokens ?? 0
    const choice = data.choices?.[0]
    const assistant = choice?.message
    if (assistant?.content) {
      messages.push({ role: "assistant", content: assistant.content })
    }
    const toolCalls = assistant?.tool_calls ?? []
    if (toolCalls.length === 0) {
      const summary = assistant?.content?.trim() || "agent finished without a final message"
      await deps.complete(true, summary, { steps, tokens, artifacts: written })
      return
    }
    messages.push({ ...assistant, role: "assistant" } as ChatMessage)
    for (const call of toolCalls) {
      const result = await executeTool(call.function.name, call.function.arguments, deps, written)
      deps.log("info", "worker.agentic_step", {
        step: steps,
        tool: call.function.name,
        ok: result.ok,
      })
      messages.push({
        role: "tool",
        tool_call_id: call.id,
        content: result.output,
      })
      await deps.checkpoint(steps, {
        completed_steps: steps,
        tokens,
        last_tool: call.function.name,
        artifacts: written,
        by: "a://principal/gizzi",
      })
    }
    if (tokens > maxTokens) {
      await deps.complete(false, `token budget exceeded (${tokens} > ${maxTokens})`, {
        steps,
        tokens,
        artifacts: written,
      })
      return
    }
  }
  await deps.complete(false, `step budget exceeded (${maxSteps})`, {
    steps,
    tokens,
    artifacts: written,
  })
}
