import { Hono } from "hono"
import fs from "fs/promises"
import os from "os"
import path from "path"
import z from "zod/v4"
import { validator } from "@/runtime/server/openapi"
import { Instance } from "@/runtime/context/project/instance"
import { InstanceBootstrap } from "@/runtime/context/project/bootstrap"
import { Project } from "@/runtime/context/project/project"
import { Flag } from "@/runtime/context/flag/flag"
import { Session } from "@/runtime/session"
import { SessionPrompt } from "@/runtime/session/prompt"
import { SessionTable } from "@/runtime/session/session.sql"
import { Database, desc } from "@/runtime/session/storage/db"
import { Storage } from "@/runtime/session/storage/storage"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { Log } from "@/shared/util/log"

/**
 * Start anywhere: one call that starts a session in a folder on this computer
 * (contract: docs/contracts/session-start/). The engine kind only: runtime
 * `allternit` becomes a session whose `directory` is the folder. Terminal kinds
 * are served by allternit-api; here they answer 404 runtime_missing.
 *
 * Reached on this computer as /v1/session-start and, so the cloud relay's
 * `/api` allowlist carries it, as /api/v1/session-start.
 */
const log = Log.create({ service: "session-start" })

const MAX_FOLDERS = 30
const MAX_DEPTH = 3

/** The app's mode ids; each maps to the engine's own permission mode. */
export const MODE_TO_ENGINE = {
  auto: "acceptEdits",
  default: "default",
  plan: "plan",
  bypass: "bypassPermissions",
} as const satisfies Record<string, PermissionNext.Mode>
type AppMode = keyof typeof MODE_TO_ENGINE
const APP_MODES = Object.keys(MODE_TO_ENGINE) as AppMode[]

/** Engine permission mode back to the app's ids (for the device default). */
export function appModeOf(engine: string | undefined): AppMode {
  switch (engine) {
    case "acceptEdits":
    case "auto":
      return "auto"
    case "plan":
      return "plan"
    case "bypassPermissions":
    case "yolo":
      return "bypass"
    default:
      return "default"
  }
}

const ERRORS = {
  mode_not_allowed: {
    status: 403,
    message: "This permission mode is higher than the one you are allowed to grant.",
    action: "Pick a mode at or below your own, or confirm bypass on the computer.",
  },
  depth_exceeded: {
    status: 403,
    message: "Sessions started from sessions can only go three levels deep.",
    action: "Start the work from a top-level session instead.",
  },
  folder_missing: {
    status: 404,
    message: "That folder no longer exists on this computer.",
    action: "Pick another folder from the list.",
  },
  runtime_missing: {
    status: 404,
    message: "That coding tool is not installed on this computer.",
    action: "Start it from Allternit Desktop or a computer with the terminal service.",
  },
} as const

function fail(c: any, code: keyof typeof ERRORS) {
  const { status, ...rest } = ERRORS[code]
  return c.json({ error: code, ...rest }, status)
}

async function isDirectory(p: string) {
  try {
    return (await fs.stat(p)).isDirectory()
  } catch {
    return false
  }
}

function deviceDefaultMode(): AppMode {
  return appModeOf(Flag.GIZZI_PERMISSION_MODE)
}

const StartRequest = z.object({
  folder: z.object({ path: z.string().min(1) }),
  runtime: z.string().min(1),
  mode: z.string().optional(),
  confirmBypass: z.boolean().optional(),
  prompt: z.string().optional(),
  origin: z.object({
    surface: z.string(),
    deviceName: z.string().optional(),
    parentSessionId: z.string().optional(),
    depth: z.number().int().min(0).optional(),
  }),
})

export type SessionOrigin = z.infer<typeof StartRequest>["origin"]

/** Where a session's origin is kept: next to the session, keyed by its id. */
export const originKey = (sessionID: string) => ["session_origin", sessionID]

async function folders() {
  const seen = new Map<string, number>()
  const note = (dir: string | undefined, at: number) => {
    if (!dir || dir === "/") return
    if (!seen.has(dir) || seen.get(dir)! < at) seen.set(dir, at)
  }
  for (const project of Project.list()) note(project.worktree, project.time.updated)
  const rows = Database.use((db) =>
    db
      .select({ directory: SessionTable.directory, at: SessionTable.time_updated })
      .from(SessionTable)
      .orderBy(desc(SessionTable.time_updated))
      .limit(500)
      .all(),
  )
  for (const row of rows) note(row.directory, row.at)
  const newest = [...seen.entries()].sort((a, b) => b[1] - a[1])
  const out: { path: string; name: string; lastUsedAt?: string }[] = []
  const home = os.homedir()
  for (const [dir, at] of newest) {
    if (out.length >= MAX_FOLDERS - 1) break
    if (dir === home || !(await isDirectory(dir))) continue
    out.push({ path: dir, name: path.basename(dir) || dir, lastUsedAt: new Date(at).toISOString() })
  }
  out.push({ path: home, name: path.basename(home) || home })
  return out
}

export const SessionStartRoutes = () =>
  new Hono()
    .get("/options", async (c) => {
      return c.json({
        device: {
          runtimeId: process.env["ALLTERNIT_RUNTIME_ID"] ?? "",
          name: os.hostname(),
          os: process.platform === "darwin" ? "macos" : process.platform === "win32" ? "windows" : "linux",
          remoteStart: true,
        },
        folders: await folders(),
        modes: { allternit: { default: deviceDefaultMode(), supported: APP_MODES } },
        defaultRuntime: "allternit",
      })
    })
    .post("/", validator("json", StartRequest), async (c) => {
      const body = c.req.valid("json")
      if (body.runtime !== "allternit") return fail(c, "runtime_missing")

      const mode = (body.mode ?? deviceDefaultMode()) as string
      if (!(APP_MODES as string[]).includes(mode)) return fail(c, "mode_not_allowed")
      // Bypass from another device needs the device default to be bypass or an explicit confirm.
      if (mode === "bypass" && deviceDefaultMode() !== "bypass" && !body.confirmBypass) return fail(c, "mode_not_allowed")

      const depth = body.origin.depth ?? 0
      if (depth > MAX_DEPTH) return fail(c, "depth_exceeded")

      const directory = path.resolve(body.folder.path)
      if (!(await isDirectory(directory))) return fail(c, "folder_missing")

      const prompt = body.prompt?.trim()
      const title = prompt ? prompt.split("\n")[0].slice(0, 80) : undefined

      const session = await Instance.provide({
        directory,
        init: InstanceBootstrap,
        async fn() {
          const created = await Session.create({ title, surface: "code" })
          await PermissionNext.setMode(created.id, MODE_TO_ENGINE[mode as AppMode])
          await Storage.write(originKey(created.id), { ...body.origin, depth, mode, startedAt: Date.now() })
          if (prompt) {
            // The turn runs on its own; the caller follows it on the session's event stream.
            void SessionPrompt.prompt({ sessionID: created.id, parts: [{ type: "text", text: prompt }] }).catch((err) =>
              log.error("first prompt failed", { sessionID: created.id, error: String(err) }),
            )
          }
          return created
        },
      })
      return c.json({ kind: "engine", sessionId: session.id, directory: session.directory }, 201)
    })
