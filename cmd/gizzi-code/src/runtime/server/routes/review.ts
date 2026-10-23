import { Hono } from "hono"
import z from "zod/v4"
import { describeRoute, validator, resolver } from "@/runtime/server/openapi"
import { errors } from "@/runtime/server/error"
import { lazy } from "@/shared/util/lazy"
import { Log } from "@/shared/util/log"
import { Instance } from "@/runtime/context/project/instance"
import { ModelJobs } from "@/runtime/context/config/model-jobs"
import { Reviewer } from "@/runtime/review"
import { ReviewLoop } from "@/runtime/review/loop"
import { ReviewVerdict } from "@/runtime/review/verdict"

const log = Log.create({ service: "server.routes.review" })

const ReviewBody = z.object({
  instruction: z.string().optional().describe("What to check first"),
  untilPass: z.boolean().optional().describe("Run review → fix → re-review until PASS"),
  maxRounds: z.number().int().min(1).max(ReviewLoop.MAX_ROUNDS).optional(),
  wait: z.boolean().optional().describe("With untilPass: wait for the loop instead of returning 202"),
  model: z.string().optional().describe("Override the review model (provider/model)"),
})

function modelRef(model?: string) {
  if (!model) return undefined
  const [providerID, ...rest] = model.split("/")
  return providerID && rest.length ? { providerID, modelID: rest.join("/") } : undefined
}

const verdictResponse = {
  200: {
    description: "Review verdict (null when there was nothing to review)",
    content: { "application/json": { schema: resolver(z.object({ verdict: ReviewVerdict.Stored.nullable() })) } },
  },
}

/** Review verdict + per-job model routes. Mounted at "/" (paths carry /session and /config). */
export const ReviewRoutes = lazy(() =>
  new Hono()
    .get(
      "/config/model-jobs",
      describeRoute({
        summary: "Get per-job models",
        description: "Models configured per job (models.jobs). An unset job uses the session model.",
        operationId: "config.modelJobs.get",
        responses: {
          200: {
            description: "Per-job models",
            content: {
              "application/json": {
                schema: resolver(z.object({ jobs: ModelJobs.Jobs, available: z.array(ModelJobs.Job) })),
              },
            },
          },
        },
      }),
      async (c) => c.json({ jobs: await ModelJobs.get(), available: [...ModelJobs.JOBS] }),
    )
    .put(
      "/config/model-jobs",
      describeRoute({
        summary: "Set per-job models",
        description: "Replace the project's models.jobs. A job left out is cleared.",
        operationId: "config.modelJobs.set",
        responses: {
          200: {
            description: "Saved per-job models",
            content: { "application/json": { schema: resolver(z.object({ jobs: ModelJobs.Jobs })) } },
          },
          ...errors(400),
        },
      }),
      validator("json", z.object({ jobs: ModelJobs.Jobs })),
      async (c) => {
        const { jobs } = c.req.valid("json")
        return c.json({ jobs: await ModelJobs.set(jobs) })
      },
    )
    .post(
      "/review",
      describeRoute({
        summary: "Review uncommitted changes",
        description: "Run the independent reviewer on the project's uncommitted git changes (no session).",
        operationId: "review.run",
        responses: { ...verdictResponse, ...errors(400) },
      }),
      validator("json", ReviewBody.pick({ instruction: true, model: true })),
      async (c) => {
        const body = c.req.valid("json")
        const verdict = await Reviewer.runReview({
          cwd: Instance.directory,
          instruction: body.instruction,
          model: modelRef(body.model),
        })
        return c.json({ verdict })
      },
    )
    .post(
      "/session/:sessionID/review",
      describeRoute({
        summary: "Review session changes",
        description:
          "Run the independent reviewer on what this session changed (else uncommitted git changes). With untilPass, loop review → fix → re-review; progress is published as review.* events and the call returns 202 unless wait is true.",
        operationId: "session.review",
        responses: {
          ...verdictResponse,
          202: { description: "Review loop started; follow review.* events" },
          ...errors(400, 404),
        },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      validator("json", ReviewBody),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        const body = c.req.valid("json")
        if (!body.untilPass) {
          const verdict = await Reviewer.runReview({
            sessionID,
            cwd: Instance.directory,
            instruction: body.instruction,
            model: modelRef(body.model),
          })
          return c.json({ verdict })
        }
        const run = ReviewLoop.reviewUntilPass({
          sessionID,
          cwd: Instance.directory,
          maxRounds: body.maxRounds,
          instruction: body.instruction,
        })
        if (body.wait) return c.json(await run)
        run.catch((error) => log.error("review loop failed", { sessionID, error }))
        return c.json({ started: true, sessionID, maxRounds: ReviewLoop.clampRounds(body.maxRounds) }, 202)
      },
    )
    .delete(
      "/session/:sessionID/review",
      describeRoute({
        summary: "Stop a review loop",
        description: "Abort the review → fix loop running for this session.",
        operationId: "session.review.cancel",
        responses: {
          200: {
            description: "Whether a loop was running",
            content: { "application/json": { schema: resolver(z.object({ cancelled: z.boolean() })) } },
          },
        },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        const cancelled = ReviewLoop.running(sessionID)
        ReviewLoop.cancel(sessionID)
        return c.json({ cancelled })
      },
    )
    .get(
      "/session/:sessionID/reviews",
      describeRoute({
        summary: "List session reviews",
        description: "The last 8 review verdicts for this session, newest first.",
        operationId: "session.reviews",
        responses: {
          200: {
            description: "Verdicts",
            content: { "application/json": { schema: resolver(z.array(ReviewVerdict.Stored)) } },
          },
        },
      }),
      validator("param", z.object({ sessionID: z.string() })),
      async (c) => {
        const { sessionID } = c.req.valid("param")
        return c.json(await Reviewer.listReviews(sessionID))
      },
    ),
)
