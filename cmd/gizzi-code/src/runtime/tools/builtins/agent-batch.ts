/**
 * agent_batch: one subagent per CSV row, each replying with JSON checked
 * against an output schema (runtime/primitives/batch). Sample first (1-3
 * rows, nothing written, a cost and time estimate), then run: each row is
 * appended to the output CSV as it finishes, with a progress sidecar so the
 * same call resumes.
 *
 * Workers are child sessions like agent_swarm's, scheduled by the same
 * AdaptiveRunBatch, but they cannot fan out: task, agent_swarm, agent_batch
 * and batch are off, as are question, send_user_message, sleep and
 * automation (WORKER_DENIED_TOOLS), both as tools and as deny rules.
 *
 * Runs on the runtime this session runs on (the user's own computer or their
 * own cloud computer); there is no hosted batch route.
 *
 * Permission: `agent_batch`, asked with the row count, concurrency, model
 * and paths. The pattern is `<mode>:<subagent_type>`, so "always allow" for
 * a sample never approves a full run.
 */
import fs from "fs/promises"
import z from "zod/v4"
import { LabelField } from "@/runtime/primitives/gap-types"
import * as BotTurn from "@/runtime/bots/bot-turn"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Agent } from "@/runtime/loop/agent"
import { Session } from "@/runtime/session"
import { SessionPrompt } from "@/runtime/session/prompt"
import { MessageV2 } from "@/runtime/session/message-v2"
import { Config } from "@/runtime/context/config/config"
import { Instance } from "@/runtime/context/project/instance"
import { HookDispatcher } from "@/runtime/hooks/dispatcher"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { allowedRoots } from "@/runtime/tools/builtins/send-user-message"
import { assertOwnedSubagentRun, throwIfSubagentMessageFailed } from "@/runtime/agents/subagent-run-contract"
import { outputSchema } from "@/runtime/primitives/batch/schema"
import {
  DEFAULT_CONCURRENCY,
  DEFAULT_ROWS,
  DEFAULT_ROW_SECONDS,
  MAX_CONCURRENCY,
  MAX_ROWS,
  MAX_ROW_SECONDS,
  SAMPLE_ROWS,
  WORKER_DENIED_TOOLS,
  clampConcurrency,
  clampRowSeconds,
  hashText,
  prepareBatch,
  resolveCsvPath,
  resolveOutputPath,
  runBatch,
  runSample,
  type BatchWorker,
  type Estimate,
  type WorkerTurn,
} from "@/runtime/primitives/batch/run"
import DESCRIPTION from "@/runtime/tools/builtins/agent-batch.txt"

export const AGENT_BATCH_PARAMETERS = z.object({
  ...LabelField,
  csv_path: z.string().min(1).describe("The input CSV (first row is the header)"),
  instruction: z
    .string()
    .min(1)
    .max(8000)
    .describe('The task for one row, with {column} placeholders, e.g. "Find the website of {company}"'),
  output_schema: outputSchema,
  id_column: z.string().min(1).optional().describe("A column with a unique id per row, used to resume"),
  output_csv_path: z.string().min(1).optional().describe("Where to write results (.csv); default <input>.results.csv"),
  subagent_type: z.string().min(1).optional().describe('The worker agent (default "general")'),
  mode: z.enum(["sample", "run"]).describe(`sample: run ${SAMPLE_ROWS} rows and estimate the full run, writing nothing; run: all rows`),
  max_concurrency: z
    .number()
    .int()
    .min(1)
    .max(MAX_CONCURRENCY)
    .optional()
    .describe(`Workers at once (default ${DEFAULT_CONCURRENCY}, max ${MAX_CONCURRENCY})`),
  max_runtime_seconds: z
    .number()
    .int()
    .min(10)
    .max(MAX_ROW_SECONDS)
    .optional()
    .describe(`Time per row in seconds (default ${DEFAULT_ROW_SECONDS}, max ${MAX_ROW_SECONDS})`),
  max_rows: z.number().int().min(1).max(MAX_ROWS).optional().describe(`Rows to cover (default ${DEFAULT_ROWS}, max ${MAX_ROWS})`),
})

export type AgentBatchParams = z.infer<typeof AGENT_BATCH_PARAMETERS>

/** What clients draw (the tool part's metadata), updated as rows finish. */
export type AgentBatchMetadata = {
  mode: "sample" | "run"
  status: "running" | "done" | "cancelled"
  subagent_type: string
  model: string
  concurrency: number
  /** Rows this call covers. */
  total: number
  done: number
  failed: number
  skipped: number
  over_limit: number
  output_csv_path: string
  tokens: number
  cost?: number
  /** Sample mode: the sampled rows. */
  sample?: Array<{ id: string; status: "ok" | "failed"; values?: Record<string, unknown>; error?: string }>
  /** Sample mode: the full run's estimate; null when no row finished. */
  estimate?: Estimate | null
  failures?: Array<{ id: string; error: string }>
}

