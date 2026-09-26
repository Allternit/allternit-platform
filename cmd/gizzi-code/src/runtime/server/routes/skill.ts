import { Hono } from "hono"
import { describeRoute, resolver, validator } from "@/runtime/server/openapi"
import z from "zod/v4"
import { Skill } from "@/runtime/skills/skill"
import { Discovery } from "@/runtime/skills/discovery"
import { Evaluator } from "@/runtime/skills/evaluator"
import { EvalStore } from "@/runtime/skills/eval-store"
import { errors } from "@/runtime/server/error"

const EvalCriteriaSchema = z.object({
  id: z.string(),
  description: z.string(),
  weight: z.number().optional(),
})

const RunEvalInputSchema = z.object({
  testInput: z.string(),
  criteria: EvalCriteriaSchema.array(),
  runs: z.number().optional(),
  model: z.string().optional(),
  passingScore: z.number().optional(),
})

export function SkillRoutes() {
  return new Hono()
    // ── List all skills ──────────────────────────────────────
    .get(
      "/",
      describeRoute({
        summary: "List skills",
        description: "Get a list of all available skills.",
        operationId: "app.skills",
        responses: {
          200: {
            description: "List of skills",
            content: { "application/json": { schema: resolver(Skill.Info.array()) } },
          },
        },
      }),
      async (c) => c.json(await Skill.all()),
    )

    .get(
      "/catalog",
      describeRoute({
        summary: "Inspect skill catalog",
        description: "List deterministic source roots and resolved name collisions.",
        operationId: "skill.catalog",
        responses: {
          200: {
            description: "Skill catalog diagnostics",
            content: { "application/json": { schema: resolver(z.any()) } },
          },
        },
      }),
      async (c) => c.json({ skills: await Skill.all(), collisions: await Skill.collisions(), roots: await Skill.roots() }),
    )

    // ── Rescan skill roots ───────────────────────────────────
    .post(
      "/reload",
      describeRoute({
        summary: "Reload skills",
        description: "Rescan skill directories, e.g. after a skill was installed on disk.",
        operationId: "skill.reload",
        responses: {
          200: {
            description: "Skills reloaded",
            content: { "application/json": { schema: resolver(z.object({ count: z.number() })) } },
          },
        },
      }),
      async (c) => {
        Skill.reload()
        return c.json({ count: (await Skill.all()).length })
      },
    )

    // ── Add skill from URL ───────────────────────────────────
    .post(
      "/add",
      describeRoute({
        summary: "Add skill from URL",
        description: "Download and register skills from a remote index URL.",
        operationId: "skill.add",
        responses: {
          200: {
            description: "Skills loaded",
            content: { "application/json": { schema: resolver(z.object({ added: z.number(), dirs: z.string().array() })) } },
          },
          ...errors(400),
        },
      }),
      validator("json", z.object({ url: z.string() })),
      async (c) => {
        const { url } = c.req.valid("json")
        const dirs = await Discovery.pull(url)
        return c.json({ added: dirs.length, dirs })
      },
    )

    // ── Run eval on a skill ──────────────────────────────────
    .post(
      "/:name/eval",
      describeRoute({
        summary: "Evaluate skill",
        description: "Run N parallel eval passes of a skill against test input and rubric criteria.",
        operationId: "skill.eval",
        responses: {
          200: {
            description: "Eval report",
            content: { "application/json": { schema: resolver(z.any()) } },
          },
          ...errors(400, 404),
        },
      }),
      validator("param", z.object({ name: z.string() })),
      validator("json", RunEvalInputSchema),
      async (c) => {
        const { name } = c.req.valid("param")
        const input = c.req.valid("json")

        const skill = await Skill.get(name)
        if (!skill) return c.json({ error: `Skill "${name}" not found` }, 404)

        const report = await Evaluator.run({ skillName: name, ...input })
        await EvalStore.save(report)
        return c.json(report)
      },
    )

    // ── List eval reports for a skill ────────────────────────
    .get(
      "/:name/evals",
      describeRoute({
        summary: "List skill evals",
        description: "List all eval reports for a skill.",
        operationId: "skill.evals.list",
        responses: {
          200: {
            description: "Eval reports",
            content: { "application/json": { schema: resolver(z.any()) } },
          },
        },
      }),
      validator("param", z.object({ name: z.string() })),
      async (c) => {
        const { name } = c.req.valid("param")
        const reports = await EvalStore.list(name)
        return c.json(reports)
      },
    )

    // ── Get a specific eval report ───────────────────────────
    .get(
      "/:name/evals/:id",
      describeRoute({
        summary: "Get skill eval",
        description: "Get a specific eval report by ID.",
        operationId: "skill.evals.get",
        responses: {
          200: {
            description: "Eval report",
            content: { "application/json": { schema: resolver(z.any()) } },
          },
          ...errors(404),
        },
      }),
      validator("param", z.object({ name: z.string(), id: z.string() })),
      async (c) => {
        const { name, id } = c.req.valid("param")
        const report = await EvalStore.get(name, id)
        if (!report) return c.json({ error: `Eval "${id}" not found` }, 404)
        return c.json(report)
      },
    )
}
