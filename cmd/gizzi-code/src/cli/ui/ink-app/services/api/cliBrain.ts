// @ts-nocheck
/**
 * CLI brains in the TUI: `claude-cli/…`, `codex-cli/…`, `kimi-cli/…`, any CLI
 * the user added, and Subscription Fabric (`subs-*`). These run the user's own
 * signed-in CLI through the engine's driver (runtime/drivers/local-cli-driver,
 * the same code `gizzi run` and Desktop use), never the Allternit API.
 *
 * The CLI runs its own tools. Its text, thinking and tool calls stream back
 * here as the TUI's normal messages: tool calls are marked `cliObserved` so
 * the query loop shows them but does not run them again, and each tool the
 * CLI wants approval for goes through the TUI's own permission prompt
 * (AgentTask.approveTool), in the TUI's permission mode.
 */
import { randomUUID } from 'crypto'
import { ProviderOffer } from '@/runtime/providers/offer'
import { ModelThinking } from '@/runtime/providers/discovery/model-thinking'
import { findSubprocessSpec } from '@/runtime/providers/discovery/subprocess'
import type { AgentEvent, AgentTask, CliToolApproval, CliToolVerdict } from '@/runtime/runtime-driver'
import { getSessionId } from '../../bootstrap/state.js'
import { getCwd } from '../../utils/cwd.js'
import { logForDebugging } from '../../utils/debug.js'
import { findToolByName } from '../../Tool.js'
import { MCPTool } from '../../tools/MCPTool/MCPTool.js'
import { createAssistantAPIErrorMessage, createAssistantMessage, createUserMessage } from '../../utils/messages.js'
import type { AssistantMessage, Message, StreamEvent, SystemAPIErrorMessage, UserMessage } from '../../types/message.js'
import type { Options } from './claude.js'
import { describeProviderError, formatProviderErrorHelp } from './providerErrors.js'

export type CliBrain = { provider: string; modelId: string; fabric: boolean }

/** The CLI brain a model ref names, or undefined for any other provider. */
export function cliBrainOf(model: string | null | undefined): CliBrain | undefined {
  const slash = model?.indexOf('/') ?? -1
  if (!model || slash <= 0 || slash === model.length - 1) return undefined
  const provider = model.slice(0, slash)
  const modelId = model.slice(slash + 1)
  if (ProviderOffer.isCliBrain(provider)) return { provider, modelId, fabric: false }
  if (ProviderOffer.isFabric(provider)) return { provider, modelId, fabric: true }
  return undefined
}

export function isCliBrainModel(model: string | null | undefined): boolean {
  return cliBrainOf(model) !== undefined
}

/** The vendor CLI's own session per TUI session, so turn 2 resumes turn 1. */
const vendorSessions = new Map<string, string>()

/** The TUI's permission modes the CLI understands; others ask through the TUI (default). */
function cliPermissionMode(mode: string | undefined): AgentTask['permissionMode'] {
  return mode === 'acceptEdits' || mode === 'plan' || mode === 'bypassPermissions' ? mode : 'default'
}

/** CLI tool names → the TUI tool that renders and gates the same thing. */
const TOOL_ALIASES: Record<string, string> = {
  exec_command: 'Bash',
  web_search: 'WebSearch',
  web_fetch: 'WebFetch',
}

function tuiToolInput(name: string, args: unknown): Record<string, unknown> {
  const input = args && typeof args === 'object' ? { ...(args as Record<string, unknown>) } : {}
  if (name === 'exec_command' && Array.isArray(input.command)) input.command = input.command.join(' ')
  return input
}

/**
 * What the user sent this turn: the text of the user messages after the last
 * assistant reply. Meta messages (the prepended context, attachments) are
 * left out when the turn has the user's own words; the CLI loads its own
 * project memory and reads files itself.
 */
export function turnText(messages: readonly Message[]): string {
  let start = messages.length
  while (start > 0 && messages[start - 1]!.type !== 'assistant') start--
  const tail = messages.slice(start).filter(m => m.type === 'user') as UserMessage[]
  const textOf = (m: UserMessage) => {
    const c = m.message?.content
    if (typeof c === 'string') return c
    if (!Array.isArray(c)) return ''
    return c
      .filter(b => b && typeof b === 'object' && b.type === 'text')
      .map(b => String(b.text ?? ''))
      .join('\n')
  }
  const own = tail.filter(m => !m.isMeta).map(textOf).filter(Boolean)
  return (own.length > 0 ? own : tail.map(textOf).filter(Boolean)).join('\n\n').trim()
}

/** A tool the TUI has no component for still gets the generic approval prompt. */
function genericTool(name: string) {
  return { ...MCPTool, name, isMcp: false, userFacingName: () => name }
}

