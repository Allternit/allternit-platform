import { Hono } from "hono"
import path from "path"
import z from "zod/v4"
import { describeRoute, validator, resolver } from "@/runtime/server/openapi"
import { lazy } from "@/shared/util/lazy"
import { Instance } from "@/runtime/context/project/instance"
import { Session } from "@/runtime/session"
import { FileEditors } from "@/runtime/session/file-editors"

const Other = z.object({ sessionID: z.string(), title: z.string(), lastEditAt: z.number() })
const Response = z.object({
  files: z.array(z.object({ path: z.string(), otherSessions: z.array(Other) })),
  edited: z.array(z.string()),
})

/** Show a path relative to the project when it is inside it. */
function display(file: string) {
  const root = Instance.worktree
  if (!root || root === "/") return file
  const rel = path.relative(root, file)
  return rel && !rel.startsWith("..") && !path.isAbsolute(rel) ? rel : file
}

async function titleOf(sessionID: string) {
  try {
    return (await Session.get(sessionID)).title
  } catch {
    return "Another session"
  }
}

/** Files a session edited and which other sessions are editing them too. Mounted at "/". */
export const FileEditorsRoutes = lazy(() =>
  new Hono().get(
    "/file-editors",
    describeRoute({
      summary: "Files a session is editing",
      description:
        "The session's edited files (last 30 minutes) and, per file, other sessions that edited it in the last 10 minutes. Writes to one file are serialized, so contended edits wait their turn.",
      operationId: "file.editors",
      responses: {
        200: { description: "Edited files and other editors", content: { "application/json": { schema: resolver(Response) } } },
      },
    }),
    validator("query", z.object({ sessionID: z.string() })),
    async (c) => {
      const { sessionID } = c.req.valid("query")
      const state = FileEditors.forSession(sessionID)
      const titles = new Map<string, string>()
      for (const f of state.files)
        for (const o of f.otherSessions) if (!titles.has(o.sessionID)) titles.set(o.sessionID, await titleOf(o.sessionID))
      return c.json({
        edited: state.edited.map(display),
        files: state.files.map((f) => ({
          path: display(f.path),
          otherSessions: f.otherSessions.map((o) => ({ ...o, title: titles.get(o.sessionID) ?? "Another session" })),
        })),
      })
    },
  ),
)
