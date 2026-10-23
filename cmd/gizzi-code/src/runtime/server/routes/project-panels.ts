import { Hono } from "hono"
import z from "zod/v4"
import { describeRoute, validator, resolver } from "@/runtime/server/openapi"
import { lazy } from "@/shared/util/lazy"
import { Instance } from "@/runtime/context/project/instance"
import { ProjectPanels } from "@/runtime/project/panels"

const FileBody = z.object({ file: z.string().min(1).max(120) })

async function root() {
  return ProjectPanels.findRoot(Instance.directory, Instance.worktree)
}

/**
 * Project panels (`.allternit/panels/*.json`) for the request's directory:
 * list them, approve or revoke a file's agent rules (per file content hash),
 * and read a panel's URL feeds. Mounted at "/" (paths carry /project-panels).
 */
export const ProjectPanelsRoutes = lazy(() =>
  new Hono()
    .get(
      "/project-panels",
      describeRoute({
        summary: "List project panels",
        description:
          "Panel files in the project's .allternit/panels folder: raw content, sha256, schema problems, agent rules and whether this exact content is approved.",
        operationId: "projectPanels.list",
        responses: {
          200: { description: "Panels", content: { "application/json": { schema: resolver(ProjectPanels.Listing) } } },
        },
      }),
      validator("query", z.object({ directory: z.string().optional() })),
      async (c) => c.json(await ProjectPanels.list(await root())),
    )
    .post(
      "/project-panels/approve",
      describeRoute({
        summary: "Approve a project panel's agent rules",
        description:
          "Approves the file's agent instructions, guards and signed feeds for exactly the content with this sha256. A changed file answers 409 and needs approving again.",
        operationId: "projectPanels.approve",
        responses: {
          200: { description: "Approved", content: { "application/json": { schema: resolver(z.object({ approved: z.literal(true) })) } } },
          404: { description: "No such panel file" },
          409: { description: "The file changed since it was listed" },
        },
      }),
      validator("json", FileBody.extend({ sha256: z.string().regex(/^[a-f0-9]{64}$/) })),
      async (c) => {
        const { file, sha256 } = c.req.valid("json")
        try {
          await ProjectPanels.approve(await root(), file, sha256)
          return c.json({ approved: true as const })
        } catch (e) {
          if (e instanceof ProjectPanels.StaleError) return c.json({ error: e.message }, 409)
          if (e instanceof ProjectPanels.NotFoundError) return c.json({ error: e.message }, 404)
          throw e
        }
      },
    )
    .post(
      "/project-panels/revoke",
      describeRoute({
        summary: "Turn off a project panel's agent rules",
        operationId: "projectPanels.revoke",
        responses: { 200: { description: "Revoked", content: { "application/json": { schema: resolver(z.object({ approved: z.literal(false) })) } } } },
      }),
      validator("json", FileBody),
      async (c) => {
        const { file } = c.req.valid("json")
        await ProjectPanels.revoke(await root(), file)
        return c.json({ approved: false as const })
      },
    )
    .get(
      "/project-panels/source",
      describeRoute({
        summary: "Read a project panel's URL feed",
        description:
          "Fetches one URL source of a panel file as JSON. The URL comes from the file; a connection-signed source needs the file approved.",
        operationId: "projectPanels.source",
        responses: {
          200: { description: "The feed", content: { "application/json": { schema: resolver(z.object({ data: z.unknown() })) } } },
        },
      }),
      validator("query", z.object({ file: z.string(), source: z.string(), directory: z.string().optional() })),
      async (c) => {
        const { file, source } = c.req.valid("query")
        try {
          return c.json({ data: await ProjectPanels.fetchSource(await root(), file, source) })
        } catch (e) {
          if (e instanceof ProjectPanels.SourceError) return c.json({ error: e.message }, e.status)
          throw e
        }
      },
    ),
)
