import { describe, expect, test } from "bun:test"
import {
  parseChatCompletionsStream,
  parseResponsesStream,
  responsesBody,
  type TurnEvent,
} from "../../../src/runtime/providers/person-plan/stream"

/** A body that arrives in the given chunks (split mid-line on purpose). */
function body(chunks: string[]): ReadableStream<Uint8Array> {
  const enc = new TextEncoder()
  return new ReadableStream({
    start(c) {
      for (const ch of chunks) c.enqueue(enc.encode(ch))
      c.close()
    },
  })
}

async function collect(gen: AsyncGenerator<TurnEvent>): Promise<TurnEvent[]> {
  const out: TurnEvent[] = []
  for await (const ev of gen) out.push(ev)
  return out
}

const sse = (event: string, data: unknown) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`

// Event order and fields as the Responses API streams them.
const created = sse("response.created", { type: "response.created", sequence_number: 0, response: { id: "resp_1", object: "response", status: "in_progress", model: "gpt-6.1-sol", output: [], usage: null } })
const itemAdded = sse("response.output_item.added", { type: "response.output_item.added", sequence_number: 1, output_index: 0, item: { id: "msg_1", type: "message", status: "in_progress", role: "assistant", content: [] } })
const partAdded = sse("response.content_part.added", { type: "response.content_part.added", sequence_number: 2, item_id: "msg_1", output_index: 0, content_index: 0, part: { type: "output_text", text: "", annotations: [] } })
const delta = (n: number, d: string) => sse("response.output_text.delta", { type: "response.output_text.delta", sequence_number: n, item_id: "msg_1", output_index: 0, content_index: 0, delta: d, logprobs: [] })
const textDone = sse("response.output_text.done", { type: "response.output_text.done", sequence_number: 5, item_id: "msg_1", output_index: 0, content_index: 0, text: "Hi there" })
const completed = sse("response.completed", {
  type: "response.completed",
  sequence_number: 6,
  response: { id: "resp_1", status: "completed", usage: { input_tokens: 12, input_tokens_details: { cached_tokens: 0 }, output_tokens: 3, total_tokens: 15 } },
})

describe("Responses stream", () => {
  test("text deltas then usage, across chunk boundaries", async () => {
    const all = created + itemAdded + partAdded + delta(3, "Hi") + delta(4, " there") + textDone + completed
    const cut = [all.slice(0, 37), all.slice(37, 400), all.slice(400, all.length - 20), all.slice(all.length - 20)]
    expect(await collect(parseResponsesStream(body(cut)))).toEqual([
      { type: "delta", text: "Hi" },
      { type: "delta", text: " there" },
      { type: "done", inputTokens: 12, outputTokens: 3 },
    ])
  })

  test("a usage limit after streaming started ends with the plain ChatGPT limit message", async () => {
    const failed = sse("response.failed", {
      type: "response.failed",
      sequence_number: 4,
      response: { id: "resp_1", status: "failed", error: { code: "subscription_sharing_usage_limit_exceeded", message: "Usage limit reached" } },
    })
    const events = await collect(parseResponsesStream(body([created, delta(3, "Hi"), failed])))
    expect(events[0]).toEqual({ type: "delta", text: "Hi" })
    expect(events[1]).toMatchObject({ type: "error", code: "subscription_sharing_usage_limit_exceeded" })
    expect((events[1] as { message: string }).message).toContain("ChatGPT limit reached")
  })

  test("incomplete is a finished turn with its reason; a cut stream is an error", async () => {
    const incomplete = sse("response.incomplete", { type: "response.incomplete", response: { status: "incomplete", incomplete_details: { reason: "max_output_tokens" }, usage: { input_tokens: 5, output_tokens: 9 } } })
    expect((await collect(parseResponsesStream(body([delta(1, "x"), incomplete]))))[1]).toEqual({
      type: "done",
      inputTokens: 5,
      outputTokens: 9,
      incomplete: "max_output_tokens",
    })
    const cut = await collect(parseResponsesStream(body([created, delta(3, "Hi")])))
    expect(cut[cut.length - 1]).toMatchObject({ type: "error", code: "stream_ended" })
  })

  test("the request body is chat only: store false, stream true, no tools", () => {
    const b = responsesBody("gpt-6.1-sol", [
      { role: "system", content: "Be brief." },
      { role: "user", content: "Hello" },
      { role: "assistant", content: "Hi" },
      { role: "user", content: "Again" },
    ])
    expect(b).toEqual({
      model: "gpt-6.1-sol",
      instructions: "Be brief.",
      input: [
        { role: "user", content: [{ type: "input_text", text: "Hello" }] },
        { role: "assistant", content: [{ type: "output_text", text: "Hi" }] },
        { role: "user", content: [{ type: "input_text", text: "Again" }] },
      ],
      store: false,
      stream: true,
    })
  })
})

describe("cloud plan stream (chat completions)", () => {
  const chunk = (o: unknown) => `data: ${JSON.stringify(o)}\n\n`
  test("deltas, final usage chunk and [DONE]", async () => {
    const events = await collect(
      parseChatCompletionsStream(
        body([
          chunk({ id: "c1", object: "chat.completion.chunk", choices: [{ index: 0, delta: { role: "assistant", content: "" } }] }),
          chunk({ id: "c1", choices: [{ index: 0, delta: { content: "Hel" } }] }),
          chunk({ id: "c1", choices: [{ index: 0, delta: { content: "lo" }, finish_reason: "stop" }] }),
          chunk({ id: "c1", choices: [], usage: { prompt_tokens: 7, completion_tokens: 2, total_tokens: 9 } }),
          "data: [DONE]\n\n",
        ]),
      ),
    )
    expect(events).toEqual([
      { type: "delta", text: "Hel" },
      { type: "delta", text: "lo" },
      { type: "done", inputTokens: 7, outputTokens: 2 },
    ])
  })

  test("an in-stream error carries the server's message", async () => {
    const events = await collect(parseChatCompletionsStream(body([chunk({ error: { code: "runner_busy", message: "Busy. Try again" } })])))
    expect(events).toEqual([{ type: "error", code: "runner_busy", message: "Busy. Try again" }])
  })
})
