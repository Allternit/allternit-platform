/**
 * Desktop Tool - Allternit Desktop Cloud Bridge
 *
 * Lets a GIZZI agent provision, use, and release a cloud desktop sandbox
 * through the Allternit Desktop Cloud API.
 */

import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import DESCRIPTION from "@/runtime/tools/builtins/desktop.txt"
import { Log } from "@/shared/util/log"
import { platformApiBase, platformToken } from "@/runtime/bots/platform-api"

const log = Log.create({ service: "desktop-tool" })

/**
 * The bot desktop routes live on the same Allternit API as the rest of the
 * bot platform calls (`runtime/bots/platform-api.ts`), so resolve the base the
 * same way: an explicit override, else the shared gateway base.
 */
export function desktopApiBase(env: NodeJS.ProcessEnv = process.env): string {
  const override = env.ALLTERNIT_GATEWAY_URL ?? env.VITE_ALLTERNIT_GATEWAY_URL
  return (override?.trim() || platformApiBase()).replace(/\/+$/, "")
}

const API_BASE_URL = desktopApiBase()

const API_TOKEN =
  process.env.ALLTERNIT_SELF_HOSTED_TOKEN ??
  process.env.VITE_ALLTERNIT_SELF_HOSTED_TOKEN

/** Self-hosted token when set, else the signed-in `gizzi login` credential. */
async function authHeaders(): Promise<Record<string, string>> {
  if (API_TOKEN) return { "X-Allternit-Self-Hosted-Token": API_TOKEN }
  const token = await platformToken()
  return token ? { Authorization: `Bearer ${token}` } : {}
}

type DesktopResult = {
  title: string
  output: string
  metadata: Record<string, any>
  attachments?: any[]
}

const DesktopAction = z.enum([
  "provision",
  "shell",
  "screenshot",
  "files/download",
  "files/upload",
  "mouse",
  "keyboard",
  "deprovision",
  "status",
])

async function apiCall(
  method: string,
  path: string,
  query?: Record<string, string>,
  body?: Record<string, any>,
): Promise<any> {
  const url = new URL(path, API_BASE_URL)
  if (query) {
    for (const [k, v] of Object.entries(query)) {
      if (v !== undefined && v !== "") url.searchParams.set(k, v)
    }
  }

  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    Accept: "application/json",
    ...(await authHeaders()),
  }

  log.info("Desktop Cloud API call", { method, path: url.toString() })

  const response = await fetch(url.toString(), {
    method,
    headers,
    body: body ? JSON.stringify(body) : undefined,
  })

  const text = await response.text()
  let json: any
  try {
    json = text ? JSON.parse(text) : {}
  } catch {
    json = { raw: text }
  }

  if (!response.ok) {
    throw new Error(
      `Desktop Cloud API error ${response.status}: ${json.error ?? text ?? "unknown"}`,
    )
  }
  return json
}

