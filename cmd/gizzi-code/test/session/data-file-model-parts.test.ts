// @ts-nocheck
/**
 * Data-file attachments (CSV, TSV, JSON, XLSX, XLS) must never break a model
 * turn. Before this, `toModelMessages` sent them as raw file parts and the AI
 * SDK providers threw UnsupportedFunctionalityError ("media type: text/csv"),
 * and CLI brains dropped them. The model now gets text (or a short note); the
 * stored part is untouched so run_code can still stage the file.
 *
 * The provider calls below go through the real @ai-sdk providers; only their
 * HTTP edge (fetch) is replaced, to read the request body they would send.
 */
import { describe, expect, test } from "bun:test"
import { generateText } from "ai"
import { createAnthropic } from "@ai-sdk/anthropic"
import { createOpenAICompatible } from "@ai-sdk/openai-compatible"
import { MessageV2 } from "../../src/runtime/session/message-v2"
import { ProviderTransform } from "../../src/runtime/providers/adapters/transform"

const XLSX = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"

function model(npm: string, providerID: string) {
  return {
    id: "m",
    providerID,
    api: { id: "m", url: "https://api.test", npm },
    name: "m",
    capabilities: {
      temperature: true,
      reasoning: false,
      attachment: true,
      toolcall: true,
      input: { text: true, audio: false, image: true, video: false, pdf: true },
      output: { text: true, audio: false, image: false, video: false, pdf: false },
      interleaved: false,
    },
    cost: { input: 0, output: 0, cache: { read: 0, write: 0 } },
    limit: { context: 100_000, input: 0, output: 4096 },
    status: "active",
    options: {},
    headers: {},
    release_date: "2026-01-01",
  }
}

const dataUrl = (mime: string, body: string | Buffer) =>
  `data:${mime};base64,${Buffer.from(body).toString("base64")}`

function userMessage(text: string, files: Array<{ mime: string; filename: string; url: string }>) {
  return [
    {
      info: { id: "msg_u", sessionID: "s", role: "user", time: { created: 0 }, agent: "build", model: { providerID: "p", modelID: "m" } },
      parts: [
        { id: "p0", sessionID: "s", messageID: "msg_u", type: "text", text },
        ...files.map((f, i) => ({ id: `p${i + 1}`, sessionID: "s", messageID: "msg_u", type: "file", ...f })),
      ],
    },
  ]
}

const CSV = "month,revenue\nJan,10\nFeb,30\n"
const csvPart = { mime: "text/csv", filename: "sales.csv", url: dataUrl("text/csv", CSV) }

function userContent(input: any[], m = model("@ai-sdk/anthropic", "anthropic")) {
  const [msg] = MessageV2.toModelMessages(input, m)
  return msg.content as any[]
}

