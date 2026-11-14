/**
 * PersonPlanLanguageModel — the engine's model for `plan/…`, `copilot-plan/…`
 * and `chatgpt/…` (gizzi serve, Desktop's runtime, `gizzi run`).
 *
 * Chat only: tools, tool calls and tool results are never sent; system text
 * is dropped too (gizzi's agent prompt describes tools these models cannot
 * use). A turn runs only when the request carries `x-allternit-turn: person`,
 * which session/llm.ts sets only for a user message a chat bridge marked as
 * the person's own composer send (`metadata.turn: "person"`). Bots,
 * routines, missions, sub-agents and headless runs never carry it, so their
 * turns end with the plain refusal instead of reaching the plan.
 */
import type { LanguageModelV2, LanguageModelV2StreamPart } from "@ai-sdk/provider"
import { PERSON_TURN_HEADER, refusal } from "./rules"
import { personTurn } from "./turn"
import type { ChatMessage } from "./stream"

/** Text-only history from an AI SDK prompt: user and assistant text, nothing else. */
export function chatMessagesFromPrompt(prompt: any[]): ChatMessage[] {
  const out: ChatMessage[] = []
  for (const m of prompt ?? []) {
    if (m?.role !== "user" && m?.role !== "assistant") continue
    const text = Array.isArray(m.content)
      ? m.content
          .filter((p: any) => p?.type === "text" && typeof p.text === "string")
          .map((p: any) => p.text)
          .join("")
      : typeof m.content === "string"
        ? m.content
        : ""
    if (!text) continue
    const last = out[out.length - 1]
    if (last && last.role === m.role) last.content += `\n\n${text}`
    else out.push({ role: m.role, content: text })
  }
  return out
}

export class PersonPlanLanguageModel implements LanguageModelV2 {
  readonly specificationVersion = "v2" as const
  readonly provider = "person-plan"
  readonly supportedUrls: Record<string, RegExp[]> = {}

  constructor(
    private readonly providerID: string,
    readonly modelId: string,
  ) {}

  async doGenerate(options: any): Promise<any> {
    const { stream } = await this.doStream(options)
    const reader = stream.getReader()
    const chunks: string[] = []
    let usage = { inputTokens: 0, outputTokens: 0, totalTokens: 0 }
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      if (value.type === "text-delta") chunks.push(value.delta)
      if (value.type === "error") throw value.error
      if (value.type === "finish") usage = value.usage as typeof usage
    }
    return {
      content: [{ type: "text", text: chunks.join("") }],
      finishReason: "stop" as const,
      usage,
      warnings: [],
      response: { id: "person-plan", timestamp: new Date(), modelId: this.modelId },
    }
  }

  async doStream(options: any): Promise<{
    stream: ReadableStream<LanguageModelV2StreamPart>
    rawCall: { rawPrompt: unknown; rawSettings: Record<string, unknown> }
  }> {
    const headers: Record<string, string | undefined> = options?.headers ?? {}
    const person = headers[PERSON_TURN_HEADER] === "person"
    const ref = `${this.providerID}/${this.modelId}`
    const messages = chatMessagesFromPrompt(options.prompt ?? [])
    const signal: AbortSignal | undefined = options.abortSignal
    const providerID = this.providerID
    const modelID = this.modelId

    const stream = new ReadableStream<LanguageModelV2StreamPart>({
      async start(controller) {
        controller.enqueue({ type: "stream-start", warnings: [] })
        let open = false
        const fail = (message: string) => {
          controller.enqueue({ type: "error", error: new Error(message) })
          controller.enqueue({
            type: "finish",
            finishReason: "error",
            usage: { inputTokens: 0, outputTokens: 0, totalTokens: 0 },
          } as LanguageModelV2StreamPart)
          controller.close()
        }
        if (!person) return fail(refusal(ref, "helper"))
        if (!messages.some((m) => m.role === "user")) return fail("Nothing to send: the message has no text.")
        try {
          for await (const ev of personTurn({ providerID, modelID, messages, signal })) {
            if (ev.type === "delta") {
              if (!open) {
                open = true
                controller.enqueue({ type: "text-start", id: "text-1" })
              }
              controller.enqueue({ type: "text-delta", id: "text-1", delta: ev.text })
              continue
            }
            if (open) controller.enqueue({ type: "text-end", id: "text-1" })
            open = false
            if (ev.type === "error") return fail(ev.message)
            const inputTokens = ev.inputTokens ?? 0
            const outputTokens = ev.outputTokens ?? 0
            controller.enqueue({
              type: "finish",
              finishReason: ev.incomplete ? "length" : "stop",
              usage: { inputTokens, outputTokens, totalTokens: inputTokens + outputTokens },
            } as LanguageModelV2StreamPart)
            controller.close()
            return
          }
        } catch (e) {
          if (open) controller.enqueue({ type: "text-end", id: "text-1" })
          if (signal?.aborted) {
            controller.close()
            return
          }
          return fail(e instanceof Error ? e.message : String(e))
        }
      },
    })
    return { stream, rawCall: { rawPrompt: messages, rawSettings: {} } }
  }
}
