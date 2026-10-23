import { Hono } from "hono"
import z from "zod/v4"
import { describeRoute, validator, resolver } from "@/runtime/server/openapi"
import { lazy } from "@/shared/util/lazy"
import { Instance } from "@/runtime/context/project/instance"
import { RepoMap } from "@/codemap/repo-map"

const GraphFile = z.object({
  path: z.string(),
  score: z.number(),
  importers: z.number(),
  imports: z.number(),
  exports: z.array(z.string()),
})

export const CodeMapGraph = z.object({
  root: z.string(),
  /** False when the directory is not a project folder (home, filesystem root, no VCS/manifest). */
  project: z.boolean(),
  files: z.array(GraphFile),
  edges: z.array(z.object({ from: z.string(), to: z.string() })),
  total: z.number(),
  shown: z.number(),
  promptShown: z.number(),
  editedBoosted: z.number(),
  updatedAt: z.string().nullable(),
})
export type CodeMapGraph = z.infer<typeof CodeMapGraph>

/** How long the route waits for a first scan before answering with an empty (still loading) map. */
const FIRST_SCAN_WAIT_MS = 8_000

/**
 * The live code map for `directory`: top `limit` files by repo-map score and
 * the import edges among them. Loads the repo map on first use.
 */
export async function buildCodeMapGraph(input: {
  directory: string
  limit?: number
  sessionID?: string
  waitMs?: number
}): Promise<CodeMapGraph> {
  const project = await RepoMap.isProjectDir(input.directory)
  if (!project) return { ...RepoMap.graph({ root: input.directory, limit: input.limit }), project: false }
  await Promise.race([
    RepoMap.load(input.directory),
    new Promise<false>((resolve) => setTimeout(() => resolve(false), input.waitMs ?? FIRST_SCAN_WAIT_MS).unref?.()),
  ])
  return { ...RepoMap.graph({ root: input.directory, limit: input.limit, sessionID: input.sessionID }), project: true }
}

/** Live code map. Mounted at "/" (path carries /codemap). */
export const CodeMapRoutes = lazy(() =>
  new Hono().get(
    "/codemap/graph",
    describeRoute({
      summary: "Get the code map graph",
      description:
        "Top files of the project's repo map by score (import graph + session boost) and the import edges among them. Pair with codemap.activity events for live read/edit lighting.",
      operationId: "codemap.graph",
      responses: {
        200: { description: "Code map graph", content: { "application/json": { schema: resolver(CodeMapGraph) } } },
      },
    }),
    validator(
      "query",
      z.object({
        limit: z.coerce.number().int().min(1).max(RepoMap.GRAPH_MAX_LIMIT).optional(),
        sessionID: z.string().optional(),
        directory: z.string().optional(),
      }),
    ),
    async (c) => {
      const { limit, sessionID } = c.req.valid("query")
      return c.json(await buildCodeMapGraph({ directory: Instance.directory, limit, sessionID }))
    },
  ),
)