/**
 * The TUI's permission prompt for one tool a CLI brain wants to run: the
 * TUI's rules and mode decide, and anything left to ask shows the normal
 * approval dialog for that tool (diff for Edit, command for Bash…).
 */
export async function approveCliTool(
  request: CliToolApproval,
  canUseTool: (...args: unknown[]) => Promise<{ behavior: string; updatedInput?: Record<string, unknown>; message?: string }>,
  toolUseContext: { options: { tools: unknown[] } },
): Promise<CliToolVerdict> {
  const name = TOOL_ALIASES[request.name] ?? request.name
  const tool = findToolByName(toolUseContext.options.tools, name) ?? genericTool(request.name)
  const input = tuiToolInput(request.name, request.input)
  const id = request.id ?? `cli-${randomUUID()}`
  const assistantMessage = createAssistantMessage({ content: [{ type: 'tool_use', id, name: tool.name, input }] })
  const decision = await canUseTool(tool, input, toolUseContext, assistantMessage, id)
  if (decision.behavior === 'allow') return { allow: true, input: decision.updatedInput ?? request.input }
  return { allow: false, message: decision.message || `The user did not approve ${request.name}.` }
}

function isMainThread(querySource: string | undefined): boolean {
  return !querySource || querySource.startsWith('repl_main_thread') || querySource === 'sdk'
}

/** Driver events for one CLI turn. */
async function* driverEvents(
  brain: CliBrain,
  prompt: string,
  signal: AbortSignal,
  options: Options,
  turn: { resumeKey?: string; toolsOff: boolean },
): AsyncGenerator<AgentEvent> {
  const { RuntimeDriverFactory } = await import('@/runtime/runtime-driver-factory')
  const { driver } = await RuntimeDriverFactory.resolveCli(brain.provider)
  const approve = (options as { cliToolApproval?: AgentTask['approveTool'] }).cliToolApproval
  const permissionContext = await options.getToolPermissionContext?.().catch(() => undefined)
  const effort = typeof options.effortValue === 'string' ? ModelThinking.check(brain.modelId, options.effortValue) : undefined
  const task = await driver.assign({
    taskId: `tui-${Date.now().toString(36)}-${randomUUID().slice(0, 6)}`,
    prompt,
    model: brain.modelId,
    cwd: getCwd(),
    vendorSessionId: turn.resumeKey ? vendorSessions.get(turn.resumeKey) : undefined,
    toolsOff: turn.toolsOff,
    effort,
    ...(approve && !turn.toolsOff
      ? { permissionMode: cliPermissionMode(permissionContext?.mode), approveTool: approve }
      : {}),
  })
  const onAbort = () => void driver.abort(task).catch(() => {})
  signal.addEventListener('abort', onAbort, { once: true })
  try {
    for await (const event of driver.stream(task)) {
      if (signal.aborted) return
      if (event.type === 'vendor_session' && turn.resumeKey) vendorSessions.set(turn.resumeKey, event.id)
      yield event
    }
  } finally {
    signal.removeEventListener('abort', onAbort)
  }
}

/** Subscription Fabric (`subs-*`): the user's web subscription on their Sessions computer. */
async function* fabricEvents(brain: CliBrain, prompt: string, signal: AbortSignal): AsyncGenerator<AgentEvent> {
  const [{ SubscriptionFabricLanguageModel }, { Discovery }] = await Promise.all([
    import('@/runtime/providers/fabric/language-model'),
    import('@/runtime/providers/discovery/index'),
  ])
  const discovered = Discovery.last().find(p => p.id === brain.provider)
  const fabricProvider = String(discovered?.options?.fabricProvider ?? brain.provider.replace(/^subs-/, ''))
  const model = new SubscriptionFabricLanguageModel(brain.provider, fabricProvider, brain.modelId)
  const { stream } = await model.doStream({
    prompt: [{ role: 'user', content: [{ type: 'text', text: prompt }] }],
    abortSignal: signal,
  })
  const reader = stream.getReader()
  for (;;) {
    const { done, value } = await reader.read()
    if (done) return
    if (value.type === 'text-delta') yield { type: 'text_delta', delta: value.delta }
    else if (value.type === 'reasoning-delta') yield { type: 'reasoning_delta', delta: value.delta }
    else if (value.type === 'error') yield { type: 'error', error: value.error }
    else if (value.type === 'finish') {
      const u = value.usage
      yield {
        type: 'finish',
        finishReason: value.finishReason,
        usage: { inputTokens: u?.inputTokens ?? 0, outputTokens: u?.outputTokens ?? 0, totalTokens: u?.totalTokens ?? 0 },
      }
    }
  }
}

function errorText(error: unknown): string {
  if (error instanceof Error) return error.message
  if (typeof error === 'string') return error
  try {
    return JSON.stringify(error)
  } catch {
    return String(error)
  }
}

