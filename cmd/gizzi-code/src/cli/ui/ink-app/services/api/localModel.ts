// @ts-nocheck
/**
 * Local provider streaming client for gizzi-code.
 *
 * Routes provider-prefixed models (e.g. local-mlx/qwen3.6-35b-a3b-4bit) to the
 * OpenAI-compatible endpoint configured in ~/.config/gizzi-code/gizzi.json.
 */
import { randomUUID } from 'crypto'
import { readFileSync } from 'fs'
import { homedir } from 'os'
import { join } from 'path'
import { LocalModelServer } from '@/runtime/local-model-server'
import { normalizeRoleAlternation } from '../../utils/localChatRoles.js'
import { logForDebugging } from '../../utils/debug.js'
import { asSystemPrompt, type SystemPrompt } from '../../utils/systemPromptType.js'
import { zodToJsonSchema } from '../../utils/zodToJsonSchema.js'
import type {
  AssistantMessage,
  Message,
  StreamEvent,
  SystemAPIErrorMessage,
} from '../../types/message.js'
import type { Options } from './claude.js'

// Stream index for the reasoning block: clear of choice and tool-call indexes.
const THINKING_BLOCK_INDEX = 1_000_000

type LocalProviderConfig = {
  baseURL: string
  apiKey?: string
  authType?: string
}

type ResolvedLocalModel = {
  provider: string
  modelId: string
  config: LocalProviderConfig
}

function getGizziConfigPath(): string {
  const configDir =
    process.env.GIZZI_CONFIG_DIR ?? join(homedir(), '.config', 'gizzi-code')
  return join(configDir, 'gizzi.json')
}

function readGizziConfig(): Record<string, unknown> | undefined {
  try {
    const raw = readFileSync(getGizziConfigPath(), 'utf-8')
    const parsed = JSON.parse(raw)
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      return parsed as Record<string, unknown>
    }
  } catch {
    // fall through
  }
  return undefined
}

export function getLocalProviderConfig(model: string): ResolvedLocalModel | undefined {
  const slashIndex = model.indexOf('/')
  if (slashIndex <= 0 || slashIndex === model.length - 1) {
    return undefined
  }
  const provider = model.slice(0, slashIndex)
  const modelId = model.slice(slashIndex + 1)

  const cfg = readGizziConfig()
  const providers = cfg?.provider as Record<string, unknown> | undefined
  const providerCfg = providers?.[provider] as Record<string, unknown> | undefined
  if (!providerCfg) {
    return undefined
  }

  const options = (providerCfg.options ?? {}) as Record<string, unknown>
  const baseURL =
    (options.baseURL as string | undefined) ??
    (providerCfg.baseURL as string | undefined)
  if (!baseURL || typeof baseURL !== 'string') {
    return undefined
  }

  const envKey = `${provider.toUpperCase().replace(/[^A-Z0-9]/g, '_')}_API_KEY`
  const apiKey =
    (options.apiKey as string | undefined) ??
    (providerCfg.apiKey as string | undefined) ??
    process.env[envKey]

  return {
    provider,
    modelId,
    config: {
      baseURL: baseURL.replace(/\/$/, ''),
      apiKey: typeof apiKey === 'string' ? apiKey : undefined,
      authType:
        (providerCfg.auth_type as string | undefined) ??
        (options.auth_type as string | undefined),
    },
  }
}

export function isLocalProviderModel(model: string): boolean {
  return getLocalProviderConfig(model) !== undefined
}

function flattenContentToText(
  content: string | Array<{ type?: string; text?: string; [key: string]: unknown }> | undefined,
): string {
  if (typeof content === 'string') {
    return content
  }
  if (!Array.isArray(content)) {
    return ''
  }
  return content
    .filter(block => block && typeof block === 'object' && block.type === 'text')
    .map(block => block.text ?? '')
    .join('\n')
}