export const DesktopTool = Tool.define("desktop", async () => {
  return {
    description: DESCRIPTION,
    parameters: z.object({
      bot_id: z
        .string()
        .optional()
        .describe("The bot id that owns the desktop sandbox. Defaults to ALLTERNIT_BOT_ID env var."),
      action: DesktopAction.describe("The desktop operation to perform."),
      os: z
        .enum(["linux", "windows", "macos"])
        .optional()
        .describe("OS to provision. Defaults to linux."),
      template_id: z
        .string()
        .optional()
        .describe("Optional desktop template id to use when provisioning."),
      sandbox_id: z
        .string()
        .optional()
        .describe("Sandbox id returned by provision. Required for shell/screenshot/files/lifecycle actions."),
      command: z
        .array(z.string())
        .optional()
        .describe("Shell command array for action=shell."),
      path: z
        .string()
        .optional()
        .describe("File path for action=files/download or files/upload."),
      content_base64: z
        .string()
        .optional()
        .describe("Base64 file content for action=files/upload."),
      mouse_action: z
        .enum(["move", "click"])
        .optional()
        .describe("Mouse action type for action=mouse."),
      x: z.number().optional().describe("X coordinate for mouse actions."),
      y: z.number().optional().describe("Y coordinate for mouse actions."),
      button: z
        .enum(["left", "middle", "right"])
        .optional()
        .describe("Mouse button for click actions."),
      keyboard_action: z
        .enum(["type", "key"])
        .optional()
        .describe("Keyboard action for action=keyboard: type text or press a key."),
      keyboard_input: z
        .string()
        .optional()
        .describe("Text to type or key name to press for action=keyboard."),
      keys: z
        .string()
        .optional()
        .describe("Deprecated alias for keyboard_input when action=keyboard."),
    }),
    async execute(params, ctx): Promise<DesktopResult> {
      const bot_id = params.bot_id ?? process.env.ALLTERNIT_BOT_ID
      if (!bot_id) {
        throw new Error(
          "desktop tool requires bot_id parameter or ALLTERNIT_BOT_ID environment variable.",
        )
      }
      const { action, os, template_id, sandbox_id } = params

      if (action === "provision") {
        const query: Record<string, string> = {}
        if (os) query.os = os
        if (template_id) query.template_id = template_id
        const result = await apiCall("POST", `/api/v1/bots/${bot_id}/desktop/provision`, query)
        return {
          title: "Desktop provisioned",
          output: `Provisioned desktop for bot ${bot_id}.\nSandbox: ${result.sandbox_id}\nStatus: ${result.status}\nProvider: ${result.provider}\nHost: ${result.host ?? "none"}`,
          metadata: { action, result },
        }
      }

      if (!sandbox_id) {
        throw new Error(`action=${action} requires a sandbox_id. Call action=provision first.`)
      }

      if (action === "status") {
        const result = await apiCall("GET", `/api/v1/bots/${bot_id}/desktop`, { sandbox_id })
        return {
          title: "Desktop status",
          output: JSON.stringify(result, null, 2),
          metadata: { action, result },
        }
      }

      if (action === "shell") {
        if (!params.command || params.command.length === 0) {
          throw new Error("action=shell requires a non-empty command array.")
        }
        const result = await apiCall(
          "POST",
          `/api/v1/bots/${bot_id}/desktop/shell`,
          { sandbox_id },
          { command: params.command },
        )
        return {
          title: "Shell executed",
          output: `Exit code: ${result.exit_code}\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
          metadata: { action, result },
        }
      }

      if (action === "screenshot") {
        const url = new URL(`/api/v1/bots/${bot_id}/desktop/screenshot`, API_BASE_URL)
        url.searchParams.set("sandbox_id", sandbox_id)
        const headers = await authHeaders()
        const response = await fetch(url.toString(), { headers })
        if (!response.ok) {
          const text = await response.text()
          throw new Error(`Screenshot failed: ${response.status} ${text}`)
        }
        const buffer = Buffer.from(await response.arrayBuffer())
        const base64 = buffer.toString("base64")
        return {
          title: "Desktop screenshot",
          output: `Screenshot captured (${buffer.length} bytes). The image is attached.`,
          metadata: { action, size_bytes: buffer.length },
          attachments: [
            {
              type: "file",
              mime: "image/png",
              url: `data:image/png;base64,${base64}`,
            } as any,
          ],
        }
      }

      if (action === "files/download") {
        if (!params.path) throw new Error("action=files/download requires path.")
        const url = new URL(`/api/v1/bots/${bot_id}/desktop/files/download`, API_BASE_URL)
        url.searchParams.set("sandbox_id", sandbox_id)
        url.searchParams.set("path", params.path)
        const headers = await authHeaders()
        const response = await fetch(url.toString(), { headers })
        if (!response.ok) {
          const text = await response.text()
          throw new Error(`File download failed: ${response.status} ${text}`)
        }
        const buffer = Buffer.from(await response.arrayBuffer())
        const base64 = buffer.toString("base64")
        return {
          title: "File downloaded",
          output: `Downloaded ${params.path} (${buffer.length} bytes). The file is attached.`,
          metadata: { action, path: params.path, size_bytes: buffer.length },
          attachments: [
            {
              type: "file",
              mime: "application/octet-stream",
              url: `data:application/octet-stream;base64,${base64}`,
            } as any,
          ],
        }
      }

      if (action === "files/upload") {
        if (!params.path || !params.content_base64) {
          throw new Error("action=files/upload requires path and content_base64.")
        }
        const result = await apiCall(
          "POST",
          `/api/v1/bots/${bot_id}/desktop/files/upload`,
          { sandbox_id, path: params.path },
          { content_base64: params.content_base64 },
        )
        return {
          title: "File uploaded",
          output: `Uploaded ${params.path}.`,
          metadata: { action, result },
        }
      }

      if (action === "mouse") {
        if (!params.mouse_action) throw new Error("action=mouse requires mouse_action.")
        const body: Record<string, any> = { action: params.mouse_action }
        if (params.x !== undefined) body.x = params.x
        if (params.y !== undefined) body.y = params.y
        if (params.button) body.button = params.button
        const result = await apiCall(
          "POST",
          `/api/v1/bots/${bot_id}/desktop/mouse`,
          { sandbox_id },
          body,
        )
        return {
          title: "Mouse action sent",
          output: JSON.stringify(result, null, 2),
          metadata: { action, result },
        }
      }

      if (action === "keyboard") {
        const kbAction = params.keyboard_action ?? "type"
        const kbInput = params.keyboard_input ?? params.keys
        if (!kbInput) throw new Error("action=keyboard requires keyboard_input (or keys).")
        const result = await apiCall(
          "POST",
          `/api/v1/bots/${bot_id}/desktop/keyboard`,
          { sandbox_id },
          { action: kbAction, [kbAction === "type" ? "text" : "key"]: kbInput },
        )
        return {
          title: "Keyboard input sent",
          output: JSON.stringify(result, null, 2),
          metadata: { action, result },
        }
      }

      if (action === "deprovision") {
        const result = await apiCall("POST", `/api/v1/bots/${bot_id}/desktop/deprovision`, { sandbox_id })
        return {
          title: "Desktop deprovisioned",
          output: `Deprovisioned sandbox ${sandbox_id} for bot ${bot_id}.`,
          metadata: { action, result },
        }
      }

      throw new Error(`Unsupported desktop action: ${action}`)
    },
  }
})
