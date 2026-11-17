// @ts-nocheck
/**
 * current_time in the Gizzi TUI: a thin adapter over the shared clock core
 * (runtime/primitives/clock.ts), the same one the runtime builtin uses. The
 * terminal sends no per-turn zone, so the zone is the argument, else the last
 * zone a client sent to this machine's runtime, else this machine's own
 * (labelled as such).
 */
import { z } from 'zod/v4'
import { Clock } from '@/runtime/primitives/clock'
import { buildTool, type ToolDef } from '../../Tool.js'
import { lazySchema } from '../../utils/lazySchema.js'
import { CURRENT_TIME_TOOL_NAME, DESCRIPTION, PROMPT } from './prompt.js'
import { renderToolResultMessage, renderToolUseMessage } from './UI.js'

const inputSchema = lazySchema(() =>
  z.strictObject({
    timezone: z
      .string()
      .max(64)
      .optional()
      .describe('An IANA time zone such as "America/Chicago"; leave it out for the user\'s own zone'),
  }),
)
type InputSchema = ReturnType<typeof inputSchema>

const outputSchema = lazySchema(() =>
  z.object({
    iso: z.string(),
    time_zone: z.string(),
    utc_offset: z.string(),
    weekday: z.string(),
    source: z.enum(['argument', 'client', 'last_client', 'process']),
    // The zone the model asked for, when it was not valid (said in the result).
    requested: z.string().optional(),
  }),
)
type OutputSchema = ReturnType<typeof outputSchema>

export type Output = z.infer<OutputSchema>

export const CurrentTimeTool = buildTool({
  name: CURRENT_TIME_TOOL_NAME,
  searchHint: 'the current time and time zone where the user is',
  maxResultSizeChars: 10_000,
  userFacingName: () => 'Current time',
  get inputSchema(): InputSchema {
    return inputSchema()
  },
  get outputSchema(): OutputSchema {
    return outputSchema()
  },
  isReadOnly() {
    return true
  },
  isConcurrencySafe() {
    return true
  },
  toAutoClassifierInput(input) {
    return input.timezone ?? ''
  },
  async description() {
    return DESCRIPTION
  },
  async prompt() {
    return PROMPT
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    const { requested, ...result } = output
    return {
      tool_use_id: toolUseID,
      type: 'tool_result',
      content: Clock.currentOutput(result, requested),
    }
  },
  renderToolUseMessage,
  renderToolResultMessage,
  async call({ timezone }) {
    const result = await Clock.current({ timezone })
    return { data: timezone && result.source !== 'argument' ? { ...result, requested: timezone } : result }
  },
} satisfies ToolDef<InputSchema, Output>)