describe("data files reach the model as text, never as a raw file part", () => {
  test("a small CSV goes inline as its text", () => {
    const content = userContent(userMessage("plot it", [csvPart]))
    expect(content.some((p) => p.type === "file")).toBe(false)
    expect(content[1].text).toContain(`[Attached file: sales.csv]\n${CSV}\n[/Attached file]`)
  })

  test("a CSV over 20,000 characters becomes a note pointing at run_code", () => {
    const big = "a,b\n" + "1,2\n".repeat(6000)
    const content = userContent(userMessage("plot it", [{ ...csvPart, url: dataUrl("text/csv", big) }]))
    expect(content.some((p) => p.type === "file")).toBe(false)
    expect(content[1].text.trim()).toBe(
      "Attached file sales.csv (text/csv, 23.4 KB): too large to show inline; a code tool can read the file.",
    )
    expect(content[1].text).not.toContain("1,2")
  })

  test("TSV and JSON go inline; XLSX and XLS are always a note", () => {
    const content = userContent(
      userMessage("look", [
        { mime: "text/tab-separated-values", filename: "t.tsv", url: dataUrl("text/tab-separated-values", "a\tb\n") },
        { mime: "application/json", filename: "d.json", url: dataUrl("application/json", '{"k":1}') },
        { mime: XLSX, filename: "book.xlsx", url: dataUrl(XLSX, Buffer.from([0x50, 0x4b, 3, 4])) },
        { mime: "application/vnd.ms-excel", filename: "old.xls", url: dataUrl("application/vnd.ms-excel", "xx") },
      ]),
    )
    expect(content.some((p) => p.type === "file")).toBe(false)
    expect(content[1].text).toContain("a\tb")
    expect(content[2].text).toContain('{"k":1}')
    expect(content[3].text.trim()).toBe(`Attached file book.xlsx (${XLSX}, 4 B): binary, not shown inline; a code tool can read the file.`)
    expect(content[4].text).toContain("Attached file old.xls (application/vnd.ms-excel, 2 B)")
    expect(content[4].text).toContain("a code tool can read the file")
  })

  test("text the composer already inlined is not sent twice", () => {
    const text = `plot it\n\n[Attached file: sales.csv]\n${CSV}\n[/Attached file]`
    const content = userContent(userMessage(text, [csvPart]))
    expect(content).toHaveLength(2)
    expect(content[1].text.trim()).toBe(
      "Attached file sales.csv (text/csv, 28 B): its text is in the message above; a code tool can read the file.",
    )
    // The CSV appears once: in the person's own text.
    expect(content.map((p) => p.text).join("").split("Feb,30").length - 1).toBe(1)
  })

  test("an image keeps today's file part", () => {
    const png = { mime: "image/png", filename: "a.png", url: dataUrl("image/png", Buffer.from([0x89, 0x50, 0x4e, 0x47])) }
    const content = userContent(userMessage("see", [png]))
    expect(content[1]).toEqual({ type: "file", mediaType: "image/png", filename: "a.png", data: png.url })
  })

  test("the stored message is not changed (run_code still finds the file part)", () => {
    const input = userMessage("plot it", [csvPart])
    const before = JSON.stringify(input)
    MessageV2.toModelMessages(input, model("@ai-sdk/anthropic", "anthropic"))
    expect(JSON.stringify(input)).toBe(before)
    expect(input[0].parts.filter((p) => p.type === "file")).toHaveLength(1)
  })
})

describe("what the provider is sent", () => {
  async function send(npm: string, m: any, input: any[]) {
    let body: any
    const fetch = async (_url: string, init: any) => {
      body = JSON.parse(init.body)
      const json =
        npm === "@ai-sdk/anthropic"
          ? {
              id: "x",
              type: "message",
              role: "assistant",
              model: "m",
              content: [{ type: "text", text: "ok" }],
              stop_reason: "end_turn",
              usage: { input_tokens: 1, output_tokens: 1 },
            }
          : {
              id: "x",
              object: "chat.completion",
              created: 0,
              model: "m",
              choices: [{ index: 0, message: { role: "assistant", content: "ok" }, finish_reason: "stop" }],
              usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
            }
      return new Response(JSON.stringify(json), { headers: { "content-type": "application/json" } })
    }
    const sdk =
      npm === "@ai-sdk/anthropic"
        ? createAnthropic({ apiKey: "k", fetch }).languageModel("m")
        : createOpenAICompatible({ name: "oc", baseURL: "https://api.test/v1", apiKey: "k", fetch }).chatModel("m")
    const messages = ProviderTransform.message(MessageV2.toModelMessages(input, m), m, {})
    await generateText({ model: sdk, messages })
    return JSON.stringify(body)
  }

  const files = [csvPart, { mime: XLSX, filename: "book.xlsx", url: dataUrl(XLSX, Buffer.from([0x50, 0x4b, 3, 4])) }]

  test("Anthropic: the CSV text and the XLSX note, no document block", async () => {
    const body = await send("@ai-sdk/anthropic", model("@ai-sdk/anthropic", "anthropic"), userMessage("plot it", files))
    expect(body).toContain("Feb,30")
    expect(body).toContain("Attached file book.xlsx")
    expect(body).not.toContain('"document"')
    expect(body).not.toContain("text/csv;base64")
  })

  test("OpenAI-compatible: the CSV text and the XLSX note, no file part", async () => {
    const m = model("@ai-sdk/openai-compatible", "oc")
    const body = await send("@ai-sdk/openai-compatible", m, userMessage("plot it", files))
    expect(body).toContain("Feb,30")
    expect(body).toContain("Attached file book.xlsx")
    expect(body).not.toContain("base64")
  })
})