/** The plain-language failure for a CLI brain turn. */
export function cliBrainErrorMessage(brain: Pick<CliBrain, 'provider'>, raw: string, now?: Date): string {
  const spec = findSubprocessSpec(brain.provider)
  const help = describeProviderError({
    provider: brain.provider,
    providerName: spec?.name,
    cliBin: spec?.bin ?? brain.provider.replace(/-cli$/, ''),
    text: raw,
    alternative: subscriptionAlternative(brain.provider),
    now,
  })
  // The CLI's own words stay visible: a usage-limit reply ("resets 7:40pm")
  // is what the REPL's limit pause reads.
  const detail = raw.trim().slice(0, 400)
  if (!help) return `${spec?.name ?? brain.provider} failed: ${detail}`
  return `${formatProviderErrorHelp(help)}\n${spec?.name ?? brain.provider} said: ${detail}`
}

/**
 * One turn on a CLI brain as the TUI's message stream (the shape
 * queryModelWithStreaming yields): stream events for live text and thinking,
 * an assistant message per finished block, tool calls the CLI ran (marked
 * cliObserved) and their results as tool_result user messages.
 */
export async function* queryCliBrainWithStreaming({
  messages,
  signal,
  options,
}: {
  messages: Message[]
  signal: AbortSignal
  options: Options
}): AsyncGenerator<StreamEvent | AssistantMessage | SystemAPIErrorMessage | UserMessage, void> {
  const brain = cliBrainOf(options.model)!
  const main = isMainThread(options.querySource)
  const subagent = options.querySource?.startsWith('agent:') ?? false
  const prompt = turnText(messages)
  if (!prompt) {
    yield createAssistantMessage({ content: '' })
    return
  }
  // The main thread resumes the CLI's own session; a subagent starts fresh
  // with tools; side calls (recap, titles, compaction) get text only.
  const turn = {
    resumeKey: main ? `${getSessionId()}:${brain.provider}` : undefined,
    toolsOff: !main && !subagent,
  }
  const events = brain.fabric ? fabricEvents(brain, prompt, signal) : driverEvents(brain, prompt, signal, options, turn)

  const stamp = (m: AssistantMessage, usage?: { input_tokens: number; output_tokens: number }) => {
    m.message.model = options.model
    if (usage) m.message.usage = { ...m.message.usage, ...usage }
    return m
  }
  let started = false
  let index = 0
  let open: { kind: 'text' | 'thinking'; index: number; text: string } | null = null
  let yieldedAny = false
  let pending: AssistantMessage | null = null
  let failure: string | undefined
  let usage: { input_tokens: number; output_tokens: number } | undefined
  let contextUsed: number | undefined
  const toolMessages = new Map<string, string>()

  function* start() {
    if (started) return
    started = true
    yield { type: 'stream_event', event: { type: 'message_start', message: { usage: { input_tokens: 0, output_tokens: 0 } } } }
  }
  function* close() {
    if (!open) return
    const block = open
    open = null
    yield { type: 'stream_event', event: { type: 'content_block_stop', index: block.index } }
    if (block.kind === 'text' && block.text) {
      yield* flush()
      // Held until the next block or the end, so the turn's usage lands on it.
      pending = stamp(createAssistantMessage({ content: [{ type: 'text', text: block.text }] }))
    }
  }
  function* flush() {
    if (!pending) return
    yieldedAny = true
    yield pending
    pending = null
  }
  function* delta(kind: 'text' | 'thinking', text: string) {
    yield* start()
    if (open?.kind !== kind) {
      yield* close()
      open = { kind, index: index++, text: '' }
      yield {
        type: 'stream_event',
        event: {
          type: 'content_block_start',
          index: open.index,
          content_block: kind === 'text' ? { type: 'text', text: '' } : { type: 'thinking', thinking: '' },
        },
      }
    }
    open.text += text
    yield {
      type: 'stream_event',
      event: {
        type: 'content_block_delta',
        index: open.index,
        delta: kind === 'text' ? { type: 'text_delta', text } : { type: 'thinking_delta', thinking: text },
      },
    }
  }

  try {
    for await (const event of events) {
      if (event.type === 'text_delta') yield* delta('text', event.delta)
      else if (event.type === 'reasoning_delta') yield* delta('thinking', event.delta)
      else if (event.type === 'tool_call') {
        yield* start()
        yield* close()
        yield* flush()
        const name = TOOL_ALIASES[event.name] ?? event.name
        const message = stamp(
          createAssistantMessage({
            content: [{ type: 'tool_use', id: event.id, name, input: tuiToolInput(event.name, event.arguments) }],
          }),
        )
        message.cliObserved = true
        toolMessages.set(event.id, message.uuid)
        yieldedAny = true
        yield message
      } else if (event.type === 'tool_result') {
        yield createUserMessage({
          content: [{ type: 'tool_result', tool_use_id: event.id, content: event.content, is_error: event.isError || undefined }],
          toolUseResult: event.isError ? event.content : undefined,
          sourceToolAssistantUUID: toolMessages.get(event.id),
        })
      } else if (event.type === 'error') {
        failure ??= errorText(event.error)
      } else if (event.type === 'context') {
        // The CLI's own count of what its context holds: what the footer's context meter shows.
        contextUsed = event.used
      } else if (event.type === 'finish') {
        if (event.usage) usage = { input_tokens: event.usage.inputTokens, output_tokens: event.usage.outputTokens }
        if (event.finishReason === 'error') failure ??= 'the CLI ended the turn with an error'
      } else if (event.type === 'status' && event.status === 'failed') {
        failure ??= 'the CLI turn failed'
      }
    }
  } catch (error) {
    if (!signal.aborted) failure ??= errorText(error)
  }
  yield* close()
  if (signal.aborted) {
    yield* flush()
    return
  }

  if (failure) {
    yield* flush()
    logForDebugging(`[CliBrain] ${brain.provider} failed: ${failure}`, { level: 'error' })
    yield createAssistantAPIErrorMessage({
      content: cliBrainErrorMessage(brain, failure),
      apiError: 'cli_brain_error',
      error: 'unknown',
    })
    return
  }
  // The turn's usage goes on its last reply (the REPL's token counts read it).
  if (usage && contextUsed) usage = { ...usage, input_tokens: Math.max(usage.input_tokens, contextUsed - usage.output_tokens) }
  if (pending && usage) stamp(pending, usage)
  if (pending) yield* flush()
  else if (!yieldedAny) yield stamp(createAssistantMessage({ content: '' }), usage)
}