function internalMessagesToOpenAI(messages: Message[]): Array<{
  role: string
  content?: string | null
  tool_call_id?: string
  tool_calls?: unknown[]
}> {
  const out: Array<{
    role: string
    content?: string | null
    tool_call_id?: string
    tool_calls?: unknown[]
  }> = []

  for (const msg of messages) {
    const role = msg.type
    const content = msg.message?.content

    if (role === 'assistant') {
      const textParts: string[] = []
      const toolCalls: unknown[] = []
      if (Array.isArray(content)) {
        for (const block of content) {
          if (!block || typeof block !== 'object') continue
          if (block.type === 'text') {
            textParts.push(String(block.text ?? ''))
          } else if (block.type === 'tool_use') {
            toolCalls.push({
              id: String(block.id ?? `local-${Date.now()}`),
              type: 'function',
              function: {
                name: String(block.name ?? 'unknown'),
                arguments:
                  typeof block.input === 'object' && block.input !== null
                    ? JSON.stringify(block.input)
                    : String(block.input ?? '{}'),
              },
            })
          }
        }
      } else if (typeof content === 'string') {
        textParts.push(content)
      }
      const assistant: {
        role: string
        content?: string
        tool_calls?: unknown[]
      } = {
        role: 'assistant',
        content: textParts.join('\n') || '',
      }
      if (toolCalls.length > 0) {
        assistant.tool_calls = toolCalls
      }
      out.push(assistant)
      continue
    }

    if (role === 'user') {
      const textParts: string[] = []
      const toolResults: Array<{ content: string; tool_use_id: string }> = []
      if (Array.isArray(content)) {
        for (const block of content) {
          if (!block || typeof block !== 'object') continue
          if (block.type === 'text') {
            textParts.push(String(block.text ?? ''))
          } else if (block.type === 'tool_result') {
            const resultContent =
              typeof block.content === 'string'
                ? block.content
                : JSON.stringify(block.content ?? '')
            toolResults.push({
              content: resultContent,
              tool_use_id: String(block.tool_use_id ?? block.toolUseId ?? ''),
            })
          }
        }
      } else if (typeof content === 'string') {
        textParts.push(content)
      }
      if (textParts.length > 0) {
        out.push({ role: 'user', content: textParts.join('\n') })
      }
      for (const tr of toolResults) {
        out.push({ role: 'tool', content: tr.content, tool_call_id: tr.tool_use_id })
      }
      continue
    }

    // Non-conversational entries (attachments, tombstones, etc.) have no
    // valid OpenAI role. Skip empty ones and fold the rest into 'user' —
    // passing msg.type through as the role makes strict servers (e.g.
    // mlx_lm.server) reject the request with "Unexpected message role".
    const text = flattenContentToText(content)
    if (!text.trim()) {
      continue
    }
    out.push({ role: role === 'system' ? 'system' : 'user', content: text })
  }

  return out
}

function toolsToOpenAI(
  tools: Options['tools'],
): Array<{ type: string; function: Record<string, unknown> }> {
  return tools.map(t => {
    const parameters =
      t.inputJSONSchema ??
      (t.inputSchema ? zodToJsonSchema(t.inputSchema) : undefined) ??
      { type: 'object', properties: {} }
    return {
      type: 'function',
      function: {
        name: t.name,
        description: t.description ?? '',
        parameters,
      },
    }
  })
}

