import { Hono } from "hono"
import { describeRoute, validator } from "@/runtime/server/openapi"
import { resolver } from "@/runtime/server/openapi"
import { Question } from "@/runtime/integrations/question"
import z from "zod/v4"
import { errors } from "@/runtime/server/error"
import { lazy } from "@/shared/util/lazy"

export const QuestionRoutes = lazy(() =>
  new Hono()
    .get(
      "/",
      describeRoute({
        summary: "List pending questions",
        description: "Get all pending question requests across all sessions.",
        operationId: "question.list",
        responses: {
          200: {
            description: "List of pending questions",
            content: {
              "application/json": {
                schema: resolver(z.any()),
              },
            },
          },
        },
      }),
      async (c) => {
        const questions = await Question.list()
        return c.json(questions)
      },
    )
    .post(
      "/:requestID/reply",
      describeRoute({
        summary: "Reply to question request",
        description: "Provide answers to a question request from the AI assistant.",
        operationId: "question.reply",
        responses: {
          200: {
            description: "Question answered successfully",
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
          requestID: z.string(),
        }),
      ),
      validator("json", z.any()),
      async (c) => {
        const params = c.req.valid("param")
        const json = c.req.valid("json")
        await Question.reply({
          requestID: params.requestID,
          answers: json.answers,
          plan: json.plan,
        })
        return c.json(true)
      },
    )
    .post(
      "/:requestID/plan-page",
      describeRoute({
        summary: "Link a plan question to its plan page",
        description: "Sets the http(s) link to the full plan for a pending plan-approval question.",
        operationId: "question.planPage",
        responses: {
          200: {
            description: "Whether the link was set",
            content: {
              "application/json": {
                schema: resolver(z.boolean()),
              },
            },
          },
          ...errors(400, 404),
        },
      }),
      validator(
        "param",
        z.object({
          requestID: z.string(),
        }),
      ),
      validator("json", z.object({ url: z.string() })),
      async (c) => {
        const params = c.req.valid("param")
        const json = c.req.valid("json")
        return c.json(await Question.setPlanPage(params.requestID, json.url))
      },
    )
    .post(
      "/:requestID/reject",
      describeRoute({
        summary: "Reject question request",
        description: "Reject a question request from the AI assistant.",
        operationId: "question.reject",
        responses: {
          200: {
            description: "Question rejected successfully",
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
          requestID: z.string(),
        }),
      ),
      async (c) => {
        const params = c.req.valid("param")
        await Question.reject(params.requestID)
        return c.json(true)
      },
    ),
)
