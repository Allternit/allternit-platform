/**
 * folder_access: the person grants folders outside the project for this
 * session (Wave C decision 10; contract `folder_access` in PLAN-wave-c).
 *
 * - request: the external_directory ask is skipped inside the folders and
 *   the session's sandbox may write there. Deletes there move to
 *   `<folder>/_to_delete/` (bash.ts trashRedirect).
 * - request_delete: deletes in the folders are real.
 *
 * Every call asks: `folder_access` is an always-ask class (registered below
 * into PermissionNext.ALWAYS_ASK), so no mode, earlier answer or task grant
 * approves it. Grants live in FolderGrants (external-directory.ts), in memory,
 * keyed by the root session.
 */
import fs from "fs"
import os from "os"
import path from "path"
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Instance } from "@/runtime/context/project/instance"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { FolderGrants } from "@/runtime/tools/builtins/external-directory"
import DESCRIPTION from "@/runtime/tools/builtins/folder-access.txt"

export const FOLDER_ACCESS_PERMISSION = "folder_access"
PermissionNext.ALWAYS_ASK.add(FOLDER_ACCESS_PERMISSION)

export const parameters = z.object({
  action: z.enum(["request", "request_delete"]).describe("request: read and write; request_delete: also real deletes"),
  paths: z.array(z.string().min(1)).min(1).max(5).describe("1 to 5 folders outside the project"),
  reason: z.string().trim().min(1).max(200).describe("What you will do there, shown to the person"),
})

export type Metadata = {
  action: "request" | "request_delete"
  granted: string[]
  /** Paths already inside the project (no grant needed). */
  inside: string[]
}

/** Resolve and check the requested folders. Throws with a message the model can act on. */
export function resolveFolders(paths: string[]): { outside: string[]; inside: string[] } {
  const outside: string[] = []
  const inside: string[] = []
  const home = (() => {
    try {
      return fs.realpathSync(os.homedir())
    } catch {
      return os.homedir()
    }
  })()
  for (const raw of paths) {
    const abs = FolderGrants.absolute(raw.trim(), Instance.directory)
    let real: string
    try {
      real = fs.realpathSync(abs)
    } catch {
      throw new Error(`${raw} doesn't exist. Name an existing folder.`)
    }
    if (!fs.statSync(real).isDirectory()) throw new Error(`${raw} is a file. Name the folder that holds it.`)
    if (real === path.parse(real).root || real === home) {
      throw new Error(`${raw} is too broad (the filesystem root or the home folder). Name a narrower folder.`)
    }
    const list = Instance.containsPath(real) ? inside : outside
    if (!list.includes(real)) list.push(real)
  }
  return { outside, inside }
}

export const FolderAccessTool = Tool.define<typeof parameters, Metadata>("folder_access", {
  description: DESCRIPTION,
  parameters,
  async execute(params, ctx) {
    const { outside, inside } = resolveFolders(params.paths)
    const action = params.action
    if (!outside.length) {
      return {
        title: "Already in the project",
        output: JSON.stringify({ granted: [], inside, note: "These folders are inside the project; no access is needed." }),
        metadata: { action, granted: [], inside },
      }
    }
    const reason = params.reason.trim()
    const verb = action === "request_delete" ? "Allow deleting files in" : "Allow access to"
    await ctx.ask({
      permission: FOLDER_ACCESS_PERMISSION,
      patterns: outside,
      always: [],
      metadata: {
        summary: `${verb} ${outside.map((p) => path.basename(p) || p).join(", ")}`,
        folderAccess: { action, paths: outside, reason, scope: "session" },
      },
    })
    for (const root of outside) {
      FolderGrants.grant(ctx.sessionID, root, { delete: action === "request_delete", reason })
      FolderGrants.allowSandboxWrites(ctx.sessionID, root)
    }
    const note =
      action === "request_delete"
        ? "Deletes in these folders are real for the rest of this session."
        : `Deletes in these folders move files to ${FolderGrants.TRASH_DIR}/ inside the folder. Use request_delete only if the person wants files gone for good.`
    return {
      title: action === "request_delete" ? "Delete access granted" : "Folder access granted",
      output: JSON.stringify({ granted: outside, ...(inside.length ? { inside } : {}), note }),
      metadata: { action, granted: outside, inside },
    }
  },
})
