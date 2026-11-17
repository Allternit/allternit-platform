// @ts-nocheck
/**
 * agent_batch in the Gizzi TUI: a thin adapter over the shared core
 * (runtime/primitives/batch), the same one the runtime builtin uses, with the
 * same description and parameters. Each row's worker is one of the TUI's own
 * subagents (runAgent), run in the background (no permission prompts from
 * workers) with the batch worker tools only: no Agent, agent_batch, team or
 * task-list tools, no questions, no messages to the user, no sleep and no
 * scheduling (the runtime's WORKER_DENIED_TOOLS, by TUI name).
 *
 * Permission: always asks, with the row count, concurrency and model. A rule
 * is `agent_batch(<mode>:<agent>)`, so allowing a sample never allows a run.
 * While it runs, the progress line shows done / failed / running of N and the
 * output path.
 */
import fs from 'fs/promises'
import { z } from 'zod/v4'
import { outputSchema as batchOutputSchema } from '@/runtime/primitives/batch/schema'
import {
  DEFAULT_CONCURRENCY,
  DEFAULT_ROW_SECONDS,
  DEFAULT_ROWS,
  MAX_CONCURRENCY,
  MAX_ROW_SECONDS,
  MAX_ROWS,
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
} from '@/runtime/primitives/batch/run'
import type { AgentBatchMetadata } from '@/runtime/tools/builtins/agent-batch'
import { buildTool, type ToolDef } from '../../Tool.js'
import { getCwd } from '../../utils/cwd.js'
import { extractResultText } from '../../utils/forkedAgent.js'
import { lazySchema } from '../../utils/lazySchema.js'
import { createUserMessage } from '../../utils/messages.js'
import { getAgentModel } from '../../utils/model/agent.js'
import {
  allWorkingDirectories,
  getScratchpadDir,
  isScratchpadEnabled,
} from '../../utils/permissions/filesystem.js'
import { getRuleByContentsForTool } from '../../utils/permissions/permissions.js'
import { createAgentId } from '../../utils/uuid.js'
import { AGENT_TOOL_NAME, LEGACY_AGENT_TOOL_NAME } from '../AgentTool/constants.js'
import { runAgent } from '../AgentTool/runAgent.js'
import { ASK_USER_QUESTION_TOOL_NAME } from '../AskUserQuestionTool/prompt.js'
import { TODO_WRITE_TOOL_NAME } from '../TodoWriteTool/constants.js'
import { AGENT_BATCH_TOOL_NAME, DESCRIPTION, PROMPT } from './prompt.js'
import { renderToolResultMessage, renderToolUseMessage, renderToolUseProgressMessage } from './UI.js'

const inputSchema = lazySchema(() =>
  z.strictObject({
    csv_path: z.string().min(1).describe('The input CSV (first row is the header)'),
    instruction: z
      .string()
      .min(1)
      .max(8000)
      .describe('The task for one row, with {column} placeholders, e.g. "Find the website of {company}"'),
    output_schema: batchOutputSchema,
    id_column: z.string().min(1).optional().describe('A column with a unique id per row, used to resume'),
    output_csv_path: z.string().min(1).optional().describe('Where to write results (.csv); default <input>.results.csv'),
    subagent_type: z.string().min(1).optional().describe('The worker agent (default "general-purpose")'),
    mode: z.enum(['sample', 'run']).describe(`sample: run ${SAMPLE_ROWS} rows and estimate the full run, writing nothing; run: all rows`),
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
  }),
)
type InputSchema = ReturnType<typeof inputSchema>

/** The runtime's metadata plus the text the model gets. */
export type Output = AgentBatchMetadata & { text: string }
export type AgentBatchProgress = {
  type: 'agent_batch_progress'
  mode: 'sample' | 'run'
  done: number
  failed: number
  skipped: number
  running: number
  total: number
  output_csv_path: string
}

const outputSchema = lazySchema(() => z.object({ text: z.string() }).passthrough())
type OutputSchema = ReturnType<typeof outputSchema>

