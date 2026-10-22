/**
 * Office AI transport — streams the Allternit platform's agent-chat endpoint
 * (`POST /api/agent-chat`, SSE) and normalizes events into the chunk shape the
 * vendored GenOffice apps expect (delta / tool-call / tool-result / done /
 * error). Two SSE dialects are accepted: the legacy `{chunk_type, chunk}`
 * lines and the gateway's `{type: content_block_delta | content_block_start |
 * tool_result | tool_error | error | finish, ...}` events
 * (cmd/allternit-api/src/gizzi_chat_stream.rs).
 *
 * In the browser the call is same-origin; in the desktop the surface's
 * `allternit-api://` protocol redirect carries auth automatically.
 */

export interface OfficeAiChunk {
  type: 'delta' | 'tool-call' | 'tool-result' | 'done' | 'error' | 'ping'
  text?: string
  error?: string
  toolCall?: {
    toolCallId: string
    toolName: string
    input?: unknown
  }
  toolResult?: {
    toolCallId?: string | undefined
    toolName?: string | undefined
    result?: unknown
  }
}

export interface OfficeAiMessage {
  role: 'system' | 'user' | 'assistant'
  content: string
}

export interface StreamOfficeAiOptions {
  messages: OfficeAiMessage[]
  /** Optional model override; defaults to the platform's selected model. */
  modelId?: string
  signal?: AbortSignal
  onChunk: (chunk: OfficeAiChunk) => void
}

const AGENT_CHAT_URL = '/api/agent-chat'

export type OfficeAiTokenProvider = () => Promise<string | null | undefined> | string | null | undefined

let tokenProvider: OfficeAiTokenProvider | null = null

/**
 * Lets a host attach a bearer token (the standalone office site passes its Clerk
 * session token) to agent-chat calls. Unset, requests carry no Authorization
 * header, as before.
 */
export function setOfficeAiTokenProvider(provider: OfficeAiTokenProvider | null): void {
  tokenProvider = provider
}

function flattenMessages(messages: OfficeAiMessage[]): string {
  // The agent-chat endpoint takes a single user message (system content rides
  // separately as `systemPrompt`, which the gateway appends to the runtime's
  // system prompt); fold the short office conversations into one instruction
  // while keeping role framing explicit.
  return messages
    .filter((m) => m.role !== 'system')
    .map((m) => `${m.role === 'user' ? 'User' : 'Assistant'}: ${m.content}`)
    .join('\n\n')
}

function systemPromptOf(messages: OfficeAiMessage[]): string {
  return messages
    .filter((m) => m.role === 'system')
    .map((m) => m.content)
    .join('\n\n')
}

function tryParsePayload(value: unknown): unknown {
  if (typeof value !== 'string') return value
  const trimmed = value.trim()
  if (!trimmed) return value
  try {
    return JSON.parse(trimmed)
  } catch {
    return value
  }
}

/**
 * Normalize one gateway event (the `{type: ...}` SSE dialect emitted by
 * gizzi_chat_stream.rs) into OfficeAiChunks. Returns true when the stream is
 * finished (a `finish` event) and the reader should stop.
 */
function handleGatewayEvent(
  event: Record<string, unknown>,
  onChunk: (chunk: OfficeAiChunk) => void,
): boolean {
  switch (event.type) {
    case 'content_block_delta': {
      const delta = event.delta as { type?: string; text?: string } | undefined
      if (delta?.type === 'text_delta' && typeof delta.text === 'string' && delta.text) {
        onChunk({ type: 'delta', text: delta.text })
      }
      return false
    }
    case 'content_block_start': {
      const block = event.content_block as
        | { id?: string; type?: string; name?: string; input?: unknown }
        | undefined
      if (block?.type === 'tool_use') {
        onChunk({
          type: 'tool-call',
          toolCall: {
            toolCallId: block.id ?? `${block.name ?? 'tool'}-${Date.now()}`,
            toolName: block.name ?? 'Tool',
            input: block.input,
          },
        })
      }
      return false
    }
    case 'tool_result':
      onChunk({
        type: 'tool-result',
        toolResult: {
          toolCallId: event.toolCallId as string | undefined,
          toolName: event.toolName as string | undefined,
          result: event.result,
        },
      })
      return false
    case 'tool_error':
      onChunk({
        type: 'tool-result',
        toolResult: {
          toolCallId: event.toolCallId as string | undefined,
          toolName: event.toolName as string | undefined,
          result: event.error,
        },
      })
      return false
    case 'error':
      onChunk({ type: 'error', error: String(event.error ?? 'unknown error') })
      return false
    case 'finish':
      onChunk({ type: 'done' })
      return true
    default:
      // message_start and anything else carry no payload for the office apps
      return false
  }
}

