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
