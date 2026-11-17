// @ts-nocheck
/**
 * connector_search / connector_suggest in the Gizzi TUI: thin adapters over
 * the shared core (runtime/primitives/connectors.ts), the same one the
 * runtime builtins use (runtime/tools/builtins/connector.ts).
 *
 * The TUI has no Connect kit, so a suggestion is a text card:
 * "Connect <name> in Allternit → Settings → Connections". It carries no link
 * or code and connects nothing; the user connects in the app themselves.
 */
import { z } from 'zod/v4'
import { Connectors } from '@/runtime/primitives/connectors'
import { getSessionId } from '../../bootstrap/state.js'
import { buildTool, type ToolDef } from '../../Tool.js'
import { lazySchema } from '../../utils/lazySchema.js'
import {
  CONNECTOR_SEARCH_PROMPT,
  CONNECTOR_SEARCH_TOOL_NAME,
  CONNECTOR_SUGGEST_PROMPT,
  CONNECTOR_SUGGEST_TOOL_NAME,
} from './prompt.js'
import { renderSearchUseMessage, renderSuggestUseMessage, renderToolResultMessage } from './UI.js'

export const CONNECTIONS_PATH = 'Allternit → Settings → Connections'

const outputSchema = lazySchema(() =>
  z.object({
    summary: z.string(),
    text: z.string(),
    /** Suggestion card lines (connector_suggest only). */
    card: z.array(z.string()).optional(),
    reason: z.string().optional(),
  }),
)
type OutputSchema = ReturnType<typeof outputSchema>
export type Output = z.infer<OutputSchema>

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function mapResult(output: Output, toolUseID: string) {
  return { tool_use_id: toolUseID, type: 'tool_result', content: output.text }
}

/** Names from this process's searches, so the card can say "Slack" rather than an id. */
const names = new Map<string, string>()

export function cardLine(id: string): string {
  return `Connect ${names.get(id) ?? id} in ${CONNECTIONS_PATH}`
}

const searchSchema = lazySchema(() => Connectors.SearchInput)
const suggestSchema = lazySchema(() => Connectors.SuggestInput)

export const ConnectorSearchTool = buildTool({
  name: CONNECTOR_SEARCH_TOOL_NAME,
  searchHint: "find the user's connected apps (Gmail, Slack, Linear) and mount their tools",
  maxResultSizeChars: 100_000,
  userFacingName: () => 'Connectors',
  get inputSchema() {
    return searchSchema()
  },
  get outputSchema(): OutputSchema {
    return outputSchema()
  },
  shouldDefer: true,
  // `load` mounts tools for the session; a plain search changes nothing.
  isReadOnly: input => !input?.load,
  isConcurrencySafe: input => !input?.load,
  toAutoClassifierInput: input => input.query,
  async description() {
    return CONNECTOR_SEARCH_PROMPT
  },
  async prompt() {
    return CONNECTOR_SEARCH_PROMPT
  },
  mapToolResultToToolResultBlockParam: mapResult,
  renderToolUseMessage: renderSearchUseMessage,
  renderToolResultMessage,
  async call(input, context) {
    try {
      const r = await Connectors.search(getSessionId(), input, context.abortController?.signal)
      for (const hit of r.hits) names.set(hit.id, hit.name)
      const loaded = r.loaded ?? []
      const summary = loaded.length
        ? `Loaded ${loaded.length} connector tools`
        : `${r.hits.length} connector${r.hits.length === 1 ? '' : 's'} for "${input.query}"`
      return { data: { summary, text: JSON.stringify(r) } }
    } catch (error) {
      if (context.abortController?.signal.aborted) throw error
      return { data: { summary: 'Connector search unavailable', text: messageOf(error) } }
    }
  },
} satisfies ToolDef<any, Output>)

export const ConnectorSuggestTool = buildTool({
  name: CONNECTOR_SUGGEST_TOOL_NAME,
  searchHint: 'suggest a connector for the user to connect in Allternit Settings',
  maxResultSizeChars: 20_000,
  userFacingName: () => 'Suggest connector',
  get inputSchema() {
    return suggestSchema()
  },
  get outputSchema(): OutputSchema {
    return outputSchema()
  },
  shouldDefer: true,
  isReadOnly: () => true,
  isConcurrencySafe: () => true,
  toAutoClassifierInput: input => input.ids.join(' '),
  async description() {
    return CONNECTOR_SUGGEST_PROMPT
  },
  async prompt() {
    return CONNECTOR_SUGGEST_PROMPT
  },
  mapToolResultToToolResultBlockParam: mapResult,
  renderToolUseMessage: renderSuggestUseMessage,
  renderToolResultMessage,
  async call(input, context) {
    try {
      const r = await Connectors.suggest(getSessionId(), input)
      if (!r.shown.length) {
        return {
          data: {
            summary: 'No connector to suggest',
            text: `${JSON.stringify(r)}\nNo card was shown: suggest only ids from a connector_search in this session.`,
          },
        }
      }
      const card = r.shown.map(cardLine)
      return {
        data: {
          summary: `Suggested ${r.shown.map(id => names.get(id) ?? id).join(', ')}`,
          text: `${JSON.stringify(r)}\nThe user was shown:\n${card.join('\n')}\nNothing is connected until a later connector_search shows it connected.`,
          card,
          reason: input.reason,
        },
      }
    } catch (error) {
      return { data: { summary: 'Connector suggestion unavailable', text: messageOf(error) } }
    }
  },
} satisfies ToolDef<any, Output>)
