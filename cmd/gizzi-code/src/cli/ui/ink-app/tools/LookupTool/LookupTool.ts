// @ts-nocheck
/**
 * lookup in the Gizzi TUI: a thin adapter over the shared lookup core
 * (runtime/primitives/lookup), the same one the runtime builtin uses
 * (runtime/tools/builtins/lookup.ts). The card comes back on the output and
 * UI.tsx draws it as aligned text with its credit line.
 *
 * The terminal shares no location, so "here" for weather and places asks the
 * model to get a place from the user before anything is fetched. The zone for
 * "now" is the last zone a client sent to this machine's runtime, else this
 * machine's own (Clock.resolveTimeZone).
 *
 * Permissions: time and unit conversion are local and ask nothing; network
 * kinds ask once, with an "allow lookup" rule on offer.
 */
import { z } from 'zod/v4'
import { Clock } from '@/runtime/primitives/clock'
import { LOOKUP_KINDS, LookupError, isLocalLookup, lookup, placeOrHere } from '@/runtime/primitives/lookup'
import { buildTool, type ToolDef } from '../../Tool.js'
import { lazySchema } from '../../utils/lazySchema.js'
import type { PermissionResult } from '../../utils/permissions/PermissionResult.js'
import { cardSearchText } from './cardText.js'
import { DESCRIPTION, LOOKUP_TOOL_NAME, PROMPT } from './prompt.js'
import { renderToolResultMessage, renderToolUseMessage } from './UI.js'

// Mirrors LOOKUP_PARAMETERS in runtime/tools/builtins/lookup.ts (not imported:
// that module pulls the runtime session types into the TUI bundle).
const inputSchema = lazySchema(() =>
  z.strictObject({
    kind: z.enum(LOOKUP_KINDS).describe('What to look up: weather, time, convert, link, places or images'),
    query: z.string().min(1).max(500).describe('The place, search, URL or moment, depending on kind'),
    place: z.string().max(200).optional().describe('Where: the place for weather, or where to search for places'),
    zones: z.array(z.string().min(1).max(64)).max(12).optional().describe('For time: IANA zones or place names'),
    amount: z.number().finite().optional().describe('For convert: the amount to convert'),
    from: z.string().min(1).max(32).optional().describe('For convert: the unit or currency code to convert from, e.g. "mi" or "USD"'),
    to: z.string().min(1).max(32).optional().describe('For convert: the unit or currency code to convert to, e.g. "km" or "EUR"'),
    count: z.number().int().min(1).max(10).optional().describe('Forecast days (weather, up to 7), places (up to 10) or images (3 to 5)'),
  }),
)
type InputSchema = ReturnType<typeof inputSchema>

const outputSchema = lazySchema(() =>
  z.object({
    kind: z.enum(LOOKUP_KINDS),
    text: z.string(),
    credit: z.string().optional(),
    // A LookupCard, already checked against the answer contract by the core.
    card: z.unknown().optional(),
    error: z.string().optional(),
  }),
)
type OutputSchema = ReturnType<typeof outputSchema>

export type Output = z.infer<OutputSchema>

const ASK_FOR_PLACE = {
  weather: "Which place? Name a city; the terminal doesn't share your location.",
  places: "Which place? Name a city or address; the terminal doesn't share your location.",
} as const

/** Same rule as the runtime tool: "here" (or no place, for weather) needs a shared location the TUI never has. */
function meansHere(input): boolean {
  if (input.kind === 'weather') return placeOrHere(input.place ?? input.query) === undefined
  if (input.kind === 'places') return !!input.place?.trim() && placeOrHere(input.place) === undefined
  return false
}

export const LookupTool = buildTool({
  name: LOOKUP_TOOL_NAME,
  searchHint: 'weather, time in other places, unit or currency conversion, link preview, places, images',
  maxResultSizeChars: 20_000,
  userFacingName: () => 'Lookup',
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
    return `${input.kind} ${input.query}`
  },
  async checkPermissions(input): Promise<PermissionResult> {
    if (isLocalLookup(input) || meansHere(input)) return { behavior: 'allow', updatedInput: input }
    return {
      behavior: 'passthrough',
      message: `Lookup (${input.kind}) uses the network.`,
      suggestions: [
        {
          type: 'addRules',
          rules: [{ toolName: LOOKUP_TOOL_NAME }],
          behavior: 'allow',
          destination: 'localSettings',
        },
      ],
    }
  },
  async description() {
    return DESCRIPTION
  },
  async prompt() {
    return PROMPT
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    return {
      tool_use_id: toolUseID,
      type: 'tool_result',
      content: output.error ? output.text : output.credit ? `${output.text}\n\nCredit: ${output.credit}` : output.text,
    }
  },
  extractSearchText(output) {
    return output.card ? cardSearchText(output.card) : output.text
  },
  renderToolUseMessage,
  renderToolResultMessage,
  async call(input, context) {
    if (meansHere(input)) {
      const message = ASK_FOR_PLACE[input.kind]
      return { data: { kind: input.kind, text: message, error: message } }
    }
    const { zone } = await Clock.resolveTimeZone({})
    try {
      const result = await lookup(input, { signal: context.abortController.signal, timeZone: zone })
      return { data: { kind: input.kind, text: result.text, credit: result.credit, ...(result.card ? { card: result.card } : {}) } }
    } catch (error) {
      if (!(error instanceof LookupError)) throw error
      return { data: { kind: input.kind, text: error.message, error: error.message } }
    }
  },
} satisfies ToolDef<InputSchema, Output>)