function turnOf(message: MessageV2.WithParts): WorkerTurn {
  const info = message.info as Partial<MessageV2.Assistant>
  const t = info.tokens
  const tokens = t ? t.input + t.output + t.reasoning + (t.cache?.read ?? 0) + (t.cache?.write ?? 0) : 0
  const text = message.parts.findLast((part) => part.type === "text")
  return { text: text && "text" in text ? text.text : "", tokens, cost: info.cost ?? 0 }
}

function duration(seconds: number): string {
  if (seconds < 90) return `${seconds} s`
  if (seconds < 5400) return `${Math.round(seconds / 60)} min`
  return `${(seconds / 3600).toFixed(1)} h`
}

const money = (n: number) => `$${n < 1 ? n.toFixed(4) : n.toFixed(2)}`

export const AgentBatchTool = Tool.define("agent_batch", async () => ({
  description: DESCRIPTION,
  parameters: AGENT_BATCH_PARAMETERS,
  async execute(params, ctx) {
    const parentMessage = await MessageV2.get({ sessionID: ctx.sessionID, messageID: ctx.messageID })
    if (parentMessage.info.role !== "assistant") throw new Error("agent_batch must be called from an assistant turn")
    const parentInfo = parentMessage.info

    const type = params.subagent_type ?? "general"
    const agent = await Agent.getSubagent(type)
    if (!agent || agent.mode === "primary") throw new Error(`Unknown subagent type: ${type}`)
    const caller = await Agent.get(ctx.agent)
    if (caller && PermissionNext.evaluate("task", agent.name, caller.permission).action === "deny") {
      throw new Error(`This agent may not start ${agent.name} workers.`)
    }

    const pathCtx = { roots: await allowedRoots(ctx.sessionID), cwd: Instance.workdir }
    const csvPath = await resolveCsvPath(params.csv_path, pathCtx)
    const text = await fs.readFile(csvPath, "utf8")
    const plan = prepareBatch({
      text,
      instruction: params.instruction,
      fields: params.output_schema,
      idColumn: params.id_column,
      maxRows: params.max_rows,
    })
    const outputPath = await resolveOutputPath(params.output_csv_path, csvPath, pathCtx)

    const model = agent.model ?? { providerID: parentInfo.providerID, modelID: parentInfo.modelID }
    const modelName = `${model.providerID}/${model.modelID}`
    const concurrency = clampConcurrency(params.max_concurrency)
    const seconds = clampRowSeconds(params.max_runtime_seconds)
    const rows = params.mode === "sample" ? Math.min(SAMPLE_ROWS, plan.rows.length) : plan.rows.length

    await ctx.ask({
      permission: "agent_batch",
      patterns: [`${params.mode}:${agent.name}`],
      always: [`${params.mode}:${agent.name}`],
      metadata: {
        mode: params.mode,
        rows,
        total_rows: plan.totalRows,
        over_limit: plan.overLimit,
        concurrency,
        model: modelName,
        subagent_type: agent.name,
        max_runtime_seconds: seconds,
        csv_path: csvPath,
        output_csv_path: params.mode === "run" ? outputPath : undefined,
        preview: plan.rows.slice(0, 3).map((row) =>
          Object.fromEntries(Object.entries(row.record).map(([k, v]) => [k, v.length > 80 ? v.slice(0, 79) + "…" : v])),
        ),
      },
    })

    const config = await Config.get()
    const tools: Record<string, boolean> = {
      ...Object.fromEntries((config.experimental?.primary_tools ?? []).map((tool) => [tool, false])),
      ...Object.fromEntries(WORKER_DENIED_TOOLS.map((tool) => [tool, false])),
    }
    const bot = BotTurn.get(ctx.sessionID)

    const worker: BatchWorker = {
      async start({ row, signal, retryRunID }) {
        const child = retryRunID
          ? await Session.get(retryRunID).then((session) => {
              assertOwnedSubagentRun(
                { id: session.id, parentID: session.parentID, profile: session.agentID, title: session.title },
                { parentSessionID: ctx.sessionID, profile: agent.name },
              )
              return session
            })
          : await Session.create({
              parentID: ctx.sessionID,
              agentID: agent.name,
              title: `Row ${row.id} (@${agent.name} batch worker)`,
              permission: WORKER_DENIED_TOOLS.map((permission) => ({ permission, pattern: "*", action: "deny" as const })),
            })
        if (!retryRunID) await PermissionNext.setMode(child.id, await PermissionNext.getMode(ctx.sessionID))
        // A bot's batch workers stay isolated from the user's personal setup.
        BotTurn.mark(child.id, bot)
        const cancel = () => SessionPrompt.cancel(child.id)
        signal.addEventListener("abort", cancel, { once: true })
        await HookDispatcher.emit({
          name: "SubagentStart",
          timestamp: Date.now(),
          sessionId: ctx.sessionID,
          payload: { agent: agent.name, childSessionID: child.id, batch: true },
        })
        const turn = (message: MessageV2.WithParts) => {
          throwIfSubagentMessageFailed(message)
          return turnOf(message)
        }
        return {
          runID: child.id,
          // Plain text parts: row values are data, so an "@path" in a cell is never read as a file include.
          send: async (prompt) =>
            turn(
              await SessionPrompt.prompt({
                sessionID: child.id,
                agent: agent.name,
                model,
                tools,
                parts: [{ type: "text", text: prompt }],
              }),
            ),
          retry: async () => turn(await SessionPrompt.retry(child.id)),
          close: async () => {
            signal.removeEventListener("abort", cancel)
            await HookDispatcher.emit({
              name: "SubagentStop",
              timestamp: Date.now(),
              sessionId: ctx.sessionID,
              payload: { agent: agent.name, childSessionID: child.id, batch: true },
            })
          },
        }
      },
    }

    const base = {
      mode: params.mode,
      subagent_type: agent.name,
      model: modelName,
      concurrency,
      total: rows,
      over_limit: plan.overLimit,
      output_csv_path: outputPath,
    } as const

    if (params.mode === "sample") {
      ctx.metadata({
        title: `Sampling ${rows} row${rows === 1 ? "" : "s"}`,
        metadata: { ...base, status: "running", done: 0, failed: 0, skipped: 0, tokens: 0 } satisfies AgentBatchMetadata,
      })
      const sample = await runSample(plan, { worker, maxConcurrency: concurrency, maxRuntimeSeconds: seconds, signal: ctx.abort })
      const done = sample.outcomes.filter((o) => o.status === "ok").length
      const tokens = sample.outcomes.reduce((sum, o) => sum + o.tokens, 0)
      const cost = sample.outcomes.reduce((sum, o) => sum + o.cost, 0)
      const metadata: AgentBatchMetadata = {
        ...base,
        status: sample.cancelled ? "cancelled" : "done",
        done,
        failed: sample.outcomes.length - done,
        skipped: 0,
        tokens,
        ...(cost > 0 ? { cost } : {}),
        sample: sample.outcomes.map((o) => ({ id: o.row.id, status: o.status, values: o.values, error: o.error })),
        estimate: sample.estimate,
      }
      const est = sample.estimate
      const lines = [
        `Sample: ${done} of ${sample.outcomes.length} row${sample.outcomes.length === 1 ? "" : "s"} ok${sample.cancelled ? " (cancelled)" : ""}. Nothing was written.`,
        JSON.stringify(metadata.sample, null, 2),
        est
          ? `Estimate for all ${est.rows} rows with ${concurrency} at once on ${modelName}: about ${est.tokens.toLocaleString("en-US")} tokens, ${est.cost === null ? "cost not reported by this model" : `about ${money(est.cost)}`}, about ${duration(est.seconds)}.`
          : "No sample row finished, so there is no estimate.",
        plan.overLimit ? `${plan.overLimit} more row${plan.overLimit === 1 ? "" : "s"} are past max_rows and won't run.` : "",
        `Show the user these results and the estimate. Run all ${plan.rows.length} rows with mode "run" only after they agree; results will go to ${outputPath}.`,
      ]
      return { title: `Sampled ${sample.outcomes.length} rows`, metadata, output: lines.filter(Boolean).join("\n\n") }
    }

    ctx.metadata({
      title: `Batch 0/${rows}`,
      metadata: { ...base, status: "running", done: 0, failed: 0, skipped: 0, tokens: 0 } satisfies AgentBatchMetadata,
    })
    const result = await runBatch(plan, {
      worker,
      maxConcurrency: concurrency,
      maxRuntimeSeconds: seconds,
      signal: ctx.abort,
      outputPath,
      csvPath,
      csvHash: hashText(text),
      onProgress: (p) =>
        ctx.metadata({
          title: `Batch ${p.done + p.failed + p.skipped}/${p.total}`,
          metadata: { ...base, status: "running", done: p.done, failed: p.failed, skipped: p.skipped, tokens: 0 } satisfies AgentBatchMetadata,
        }),
    })
    const metadata: AgentBatchMetadata = {
      ...base,
      status: result.cancelled ? "cancelled" : "done",
      done: result.done,
      failed: result.failed,
      skipped: result.skipped,
      tokens: result.tokens,
      ...(result.cost > 0 ? { cost: result.cost } : {}),
      failures: result.failures,
    }
    const lines = [
      `${result.done} ok, ${result.failed} failed${result.skipped ? `, ${result.skipped} skipped (finished on an earlier run)` : ""}${result.cancelled ? `, ${result.remaining} not run (cancelled)` : ""}, of ${result.total} rows. ${result.tokens.toLocaleString("en-US")} tokens${result.cost > 0 ? `, ${money(result.cost)}` : ""}.`,
      `Results: ${result.output_csv_path}`,
      result.failures.length ? `Failures:\n${result.failures.map((f) => `- ${f.id}: ${f.error}`).join("\n")}` : "",
      result.overLimit ? `${result.overLimit} row${result.overLimit === 1 ? "" : "s"} past max_rows were not run.` : "",
      result.failed || result.remaining
        ? "Call agent_batch again with the same arguments to run the failed and unfinished rows; rows that finished ok are skipped."
        : "",
    ]
    return {
      title: `Batch ${result.done}/${result.total} ok`,
      metadata,
      output: lines.filter(Boolean).join("\n\n"),
    }
  },
}))