/** The runtime's worker-denied tools by TUI name (task → Agent, question → AskUserQuestion…). */
const WORKER_DENIED = new Set<string>([
  ...WORKER_DENIED_TOOLS,
  AGENT_TOOL_NAME,
  LEGACY_AGENT_TOOL_NAME,
  TODO_WRITE_TOOL_NAME,
  ASK_USER_QUESTION_TOOL_NAME,
  'TeamCreate',
  'TeamDelete',
  'SendMessage',
  'TaskCreate',
  'TaskUpdate',
])

/** The folders the CSV and the output may be in (send_user_message's rules in the TUI). */
function roots(context): string[] {
  const dirs = [getCwd(), ...allWorkingDirectories(context.getAppState().toolPermissionContext)]
  try {
    if (isScratchpadEnabled()) dirs.push(getScratchpadDir())
  } catch {
    // No scratchpad in this session.
  }
  return [...new Set(dirs)]
}

function findAgent(context, type: string) {
  const agents = context.options.agentDefinitions?.activeAgents ?? []
  const name = type === 'general' ? 'general-purpose' : type
  const agent = agents.find(a => a.agentType === name)
  if (!agent) {
    throw new Error(`Unknown subagent type: ${type}. Available: ${agents.map(a => a.agentType).join(', ')}`)
  }
  return agent
}

async function prepare(input, context) {
  const agent = findAgent(context, input.subagent_type ?? 'general-purpose')
  const pathCtx = { roots: roots(context), cwd: getCwd() }
  const csvPath = await resolveCsvPath(input.csv_path, pathCtx)
  const text = await fs.readFile(csvPath, 'utf8')
  const plan = prepareBatch({
    text,
    instruction: input.instruction,
    fields: input.output_schema,
    idColumn: input.id_column,
    maxRows: input.max_rows,
  })
  const outputPath = await resolveOutputPath(input.output_csv_path, csvPath, pathCtx)
  const model = getAgentModel(
    agent.model,
    context.options.mainLoopModel,
    undefined,
    context.getAppState().toolPermissionContext.mode,
  )
  const concurrency = clampConcurrency(input.max_concurrency)
  const seconds = clampRowSeconds(input.max_runtime_seconds)
  const rows = input.mode === 'sample' ? Math.min(SAMPLE_ROWS, plan.rows.length) : plan.rows.length
  return { agent, csvPath, text, plan, outputPath, model, concurrency, seconds, rows }
}

function tokensOf(messages): number {
  let sum = 0
  for (const m of messages) {
    const u = m.type === 'assistant' ? m.message?.usage : undefined
    if (!u) continue
    sum += (u.input_tokens ?? 0) + (u.output_tokens ?? 0) + (u.cache_read_input_tokens ?? 0) + (u.cache_creation_input_tokens ?? 0)
  }
  return sum
}

function duration(seconds: number): string {
  if (seconds < 90) return `${seconds} s`
  if (seconds < 5400) return `${Math.round(seconds / 60)} min`
  return `${(seconds / 3600).toFixed(1)} h`
}

