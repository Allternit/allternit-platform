/**
 * `computers` tool: computer lifecycle and files for gizzi.
 *
 * Replaces the old `desktop` tool (bot desktop routes, 2026-10-09 D0). Every
 * action runs one of allternit-api's `computer_*` tools through
 * `POST /api/v1/tools/execute`, the same control plane the web app and the MCP
 * server use. Screen, mouse and keyboard go through the toolset `computer`
 * tool, so they are not duplicated here.
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { platformFetch } from "@/runtime/bots/platform-api"
import { Asleep, consoleEnabled, formatPorts, formatServices, formatStatus, formatLogLine, resolveRow, Unavailable, NOT_AVAILABLE } from "@/cli/work/console"
import { loadDetail, loadRows, tailLogs } from "@/cli/work/console-read"
import { DEFAULT_LOG_LINES, MAX_LOG_LINES, summarizeRow } from "@/cli/work/console-view"

const ACTIONS = {
  list: "computer_list",
  create: "computer_create",
  start: "computer_start",
  stop: "computer_stop",
  file_read: "computer_file_read",
  file_write: "computer_file_write",
} as const

type Action = keyof typeof ACTIONS

const DESCRIPTION = `Manage Allternit computers and move files on them.

Actions:
- list: your computers (optional bot_id, session_id filters).
- create: a new computer. kind is "cloud_desktop" or "local"; optional os, name, bot_id, session_id, cpu_cores, memory_mb. Creating can need the person's approval; pass approval_id when you have one.
- start / stop: start or stop computer_id.
- file_read: read path on computer_id (returns base64 content).
- file_write: write base64 content to path on computer_id.

To see the screen or use the mouse and keyboard, use the computer tool.`

export function lifecycleRequest(action: Action, params: Record<string, unknown>): { tool: string; args: Record<string, unknown> } {
  const args: Record<string, unknown> = {}
  for (const [key, value] of Object.entries(params)) {
    if (key !== "action" && value !== undefined && value !== "") args[key] = value
  }
  return { tool: ACTIONS[action], args }
}

export const ComputersTool = Tool.define("computers", async () => ({
  description: DESCRIPTION,
  parameters: z.object({
    action: z.enum(Object.keys(ACTIONS) as [Action, ...Action[]]).describe("The operation to perform."),
    computer_id: z.string().optional().describe("Target computer for start, stop, file_read and file_write."),
    kind: z.enum(["cloud_desktop", "local"]).optional().describe("Computer kind for create."),
    os: z.string().optional().describe("OS for create, e.g. linux, windows, macos."),
    name: z.string().optional().describe("Name for create."),
    bot_id: z.string().optional().describe("Owning bot for create, or a list filter."),
    session_id: z.string().optional().describe("Session to bind on create, or a list filter."),
    cpu_cores: z.number().int().optional().describe("CPU cores for create (2, 4 or 8)."),
    memory_mb: z.number().int().optional().describe("Memory for create in MB."),
    approval_id: z.string().optional().describe("Approval id from the confirmation handoff, for create."),
    path: z.string().optional().describe("File path for file_read and file_write."),
    content: z.string().optional().describe("Base64 file content for file_write."),
  }),
  async execute(params, ctx) {
    const { tool, args } = lifecycleRequest(params.action, params)
    const res = await platformFetch("POST", "/api/v1/tools/execute", { tool, args }, { signal: ctx.abort, timeoutMs: 120_000 })
    const text = await res.text()
    let body: any
    try {
      body = JSON.parse(text)
    } catch {
      body = { success: false, error: text || `HTTP ${res.status}` }
    }
    if (!res.ok || body?.success === false) {
      const reason = body?.error ?? body?.message ?? `HTTP ${res.status}`
      const approval = body?.approval_id ? ` (approval_id: ${body.approval_id})` : ""
      throw new Error(`${tool} failed: ${typeof reason === "string" ? reason : JSON.stringify(reason)}${approval}`)
    }
    const result = body?.result ?? body
    return {
      title: `computers ${params.action}`,
      output: typeof result === "string" ? result : JSON.stringify(result, null, 2),
      metadata: { action: params.action, tool },
    }
  },
}))

// ── Computers console: read-only tools ─────────────────────────────────────
// Health, services, ports and recent logs of the person's computers. These
// only read. Acting on a computer (wake, sleep, restart, snapshot, restore)
// needs the person's approval in the app, so there are no action tools here.
// Hidden unless the computers_console flag (GIZZI_COMPUTERS_CONSOLE=1) is on.

export { consoleEnabled }

async function consoleResult(title: string, run: () => Promise<string>) {
  let output: string
  try {
    output = await run()
  } catch (e) {
    if (e instanceof Unavailable) output = NOT_AVAILABLE
    else if (e instanceof Asleep) output = `${e.message}${e.lastSample ? ` Last sample: ${JSON.stringify(e.lastSample)}` : ""}`
    else throw e
  }
  return { title, output, metadata: {} }
}

const computerParam = z.string().describe("Computer name or id from computers_console_list.")

export const ComputersConsoleListTool = Tool.define("computers_console_list", async () => ({
  description:
    "List the person's computers (cloud, paired and desktop) with status, group and the last health sample, and which ones need attention. Read-only; never wakes a computer.",
  parameters: z.object({}),
  execute: () =>
    consoleResult("computers console list", async () => JSON.stringify((await loadRows()).map(summarizeRow), null, 2)),
}))

export const ComputerSnapshotTool = Tool.define("computer_snapshot", async () => ({
  description:
    "Health snapshot of one computer: OS, uptime, CPU, memory, disk. An asleep computer answers from its last sample and is not woken. Read-only.",
  parameters: z.object({ computer: computerParam }),
  execute: (p) =>
    consoleResult("computer snapshot", async () => {
      const row = await resolveRow(p.computer)
      const d = await loadDetail(row)
      return formatStatus(row, d.snapshot, d.asleep)
    }),
}))

export const ComputerServicesTool = Tool.define("computer_services", async () => ({
  description: "Services (systemd, launchd or Windows services) on one computer with state and memory. Read-only. Needs the computer to be awake.",
  parameters: z.object({ computer: computerParam }),
  execute: (p) =>
    consoleResult("computer services", async () => {
      const d = await loadDetail(await resolveRow(p.computer))
      return d.asleep ? "This computer is asleep, so its services are not available." : formatServices(d.services.slice(0, 200))
    }),
}))

export const ComputerPortsTool = Tool.define("computer_ports", async () => ({
  description: "Listening ports on one computer with protocol, exposure (loopback, lan, public) and process. Read-only. Needs the computer to be awake.",
  parameters: z.object({ computer: computerParam }),
  execute: (p) =>
    consoleResult("computer ports", async () => {
      const d = await loadDetail(await resolveRow(p.computer))
      return d.asleep ? "This computer is asleep, so its ports are not available." : formatPorts(d.ports.slice(0, 200))
    }),
}))

export const ComputerLogsTailTool = Tool.define("computer_logs_tail", async () => ({
  description: `Recent log lines of one service unit on a computer (masked on the computer before they leave it). Returns at most ${MAX_LOG_LINES} lines (default ${DEFAULT_LOG_LINES}). Read-only. Needs the computer to be awake.`,
  parameters: z.object({
    computer: computerParam,
    unit: z.string().describe("Service unit name from computer_services."),
    lines: z.number().int().optional().describe(`How many recent lines (1 to ${MAX_LOG_LINES}).`),
  }),
  execute: (p, ctx) =>
    consoleResult("computer logs", async () => {
      const lines = await tailLogs(p.computer, p.unit, p.lines, ctx.abort)
      return lines.length ? lines.map(formatLogLine).join("\n") : "No log lines."
    }),
}))

export const ComputersConsoleTools = [ComputersConsoleListTool, ComputerSnapshotTool, ComputerServicesTool, ComputerPortsTool, ComputerLogsTailTool]
