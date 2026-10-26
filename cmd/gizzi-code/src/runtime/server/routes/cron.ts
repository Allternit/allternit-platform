import { Hono } from "hono"
import { describeRoute, validator, resolver } from "@/runtime/server/openapi"
import z from "zod/v4"
import { CronService } from "@/runtime/automation/cron/service"

import { errors } from "@/runtime/server/error"
import { lazy } from "@/shared/util/lazy"

export const CronRoutes = lazy(() =>
  new Hono()
    .get(
      "/status",
      describeRoute({
        summary: "Get cron status",
        description: "Get overall cron service status",
        operationId: "cron.status",
        responses: {
          200: {
            description: "Cron status",
            content: {
              "application/json": {
                schema: resolver(
                  z.object({
                    jobs: z.number(),
                    active: z.number(),
                    pendingRuns: z.number(),
                    runningRuns: z.number(),
                  }),
                ),
              },
            },
          },
        },
      }),
      async (c) => {
        const status = CronService.getStatus()
        return c.json(status)
      },
    )
    .get(
      "/jobs",
      describeRoute({
        summary: "List cron jobs",
        description: "Get all cron jobs",
        operationId: "cron.list",
        responses: {
          200: {
            description: "List of cron jobs",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
        },
      }),
      async (c) => {
        const jobs = CronService.list()
        return c.json(jobs)
      },
    )
    .post(
      "/jobs",
      describeRoute({
        summary: "Create cron job",
        description: "Create a new scheduled job",
        operationId: "cron.create",
        responses: {
          201: {
            description: "Cron job created",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(400),
        },
      }),
      validator("json", z.any()),
      async (c) => {
        const input = c.req.valid("json")
        const job = CronService.create(input)
        return c.json(job, 201)
      },
    )
    .get(
      "/jobs/:id",
      describeRoute({
        summary: "Get cron job",
        description: "Get details of a specific cron job",
        operationId: "cron.get",
        responses: {
          200: {
            description: "Cron job details",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator(
        "param",
        z.object({
          id: z.string(),
        }),
      ),
      async (c) => {
        const { id } = c.req.valid("param")
        const job = CronService.get(id)
        if (!job) {
          return c.json({ error: `Cron job "${id}" not found` }, 404)
        }
        return c.json(job)
      },
    )
    .put(
      "/jobs/:id",
      describeRoute({
        summary: "Update cron job",
        description: "Update an existing cron job",
        operationId: "cron.update",
        responses: {
          200: {
            description: "Cron job updated",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(400, 404),
        },
      }),
      validator(
        "param",
        z.object({
          id: z.string(),
        }),
      ),
      validator("json", z.any()),
      async (c) => {
        const { id } = c.req.valid("param")
        const input = c.req.valid("json")
        const job = CronService.update(id, input)
        return c.json(job)
      },
    )
    .delete(
      "/jobs/:id",
      describeRoute({
        summary: "Delete cron job",
        description: "Delete a cron job",
        operationId: "cron.delete",
        responses: {
          204: {
            description: "Cron job deleted",
          },
          ...errors(404),
        },
      }),
      validator(
        "param",
        z.object({
          id: z.string(),
        }),
      ),
      async (c) => {
        const { id } = c.req.valid("param")
        CronService.delete(id)
        return c.body(null, 204)
      },
    )
    .post(
      "/jobs/:id/pause",
      describeRoute({
        summary: "Pause cron job",
        description: "Pause a cron job temporarily",
        operationId: "cron.pause",
        responses: {
          200: {
            description: "Cron job paused",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator(
        "param",
        z.object({
          id: z.string(),
        }),
      ),
      async (c) => {
        const { id } = c.req.valid("param")
        CronService.pause(id)
        const job = CronService.get(id)
        return c.json(job)
      },
    )
    .post(
      "/jobs/:id/resume",
      describeRoute({
        summary: "Resume cron job",
        description: "Resume a paused cron job",
        operationId: "cron.resume",
        responses: {
          200: {
            description: "Cron job resumed",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator(
        "param",
        z.object({
          id: z.string(),
        }),
      ),
      async (c) => {
        const { id } = c.req.valid("param")
        CronService.resume(id)
        const job = CronService.get(id)
        return c.json(job)
      },
    )
    .post(
      "/jobs/:id/abort",
      describeRoute({
        summary: "Stop cron job run",
        description: "Stop the job's current run. It ends cancelled with the output it had and is not retried.",
        operationId: "cron.abort",
        responses: {
          200: {
            description: "Whether a run was stopped",
            content: {
              "application/json": {
                schema: resolver(z.object({ stopped: z.boolean() })),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator(
        "param",
        z.object({
          id: z.string(),
        }),
      ),
      async (c) => {
        const { id } = c.req.valid("param")
        return c.json({ stopped: CronService.abort(id) })
      },
    )
    .post(
      "/jobs/:id/run",
      describeRoute({
        summary: "Trigger cron job",
        description: "Manually trigger a cron job to run immediately",
        operationId: "cron.run",
        responses: {
          200: {
            description: "Cron job triggered",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator(
        "param",
        z.object({
          id: z.string(),
        }),
      ),
      async (c) => {
        const { id } = c.req.valid("param")
        const run = await CronService.run(id, "manual")
        return c.json(run)
      },
    )
    .get(
      "/jobs/:id/runs",
      describeRoute({
        summary: "List job runs",
        description: "Get run history for a specific job",
        operationId: "cron.runs",
        responses: {
          200: {
            description: "List of runs",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
        },
      }),
      validator(
        "param",
        z.object({
          id: z.string(),
        }),
      ),
      async (c) => {
        const { id } = c.req.valid("param")
        const runs = CronService.getRuns(id)
        return c.json(runs)
      },
    )
    .get(
      "/runs",
      describeRoute({
        summary: "List all runs",
        description: "Get all cron runs across all jobs",
        operationId: "cron.allRuns",
        responses: {
          200: {
            description: "List of all runs",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
        },
      }),
      async (c) => {
        const runs = CronService.getRecentRuns()
        return c.json(runs)
      },
    )
    .get(
      "/runs/:id",
      describeRoute({
        summary: "Get run",
        description: "Get details of a specific run",
        operationId: "cron.getRun",
        responses: {
          200: {
            description: "Run details",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
          ...errors(404),
        },
      }),
      validator(
        "param",
        z.object({
          id: z.string(),
        }),
      ),
      async (c) => {
        const { id } = c.req.valid("param")
        const run = CronService.getRun(id)
        if (!run) {
          return c.json({ error: `Run "${id}" not found` }, 404)
        }
        return c.json(run)
      },
    )
    .post(
      "/wake",
      describeRoute({
        summary: "Wake cron service",
        description: "Trigger due jobs immediately",
        operationId: "cron.wake",
        responses: {
          200: {
            description: "Wake completed",
            content: {
              "application/json": {
                schema: resolver(
                  z.object({
                    triggered: z.number(),
                    jobs: z.string().array(),
                  }),
                ),
              },
            },
          },
        },
      }),
      async (c) => {
        const result = CronService.wake()
        return c.json(result)
      },
    )
    .post(
      "/thread/:threadId/close",
      describeRoute({
        summary: "Stop a closed thread's jobs",
        description:
          "Thread close-out (allternit-api thread_closeout.rs): delete (remove=true) or pause the thread sessions' loops and pause its tied persistent jobs.",
        operationId: "cron.closeThread",
        responses: {
          200: {
            description: "Thread jobs stopped",
            content: { "application/json": { schema: resolver(z.object({ deleted: z.number(), paused: z.number() })) } },
          },
        },
      }),
      validator("param", z.object({ threadId: z.string() })),
      validator(
        "json",
        z.object({
          sessionIds: z.array(z.string()).optional(),
          jobIds: z.array(z.string()).optional(),
          remove: z.boolean().optional(),
        }),
      ),
      async (c) => {
        const { threadId } = c.req.valid("param")
        return c.json(CronService.closeThreadJobs(threadId, c.req.valid("json")))
      },
    )
    .post(
      "/thread/:threadId/reopen",
      describeRoute({
        summary: "Restart a reopened thread's jobs",
        description: "Re-enable the jobs a thread close-out paused.",
        operationId: "cron.reopenThread",
        responses: {
          200: {
            description: "Thread jobs resumed",
            content: { "application/json": { schema: resolver(z.object({ resumed: z.number() })) } },
          },
        },
      }),
      validator("param", z.object({ threadId: z.string() })),
      validator("json", z.object({ jobIds: z.array(z.string()).optional() })),
      async (c) => {
        const { threadId } = c.req.valid("param")
        return c.json(CronService.reopenThreadJobs(threadId, c.req.valid("json")))
      },
    )
    .delete(
      "/session/:sessionId",
      describeRoute({
        summary: "Cleanup session loops",
        description: "Delete all session-scoped cron jobs for a session (called on session close).",
        operationId: "cron.cleanupSession",
        responses: {
          200: {
            description: "Session jobs cleaned up",
            content: {
              "application/json": {
                schema: resolver(z.object({ deleted: z.number() })),
              },
            },
          },
        },
      }),
      validator("param", z.object({ sessionId: z.string() })),
      async (c) => {
        const { sessionId } = c.req.valid("param")
        const deleted = CronService.cleanupSessionJobs(sessionId)
        return c.json({ deleted })
      },
    ),
)