const money = (n: number) => `$${n < 1 ? n.toFixed(4) : n.toFixed(2)}`
const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`

export const AgentBatchTool = buildTool({
  name: AGENT_BATCH_TOOL_NAME,
  searchHint: 'run the same task over every row of a CSV with one subagent per row',
  maxResultSizeChars: 40_000,
  userFacingName: () => 'Batch',
  get inputSchema(): InputSchema {
    return inputSchema()
  },
  get outputSchema(): OutputSchema {
    return outputSchema()
  },
  isReadOnly() {
    return false
  },
  isConcurrencySafe() {
    return false
  },
  toAutoClassifierInput(input) {
    return `${input.mode} ${input.csv_path}: ${input.instruction}`
  },
  async description() {
    return DESCRIPTION
  },
  async prompt() {
    return PROMPT
  },
  async validateInput(input, context) {
    try {
      await prepare(input, context)
      return { result: true }
    } catch (error) {
      return { result: false, message: error instanceof Error ? error.message : String(error), errorCode: 1 }
    }
  },
  async checkPermissions(input, context) {
    const p = await prepare(input, context)
    const ruleContent = `${input.mode}:${p.agent.agentType}`
    const permissionContext = context.getAppState().toolPermissionContext
    const deny = getRuleByContentsForTool(permissionContext, AgentBatchTool, 'deny').get(ruleContent)
    if (deny) {
      return {
        behavior: 'deny',
        message: `${AGENT_BATCH_TOOL_NAME} ${ruleContent} is denied by a permission rule.`,
        decisionReason: { type: 'rule', rule: deny },
      }
    }
    const allow = getRuleByContentsForTool(permissionContext, AgentBatchTool, 'allow').get(ruleContent)
    if (allow) return { behavior: 'allow', updatedInput: input, decisionReason: { type: 'rule', rule: allow } }
    const what = input.mode === 'sample' ? `sample ${plural(p.rows, 'row')} (nothing written)` : `run ${plural(p.rows, 'row')}`
    const message = [
      `Gizzi wants to ${what} of ${p.csvPath}, ${p.concurrency} at once, with ${p.agent.agentType} workers on ${p.model}.`,
      input.mode === 'run' ? `Results go to ${p.outputPath}.` : '',
      p.plan.overLimit ? `${plural(p.plan.overLimit, 'more row')} past max_rows won't run.` : '',
      'Every row is a model call.',
    ]
      .filter(Boolean)
      .join(' ')
    return {
      behavior: 'ask',
      message,
      suggestions: [
        {
          type: 'addRules',
          destination: 'localSettings',
          rules: [{ toolName: AGENT_BATCH_TOOL_NAME, ruleContent }],
          behavior: 'allow',
        },
      ],
    }
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    return { tool_use_id: toolUseID, type: 'tool_result', content: output.text }
  },
  renderToolUseMessage,
  renderToolUseProgressMessage,
  renderToolResultMessage,
  async call(input, context, canUseTool, _parentMessage, onProgress) {
    const p = await prepare(input, context)
    const { agent, plan, outputPath, concurrency, seconds, rows } = p
    const appState = context.getAppState()
    const { assembleToolPool } = await import('../../tools.js')
    const workerTools = assembleToolPool(
      { ...appState.toolPermissionContext, mode: agent.permissionMode ?? 'acceptEdits' },
      appState.mcp.tools,
    ).filter(tool => !WORKER_DENIED.has(tool.name))

    let running = 0
    let counts = { done: 0, failed: 0, skipped: 0 }
    let tick = 0
    const report = () =>
      onProgress?.({
        toolUseID: `agent-batch-${tick++}`,
        data: { type: 'agent_batch_progress', mode: input.mode, ...counts, running, total: rows, output_csv_path: outputPath },
      })

    const worker: BatchWorker = {
      async start({ row, signal }) {
        const agentId = createAgentId()
        const abortController = new AbortController()
        const onAbort = () => abortController.abort(signal.reason)
        if (signal.aborted) abortController.abort(signal.reason)
        else signal.addEventListener('abort', onAbort, { once: true })
        let history = []
        running++
        report()
        return {
          runID: agentId,
          async send(prompt) {
            const promptMessages = [...history, createUserMessage({ content: prompt })]
            const out = []
            for await (const message of runAgent({
              agentDefinition: agent,
              promptMessages,
              toolUseContext: context,
              canUseTool,
              isAsync: true,
              querySource: 'agent:custom',
              availableTools: workerTools,
              override: { agentId, abortController },
              description: `Row ${row.id} (agent_batch)`,
              transcriptSubdir: 'agent-batch',
            })) {
              if (message.type === 'assistant' || message.type === 'user') out.push(message)
            }
            history = [...promptMessages, ...out]
            const last = out.findLast(m => m.type === 'assistant')
            if (last?.isApiErrorMessage) throw new Error(extractResultText([last], 'The model call failed.'))
            return { text: extractResultText(out, ''), tokens: tokensOf(out) }
          },
          close() {
            signal.removeEventListener('abort', onAbort)
            abortController.abort()
            running--
            report()
          },
        }
      },
    }

    const base = {
      mode: input.mode,
      subagent_type: agent.agentType,
      model: p.model,
      concurrency,
      total: rows,
      over_limit: plan.overLimit,
      output_csv_path: outputPath,
    } as const
    report()

    if (input.mode === 'sample') {
      const sample = await runSample(plan, {
        worker,
        maxConcurrency: concurrency,
        maxRuntimeSeconds: seconds,
        signal: context.abortController.signal,
      })
      const done = sample.outcomes.filter(o => o.status === 'ok').length
      const metadata: AgentBatchMetadata = {
        ...base,
        status: sample.cancelled ? 'cancelled' : 'done',
        done,
        failed: sample.outcomes.length - done,
        skipped: 0,
        tokens: sample.outcomes.reduce((sum, o) => sum + o.tokens, 0),
        sample: sample.outcomes.map(o => ({ id: o.row.id, status: o.status, values: o.values, error: o.error })),
        estimate: sample.estimate,
      }
      const est = sample.estimate
      const text = [
        `Sample: ${done} of ${plural(sample.outcomes.length, 'row')} ok${sample.cancelled ? ' (cancelled)' : ''}. Nothing was written.`,
        JSON.stringify(metadata.sample, null, 2),
        est
          ? `Estimate for all ${est.rows} rows with ${concurrency} at once on ${p.model}: about ${est.tokens.toLocaleString('en-US')} tokens, ${est.cost === null ? 'cost not reported by this model' : `about ${money(est.cost)}`}, about ${duration(est.seconds)}.`
          : 'No sample row finished, so there is no estimate.',
        plan.overLimit ? `${plural(plan.overLimit, 'more row')} are past max_rows and won't run.` : '',
        `Show the user these results and the estimate. Run all ${plan.rows.length} rows with mode "run" only after they agree; results will go to ${outputPath}.`,
      ]
        .filter(Boolean)
        .join('\n\n')
      return { data: { ...metadata, text } }
    }

    const result = await runBatch(plan, {
      worker,
      maxConcurrency: concurrency,
      maxRuntimeSeconds: seconds,
      signal: context.abortController.signal,
      outputPath,
      csvPath: p.csvPath,
      csvHash: hashText(p.text),
      onProgress: progress => {
        counts = { done: progress.done, failed: progress.failed, skipped: progress.skipped }
        report()
      },
    })
    const metadata: AgentBatchMetadata = {
      ...base,
      status: result.cancelled ? 'cancelled' : 'done',
      done: result.done,
      failed: result.failed,
      skipped: result.skipped,
      tokens: result.tokens,
      failures: result.failures,
    }
    const text = [
      `${result.done} ok, ${result.failed} failed${result.skipped ? `, ${result.skipped} skipped (finished on an earlier run)` : ''}${result.cancelled ? `, ${result.remaining} not run (cancelled)` : ''}, of ${result.total} rows. ${result.tokens.toLocaleString('en-US')} tokens.`,
      `Results: ${result.output_csv_path}`,
      result.failures.length ? `Failures:\n${result.failures.map(f => `- ${f.id}: ${f.error}`).join('\n')}` : '',
      result.overLimit ? `${plural(result.overLimit, 'row')} past max_rows were not run.` : '',
      result.failed || result.remaining
        ? 'Call agent_batch again with the same arguments to run the failed and unfinished rows; rows that finished ok are skipped.'
        : '',
    ]
      .filter(Boolean)
      .join('\n\n')
    return { data: { ...metadata, text } }
  },
} satisfies ToolDef<InputSchema, Output>)
