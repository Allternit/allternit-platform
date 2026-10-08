/**
 * Claude-native wire mapping for the computer toolset.
 *
 * gizzi registers ONE function tool per toolset (`computer`, `browser`) with
 * an `action` field. For Anthropic Messages requests this hook rewrites the
 * wire so Claude sees the native `computer_toolset_20260801` /
 * `browser_toolset_20260801` entries, and maps Claude's toolset tool_use
 * blocks (`name: "left_click", toolset_name: "computer"`) back to the function
 * call the AI SDK and the gizzi loop expect (`name: "computer", input:
 * {action: "left_click", ...}`). gizzi stays the only loop; this is purely a
 * request/response transform in the provider fetch.
 *
 * Fallback: when the API rejects the native entry (a model without the
 * toolset), the untouched function-tool request is sent instead and that
 * model is remembered for the process.
 */
import { CONTRACTS, type ToolsetName } from "./contract.gen"
import { TOOL_MARKER } from "./adapter"

const TOOLSET_TYPE: Record<ToolsetName, string> = {
  computer: CONTRACTS.computer.upstream.anthropic_type,
  browser: CONTRACTS.browser.upstream.anthropic_type,
}
const TAB_MEMBERS = new Set(["new_tab", "list_tabs", "switch_tab", "close_tab"])
export const BROWSER_STATE_OPEN = "<browser_state>"
export const BROWSER_STATE_CLOSE = "</browser_state>"

const nativeRejected = new Set<string>()

function ourToolset(tool: any): ToolsetName | undefined {
  if (!tool || typeof tool !== "object" || typeof tool.description !== "string") return undefined
  for (const name of ["computer", "browser"] as const) {
    if (tool.name === name && tool.description.startsWith(TOOL_MARKER[name])) return name
  }
  return undefined
}

/** Split our `<browser_state>{json}</browser_state>` sentinel out of a tool result text. */
function liftBrowserState(blocks: any[]): any[] {
  const out: any[] = []
  for (const b of blocks) {
    if (b?.type === "text" && typeof b.text === "string" && b.text.includes(BROWSER_STATE_OPEN)) {
      const start = b.text.indexOf(BROWSER_STATE_OPEN)
      const end = b.text.indexOf(BROWSER_STATE_CLOSE, start)
      if (end > start) {
        try {
          const state = JSON.parse(b.text.slice(start + BROWSER_STATE_OPEN.length, end))
          const rest = (b.text.slice(0, start) + b.text.slice(end + BROWSER_STATE_CLOSE.length)).trim()
          if (rest) out.push({ ...b, text: rest })
          out.push({ type: "browser_state", ...state })
          continue
        } catch {
          // leave the text as-is
        }
      }
    }
    out.push(b)
  }
  return out
}

/**
 * Rewrite an Anthropic Messages request body for the native toolsets.
 * Returns undefined when the body has none of our tools (or the model was
 * found not to support them), so the caller sends it unchanged.
 */
export function toNativeRequest(body: any): any | undefined {
  if (!body || !Array.isArray(body.tools)) return undefined
  if (typeof body.model === "string" && nativeRejected.has(body.model)) return undefined
  const ours = new Map<string, ToolsetName>()
  for (const t of body.tools) {
    const ts = ourToolset(t)
    if (ts) ours.set(t.name, ts)
  }
  if (ours.size === 0) return undefined
  const out = structuredClone(body)
  out.tools = out.tools.map((t: any) => {
    const ts = ourToolset(t)
    if (!ts) return t
    const members = (t.input_schema?.properties?.action?.enum as string[] | undefined) ?? []
    const configs: Record<string, { enabled: boolean }> = {}
    for (const m of CONTRACTS[ts].members) {
      const on = members.includes(m.name)
      if (on !== m.default_enabled) configs[m.name] = { enabled: on }
    }
    return {
      type: TOOLSET_TYPE[ts],
      ...(Object.keys(configs).length ? { configs } : {}),
      ...(t.cache_control ? { cache_control: t.cache_control } : {}),
    }
  })
  // A forced tool choice can't name a toolset member set; let the model pick.
  if (out.tool_choice?.type === "tool" && ours.has(out.tool_choice.name)) out.tool_choice = { type: "any" }
  const callToolset = new Map<string, { toolset: ToolsetName; member: string }>()
  for (const msg of out.messages ?? []) {
    if (!Array.isArray(msg.content)) continue
    msg.content = msg.content.map((block: any) => {
      if (block?.type === "tool_use" && ours.has(block.name)) {
        const toolset = ours.get(block.name)!
        const { action, ...input } = (block.input ?? {}) as Record<string, unknown>
        callToolset.set(block.id, { toolset, member: String(action) })
        return { ...block, name: String(action), toolset_name: toolset, input }
      }
      if (block?.type === "tool_result" && callToolset.has(block.tool_use_id)) {
        const { toolset, member } = callToolset.get(block.tool_use_id)!
        let content = typeof block.content === "string" ? [{ type: "text", text: block.content }] : (block.content ?? [])
        if (toolset === "browser") {
          content = liftBrowserState(content)
          if (block.is_error) content = content.filter((c: any) => c.type !== "browser_state")
          else if (TAB_MEMBERS.has(member) && content.some((c: any) => c.type === "browser_state")) {
            content = content.filter((c: any) => c.type === "browser_state")
          }
        }
        return { ...block, toolset_name: toolset, content }
      }
      return block
    })
  }
  return out
}