export async function streamOfficeAi(options: StreamOfficeAiOptions): Promise<void> {
  const { messages, modelId, signal, onChunk } = options
  const chatId = globalThis.crypto?.randomUUID?.() ?? `office-${Date.now()}`
  const systemPrompt = systemPromptOf(messages)

  const headers: Record<string, string> = { 'Content-Type': 'application/json' }
  const token = tokenProvider ? await tokenProvider() : null
  if (token) headers.Authorization = `Bearer ${token}`

  const response = await fetch(AGENT_CHAT_URL, {
    method: 'POST',
    headers,
    body: JSON.stringify({
      chatId,
      message: flattenMessages(messages),
      ...(systemPrompt ? { systemPrompt } : {}),
      ...(modelId ? { runtimeModelId: modelId } : {}),
    }),
    signal: signal ?? null,
  })

  if (!response.ok) {
    onChunk({ type: 'error', error: `chat failed (${response.status})` })
    onChunk({ type: 'done' })
    return
  }
  if (!response.body) {
    onChunk({ type: 'error', error: 'chat failed (no response body)' })
    onChunk({ type: 'done' })
    return
  }

  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''

  try {
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break

      buffer += decoder.decode(value, { stream: true })
      const lines = buffer.split('\n')
      buffer = lines.pop() ?? ''

      for (const line of lines) {
        if (!line.trim() || !line.startsWith('data: ')) continue
        const data = line.slice(6)
        if (data === '[DONE]') continue

        let parsed: Record<string, unknown>
        try {
          parsed = JSON.parse(data) as Record<string, unknown>
        } catch {
          continue
        }

        if (typeof parsed.chunk_type !== 'string' && typeof parsed.type === 'string') {
          // Gateway dialect (cmd/allternit-api/src/gizzi_chat_stream.rs):
          // {type: message_start | content_block_delta | content_block_start |
          // tool_result | tool_error | error | finish, ...}
          if (handleGatewayEvent(parsed, onChunk)) return
          continue
        }

        const chunkType = typeof parsed.chunk_type === 'string' ? parsed.chunk_type : 'text'
        const payload = tryParsePayload(parsed.chunk)

        if (chunkType === 'tool_call') {
          const call = payload as { toolCallId?: string; toolName?: string; input?: unknown; args?: unknown } | string
          if (typeof call === 'object' && call) {
            onChunk({
              type: 'tool-call',
              toolCall: {
                toolCallId: call.toolCallId ?? `${call.toolName ?? 'tool'}-${Date.now()}`,
                toolName: call.toolName ?? 'Tool',
                input: call.input ?? call.args,
              },
            })
          }
        } else if (chunkType === 'tool_result') {
          const result = payload as { toolCallId?: string; toolName?: string; result?: unknown } | string
          if (typeof result === 'object' && result) {
            onChunk({
              type: 'tool-result',
              toolResult: {
                toolCallId: result.toolCallId,
                toolName: result.toolName,
                result: result.result,
              },
            })
          }
        } else if (chunkType === 'error') {
          onChunk({ type: 'error', error: String(payload ?? 'unknown error') })
        } else {
          const text = typeof payload === 'string' ? payload : ''
          if (text) onChunk({ type: 'delta', text })
        }
      }
    }
  } catch (err) {
    if ((err as Error).name !== 'AbortError') {
      onChunk({ type: 'error', error: (err as Error).message })
    }
  }

  onChunk({ type: 'done' })
}