/**
 * One side call (classifier, titles, session search…) on a CLI brain: a
 * text-only CLI turn, returned in the Messages API shape sideQuery's callers
 * read. A CLI has no tool_choice, so callers that need a forced tool call get
 * text and fall back as they do for any reply without one.
 */
export async function sideQueryCliBrain(opts: {
  model: string
  system: Array<{ type: 'text'; text: string }>
  messages: ReadonlyArray<{ role: 'user' | 'assistant'; content: unknown }>
  signal?: AbortSignal
}) {
  const flatten = (content: unknown) =>
    typeof content === 'string'
      ? content
      : Array.isArray(content)
        ? content.filter(b => b?.type === 'text').map(b => String(b.text ?? '')).join('\n')
        : ''
  const text = [
    ...opts.system.map(b => b.text),
    ...opts.messages.map(m => (m.role === 'assistant' ? `(assistant) ${flatten(m.content)}` : flatten(m.content))),
  ]
    .filter(Boolean)
    .join('\n\n')
  const messages = [createUserMessage({ content: text })]
  const signal = opts.signal ?? new AbortController().signal
  let reply = ''
  let usage = { input_tokens: 0, output_tokens: 0 }
  for await (const m of queryCliBrainWithStreaming({
    messages,
    signal,
    options: { model: opts.model, querySource: 'side_query', getToolPermissionContext: async () => undefined } as Options,
  })) {
    if (m.type !== 'assistant') continue
    if (m.isApiErrorMessage) throw new Error(flatten(m.message.content))
    reply += flatten(m.message.content)
    if (m.message.usage) usage = { input_tokens: m.message.usage.input_tokens ?? 0, output_tokens: m.message.usage.output_tokens ?? 0 }
  }
  return {
    id: `msg_${randomUUID()}`,
    type: 'message',
    role: 'assistant',
    model: opts.model,
    content: [{ type: 'text', text: reply, citations: null }],
    stop_reason: 'end_turn',
    stop_sequence: null,
    usage: { ...usage, cache_creation_input_tokens: 0, cache_read_input_tokens: 0 },
  }
}

/**
 * A model on the user's own subscription to offer when a Cloud or API-key
 * model fails ("/model claude-cli/claude-sonnet-5 (your Claude Code plan)"):
 * the first installed CLI brain, its Sonnet when it has one.
 */
export function subscriptionAlternative(exclude?: string): { model: string; label: string } | undefined {
  try {
    const { Discovery } = require('../../../../../runtime/providers/discovery/index.js') as typeof import('../../../../../runtime/providers/discovery/index.js')
    const brains = Discovery.last().filter(p => p.source === 'subprocess' && p.id !== exclude && p.models.length > 0)
    const pick = brains.find(p => p.id === 'claude-cli') ?? brains[0]
    if (!pick) return undefined
    const model = pick.models.find(m => /sonnet/i.test(m.id)) ?? pick.models[0]!
    return { model: `${pick.id}/${model.id}`, label: `your ${pick.name} plan` }
  } catch {
    return undefined
  }
}