/** Whether an error response means this model doesn't take the native toolset entry. */
export function isNativeRejection(status: number, text: string): boolean {
  return status === 400 && /toolset|computer_toolset_20260801|browser_toolset_20260801/i.test(text)
}

export function rememberRejected(model: string | undefined) {
  if (model) nativeRejected.add(model)
}

/** Map one native toolset tool_use block back to our function call. */
function toFunctionBlock(block: any) {
  if (block?.type !== "tool_use" || (block.toolset_name !== "computer" && block.toolset_name !== "browser")) return block
  const { toolset_name, name, input, ...rest } = block
  return { ...rest, name: toolset_name, input: { action: name, ...(input ?? {}) } }
}

/** Non-streaming response body. */
export function fromNativeResponse(body: any): any {
  if (!body || !Array.isArray(body.content)) return body
  return { ...body, content: body.content.map(toFunctionBlock) }
}

/**
 * Streaming (SSE) response: rename toolset tool_use blocks and fold their
 * input deltas into one delta that carries `action`, emitted just before the
 * block stops. Everything else passes through byte-for-byte.
 */
export function fromNativeStream(stream: ReadableStream<Uint8Array>): ReadableStream<Uint8Array> {
  const decoder = new TextDecoder()
  const encoder = new TextEncoder()
  const held = new Map<number, { member: string; json: string }>()
  let buffer = ""
  const rewrite = (event: string): string => {
    const dataLine = event.split("\n").find((l) => l.startsWith("data:"))
    if (!dataLine) return event
    let data: any
    try {
      data = JSON.parse(dataLine.slice(5).trim())
    } catch {
      return event
    }
    const frame = (d: any) => `event: ${d.type}\ndata: ${JSON.stringify(d)}`
    if (data.type === "content_block_start" && data.content_block?.toolset_name) {
      const block = data.content_block
      held.set(data.index, { member: block.name, json: "" })
      return frame({ ...data, content_block: { ...toFunctionBlock({ ...block, input: {} }), input: {} } })
    }
    if (data.type === "content_block_delta" && held.has(data.index) && data.delta?.type === "input_json_delta") {
      held.get(data.index)!.json += data.delta.partial_json ?? ""
      return ""
    }
    if (data.type === "content_block_stop" && held.has(data.index)) {
      const { member, json } = held.get(data.index)!
      held.delete(data.index)
      let input: Record<string, unknown> = {}
      try {
        input = json.trim() ? JSON.parse(json) : {}
      } catch {
        input = {}
      }
      const delta = {
        type: "content_block_delta",
        index: data.index,
        delta: { type: "input_json_delta", partial_json: JSON.stringify({ action: member, ...input }) },
      }
      return `${frame(delta)}\n\n${frame(data)}`
    }
    return event
  }
  return stream.pipeThrough(
    new TransformStream<Uint8Array, Uint8Array>({
      transform(chunk, controller) {
        buffer += decoder.decode(chunk, { stream: true })
        let idx: number
        while ((idx = buffer.indexOf("\n\n")) !== -1) {
          const event = buffer.slice(0, idx)
          buffer = buffer.slice(idx + 2)
          const out = rewrite(event)
          if (out) controller.enqueue(encoder.encode(`${out}\n\n`))
        }
      },
      flush(controller) {
        if (buffer) controller.enqueue(encoder.encode(rewrite(buffer)))
      },
    }),
  )
}

/**
 * Wrap a fetch call for an Anthropic Messages request. `send(body)` performs
 * the request with a (possibly rewritten) JSON body string.
 */
export async function nativeToolsetFetch(bodyText: string, send: (body: string) => Promise<Response>): Promise<Response> {
  let body: any
  try {
    body = JSON.parse(bodyText)
  } catch {
    return send(bodyText)
  }
  const native = toNativeRequest(body)
  if (!native) return send(bodyText)
  const res = await send(JSON.stringify(native))
  if (!res.ok) {
    const text = await res.clone().text().catch(() => "")
    if (isNativeRejection(res.status, text)) {
      rememberRejected(body.model)
      return send(bodyText)
    }
    return res
  }
  const type = res.headers.get("content-type") ?? ""
  // The body is re-encoded, so its length and transfer encoding change.
  const headers = new Headers(res.headers)
  headers.delete("content-length")
  headers.delete("content-encoding")
  if (type.includes("text/event-stream") && res.body) {
    return new Response(fromNativeStream(res.body), { status: res.status, statusText: res.statusText, headers })
  }
  if (type.includes("application/json")) {
    const json = await res.json()
    return new Response(JSON.stringify(fromNativeResponse(json)), { status: res.status, statusText: res.statusText, headers })
  }
  return res
}
