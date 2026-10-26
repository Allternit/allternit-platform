import {
  getCwdState,
  getOriginalCwd,
  getSessionId,
  getTotalCostUSD,
  getTotalDuration,
  getTotalInputTokens,
  getTotalOutputTokens,
  getTotalCacheReadInputTokens,
  getTotalCacheCreationInputTokens,
} from '../bootstrap/state.js'
import {
  calculateContextPercentages,
  getContextWindowForModel,
} from './context.js'
import { getCurrentUsage, getTokenUsage } from './tokens.js'
import {
  getRuntimeMainLoopModel,
  renderModelName,
} from './model/model.js'
import { getHarnessMode, shouldUseHarness } from './feature-flags.js'
import {
  getAuthTokenSource,
  getAllternitApiKeyWithSource,
} from './auth.js'
import { getAPIProvider } from './model/providers.js'
import { detectWorkspace } from '../../../../runtime/kernel/bridge.js'
import type { LocalJSXCommandContext } from '../types/command.js'
import type { Message } from '../types/message.js'

export const STATUS_SCHEMA_VERSION = 1

/** Human-readable auth method; works for first-party and 3P providers. */
export function getAuthMethodDescription(): string {
  const apiProvider = getAPIProvider()
  if (apiProvider !== 'firstParty') {
    return apiProvider
  }
  const { source, hasToken } = getAuthTokenSource()
  if (hasToken) {
    return source
  }
  // Under CI the key lookup throws when no key is set; /status reports
  // "none" instead of failing.
  try {
    const { key, source: apiKeySource } = getAllternitApiKeyWithSource()
    if (key) {
      return apiKeySource
    }
  } catch {}
  return 'none'
}

/** Count user turns: non-meta user messages that are not tool results. */
export function countUserTurns(messages: Array<any> | undefined): number {
  return (messages ?? []).filter(m => {
    if (m.type !== 'user' || m.isMeta) {
      return false
    }
    const block = m.message?.content?.[0]
    return block?.type !== 'tool_result'
  }).length
}

export type SessionStatus = {
  schemaVersion: number
  model: string
  version: string
  directory: string
  projectDirectory?: string
  sessionId: string
  authMethod: string
  turns: number
  context: {
    used: number | null
    total: number
    percent: number | null
  }
  costUSD: number
  durationMs: number
  requests: number
  tokens: {
    input: number
    output: number
    cacheRead: number
    cacheCreation: number
  }
  mcp: {
    connected: number
    servers: string[]
  }
  harness: {
    enabled: boolean
    mode: string
  }
  workspace: {
    present: boolean
    path?: string
    name?: string
  }
  errors: string[]
}

export async function buildSessionStatus(
  context: Pick<LocalJSXCommandContext, 'getAppState' | 'options'>,
): Promise<SessionStatus> {
  // TODO(types): AppState has no `messages` field (upstream drift shared with
  // status.tsx/dash.ts/live.ts/usage.ts); runtime value is always undefined,
  // so the `?? []` fallback was always in effect.
  const messages = (context.getAppState() as { messages?: Message[] }).messages ?? []
  const model = getRuntimeMainLoopModel({
    permissionMode: context.getAppState().toolPermissionContext.mode,
    mainLoopModel: context.options.mainLoopModel,
    exceeds200kTokens: false,
  })

  const modelDisplay = renderModelName(model)
  const cwd = getCwdState()
  const projectDir = getOriginalCwd()
  const sessionId = getSessionId()
  const version =
    (typeof MACRO !== 'undefined' ? MACRO.VERSION : undefined) ?? 'unknown'

  const contextWindowSize = getContextWindowForModel(model, undefined)
  const currentUsage = getCurrentUsage(messages)
  const contextPercentages = calculateContextPercentages(
    currentUsage,
    contextWindowSize,
  )

  const totalCost = getTotalCostUSD()
  const totalDuration = getTotalDuration()
  const totalInput = getTotalInputTokens()
  const totalOutput = getTotalOutputTokens()
  const cacheRead = getTotalCacheReadInputTokens()
  const cacheCreation = getTotalCacheCreationInputTokens()
  const requestCount = messages.filter(m => getTokenUsage(m) !== undefined).length

  const mcpClients = context.options.mcpClients ?? []
  const mcpServers = mcpClients
    .map(c => c.name ?? 'unknown')
    .filter((n): n is string => Boolean(n))

  const harnessMode = getHarnessMode()
  const harnessEnabled = shouldUseHarness()

  const workspace = await detectWorkspace(cwd).catch(() => null)

  return {
    schemaVersion: STATUS_SCHEMA_VERSION,
    model: modelDisplay,
    version,
    directory: cwd,
    ...(projectDir && projectDir !== cwd ? { projectDirectory: projectDir } : {}),
    sessionId,
    authMethod: getAuthMethodDescription(),
    turns: countUserTurns(messages),
    context: {
      used:
        contextPercentages.used !== null && currentUsage
          ? currentUsage.input_tokens +
            currentUsage.cache_creation_input_tokens +
            currentUsage.cache_read_input_tokens
          : null,
      total: contextWindowSize,
      percent: contextPercentages.used,
    },
    costUSD: totalCost,
    durationMs: totalDuration,
    requests: requestCount,
    tokens: {
      input: totalInput,
      output: totalOutput,
      cacheRead,
      cacheCreation,
    },
    mcp: {
      connected: mcpClients.length,
      servers: mcpServers,
    },
    harness: {
      enabled: harnessEnabled,
      mode: harnessMode,
    },
    workspace: {
      present: workspace !== null,
      ...(workspace?.path ? { path: workspace.path } : {}),
      ...(workspace?.identity?.name ? { name: workspace.identity.name } : {}),
    },
    errors: [],
  }
}