export async function* queryLocalModelWithStreaming({
  messages,
  systemPrompt,
  tools,
  signal,
  options,
}: {
  messages: Message[]
  systemPrompt: SystemPrompt
  tools: Options['tools']
  signal: AbortSignal
  options: Options
}): AsyncGenerator<StreamEvent | AssistantMessage | SystemAPIErrorMessage, void> {
  const { createAssistantAPIErrorMessage } = await import(
    '../../utils/messages.js'
  )
  const resolved = getLocalProviderConfig(options.model)
  if (!resolved) {
    yield createAssistantAPIErrorMessage({
      content: `Model '${options.model}' is not configured as a local provider. Add it to ~/.config/gizzi-code/gizzi.json.`,
      apiError: 'local_model_config_missing',
      error: 'local_model_config_missing',
    })
    return
  }

  const { provider, modelId, config } = resolved
  const baseURL = config.baseURL

  // Spawn/adopt/kill the per-model local server when this model opts in via
  // options.modelPath (e.g. local-mlx MLX models). Unmanaged models pass
  // through to the existing bring-your-own-server behavior.
  try {
    const fullCfg = readGizziConfig()
    const providerBlock = (fullCfg?.provider as Record<string, unknown> | undefined)?.[provider] as
      | { options?: Record<string, unknown>; models?: Record<string, { options?: Record<string, unknown> }> }
      | undefined
    await LocalModelServer.ensure({ providerID: provider, modelID: modelId }, providerBlock)
  } catch (ensureErr) {
    yield createAssistantAPIErrorMessage({
      content: `Local model server error: ${ensureErr instanceof Error ? ensureErr.message : String(ensureErr)}`,
      apiError: 'local_model_server_error',
      error: 'local_model_server_error',
    })
    return
  }

  logForDebugging(
    `[LocalModel] streaming ${provider}/${modelId} via ${baseURL}/chat/completions`,
  )

  // Resolve the real model id served by the local endpoint. Servers like
  // mlx_lm.server expose whatever path/name they were started with, and will
  // try to download from HuggingFace if we send a bare short id.
  let resolvedModelId = modelId
  try {
    const modelsRes = await fetch(`${baseURL}/models`, {
      method: 'GET',
      headers: config.apiKey ? { Authorization: `Bearer ${config.apiKey}` } : {},
      signal,
    })
    if (modelsRes.ok) {
      const modelsJson = (await modelsRes.json()) as { data?: { id: string }[] }
      // mlx_lm.server lists every mlx-compatible model in the whole HF
      // cache, not just the loaded one — the actually-running --model only
      // appears as an absolute-path entry. Sending data[0] makes the server
      // try to download a random cached model from HuggingFace, which fails
      // offline with "Unable to connect. Is the computer able to access the
      // url?" (503). Prefer the absolute-path entry; then an exact/basename
      // match for the configured id; data[0] only as last resort.
      const list = modelsJson.data ?? []
      const served =
        list.find((m) => m.id?.startsWith('/'))?.id ??
        list.find((m) => m.id === modelId || m.id?.endsWith('/' + modelId))?.id ??
        list[0]?.id
      if (served) {
        resolvedModelId = served
      }
    }
  } catch {
    // keep configured modelId as fallback
  }

  // Normalize to strict user/assistant alternation: local templates like
  // Gemma3 reject system/tool roles ("Conversation roles must alternate...").
  const openaiMessages = normalizeRoleAlternation([
    { role: 'system', content: asSystemPrompt(systemPrompt).join('\n\n') },
    ...internalMessagesToOpenAI(messages),
  ])

  const body: Record<string, unknown> = {
    model: resolvedModelId,
    messages: openaiMessages,
    stream: true,
    // Ask the server for a final usage chunk so the UI can show real
    // context-window consumption (mlx_lm.server only emits it when asked).
    stream_options: { include_usage: true },
    temperature: options.temperatureOverride ?? 0.7,
  }

  if (tools && tools.length > 0) {
    body.tools = toolsToOpenAI(tools)
  }

  if (options.maxOutputTokensOverride) {
    body.max_tokens = options.maxOutputTokensOverride
  }

  try {
    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
    }
    if (config.apiKey && config.authType !== 'none') {
      headers.Authorization = `Bearer ${config.apiKey}`
    }

    const response = await fetch(`${baseURL}/chat/completions`, {
      method: 'POST',
      headers,
      body: JSON.stringify(body),
      signal,
    })

    if (!response.ok) {
      const text = await response.text().catch(() => '')
      throw new Error(`Local model error ${response.status}: ${text}`)
    }

    const reader = response.body?.getReader()
    if (!reader) {
      throw new Error('Local model response has no readable body')
    }

    const decoder = new TextDecoder()
    let buffer = ''
    const contentBlocks = new Map<
      number,
      | { type: 'text'; text: string }
      | { type: 'tool_use'; id: string; name: string; input: string }
    >()
    let usage = { input_tokens: 0, output_tokens: 0 }
    let firstChunk = true
    const start = Date.now()
    // Reasoning (OpenRouter `reasoning`, vLLM/DeepSeek `reasoning_content`)
    // streams as a thinking block, so the spinner shows "thinking" and counts
    // its tokens the way Claude Code does. It never becomes reply text: some
    // servers send `content: ""` beside it, others `content: null`.
    let thinkingOpen = false
    function* closeThinking() {
      if (!thinkingOpen) return
      thinkingOpen = false
      yield {
        type: 'stream_event',
        event: { type: 'content_block_stop', index: THINKING_BLOCK_INDEX },
      }
    }
    let ttftMs: number | undefined

    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      buffer += decoder.decode(value, { stream: true })
      const lines = buffer.split('\n')
      buffer = lines.pop() ?? ''

      for (const line of lines) {
        const trimmed = line.trim()
        if (!trimmed || trimmed === ':') continue
        if (!trimmed.startsWith('data: ')) continue

        const data = trimmed.slice(6)
        if (data === '[DONE]') continue

        try {
          const json = JSON.parse(data)
          const choice = json.choices?.[0]
          const delta = choice?.delta ?? {}
          const index = choice?.index ?? 0

          if (json.usage) {
            usage = {
              input_tokens: json.usage.prompt_tokens ?? usage.input_tokens,
              output_tokens:
                json.usage.completion_tokens ?? usage.output_tokens,
            }
          }

          if (firstChunk) {
            ttftMs = Date.now() - start
            firstChunk = false
            yield {
              type: 'stream_event',
              event: { type: 'message_start', message: { usage } },
              ttftMs,
            }
          }

          const reasoningDelta = delta.reasoning ?? delta.reasoning_content
          if (typeof reasoningDelta === 'string' && reasoningDelta) {
            if (!thinkingOpen) {
              thinkingOpen = true
              yield {
                type: 'stream_event',
                event: {
                  type: 'content_block_start',
                  index: THINKING_BLOCK_INDEX,
                  content_block: { type: 'thinking', thinking: '' },
                },
              }
            }
            yield {
              type: 'stream_event',
              event: {
                type: 'content_block_delta',
                index: THINKING_BLOCK_INDEX,
                delta: { type: 'thinking_delta', thinking: reasoningDelta },
              },
            }
          }

          const textDelta = delta.content
          if (textDelta) {
            yield* closeThinking()
            let block = contentBlocks.get(index)
            if (!block || block.type !== 'text') {
              block = { type: 'text', text: '' }
              contentBlocks.set(index, block)
              yield {
                type: 'stream_event',
                event: {
                  type: 'content_block_start',
                  index,
                  content_block: { type: 'text', text: '' },
                },
              }
            }
            const text = String(textDelta)
            block.text += text
            yield {
              type: 'stream_event',
              event: {
                type: 'content_block_delta',
                index,
                delta: { type: 'text_delta', text },
              },
            }
          }

          if (Array.isArray(delta.tool_calls)) {
            if (delta.tool_calls.length > 0) yield* closeThinking()
            for (const tc of delta.tool_calls) {
              const idx = tc.index ?? index
              let block = contentBlocks.get(idx)
              if (!block || block.type !== 'tool_use') {
                block = {
                  type: 'tool_use',
                  id: String(tc.id ?? `local-tool-${idx}`),
                  name: String(tc.function?.name ?? 'unknown'),
                  input: '',
                }
                contentBlocks.set(idx, block)
                yield {
                  type: 'stream_event',
                  event: {
                    type: 'content_block_start',
                    index: idx,
                    content_block: {
                      type: 'tool_use',
                      id: block.id,
                      name: block.name,
                      input: {},
                    },
                  },
                }
              }
              const argDelta = tc.function?.arguments
              if (argDelta) {
                const partial = String(argDelta)
                block.input += partial
                yield {
                  type: 'stream_event',
                  event: {
                    type: 'content_block_delta',
                    index: idx,
                    delta: { type: 'input_json_delta', partial_json: partial },
                  },
                }
              }
            }
          }
        } catch (parseErr) {
          logForDebugging(`[LocalModel] SSE parse error: ${parseErr}`)
        }
      }
    }

    yield* closeThinking()
    for (const [index] of contentBlocks) {
      yield {
        type: 'stream_event',
        event: { type: 'content_block_stop', index },
      }
    }

    const assistantContent: Array<{
      type: string
      text?: string
      id?: string
      name?: string
      input?: Record<string, unknown>
    }> = []
    for (const block of contentBlocks.values()) {
      if (block.type === 'text') {
        assistantContent.push({ type: 'text', text: block.text })
      } else {
        let input: Record<string, unknown> = {}
        try {
          input = JSON.parse(block.input)
        } catch {
          input = { raw: block.input }
        }
        assistantContent.push({
          type: 'tool_use',
          id: block.id,
          name: block.name,
          input,
        })
      }
    }

    if (assistantContent.length === 0) {
      assistantContent.push({ type: 'text', text: '' })
    }

    const assistantMessage: AssistantMessage = {
      type: 'assistant',
      message: {
        role: 'assistant',
        content: assistantContent,
        usage,
        stop_reason: 'end_turn',
      },
      model: options.model,
      usage,
      requestId: `local-${Date.now()}`,
      uuid: randomUUID(),
      timestamp: new Date().toISOString(),
    }

    yield assistantMessage
  } catch (error) {
    logForDebugging(
      `[LocalModel] error: ${error instanceof Error ? error.message : String(error)}`,
      { level: 'error' },
    )
    yield createAssistantAPIErrorMessage({
      content: `Local model error: ${error instanceof Error ? error.message : String(error)}`,
      apiError: 'local_model_error',
      error: 'local_model_error',
    })
  }
}
