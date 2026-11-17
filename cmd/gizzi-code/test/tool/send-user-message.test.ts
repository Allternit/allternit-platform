import { afterAll, beforeEach, describe, expect, test } from "bun:test"

/**
 * send_user_message through the real tool, the real path checks and the real
 * routing client. Only the HTTP edge (allternit-api user/files and
 * user/notify) is stubbed: fetch records every call.
 */
import fs from "fs/promises"
import { mkdtempSync } from "node:fs"
import { tmpdir as osTmpdir } from "node:os"
import path from "path"

process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN = "internal-test"
process.env.GIZZI_CONFIG_DIR = mkdtempSync(path.join(osTmpdir(), "gizzi-send-user-message-"))
const { tmpdir } = await import("../fixture/fixture")
const { Instance } = await import("../../src/runtime/context/project/instance")
const { SendUserMessageTool } = await import("../../src/runtime/tools/builtins/send-user-message")

type Call = { path: string; body: any }
let calls: Call[] = []
let route: (path: string, body: any) => Response
const realFetch = globalThis.fetch

const ok = (path: string, body: any) => {
  if (path.endsWith("/bot-routing/user/files")) {
    return Response.json({ id: "file_1", name: body.path.split("/").pop(), size: 5 })
  }
  if (path.endsWith("/bot-routing/user/notify")) return Response.json({ pushed: true, event: "agent.message.sent" })
  return new Response(JSON.stringify({ error: "unexpected" }), { status: 500 })
}

beforeEach(() => {
  calls = []
  route = ok
  globalThis.fetch = (async (input: string, init: RequestInit = {}) => {
    const url = new URL(input)
    const body = JSON.parse(String(init.body))
    calls.push({ path: url.pathname, body })
    return route(url.pathname, body)
  }) as typeof fetch
})

afterAll(() => {
  globalThis.fetch = realFetch
  delete process.env.ALLTERNIT_INTERNAL_SERVICE_TOKEN
})

function ctx() {
  return {
    sessionID: "ses_send",
    messageID: "msg_assistant",
    callID: "call_1",
    agent: "build",
    abort: AbortSignal.any([]),
    messages: [],
    metadata: () => {},
    ask: async () => {},
  } as any
}

async function inProject<T>(fn: (dir: string) => Promise<T>, init?: (dir: string) => Promise<void>) {
  await using tmp = await tmpdir({ init })
  return Instance.provide({ directory: tmp.path, fn: () => fn(tmp.path) })
}

async function send(params: any) {
  const tool = await SendUserMessageTool.init()
  return tool.execute(params, ctx())
}

const paths = () => calls.map((c) => c.path.replace(/^.*\/bot-routing/, ""))

describe("tool.send_user_message", () => {
  test("a symlink inside the project that points outside it is refused", async () => {
    const outside = mkdtempSync(path.join(osTmpdir(), "gizzi-outside-"))
    await fs.writeFile(path.join(outside, "secret.txt"), "not yours")
    await inProject(
      async (dir) => {
        await expect(send({ text: "Here it is", attachments: [{ path: path.join(dir, "link.txt") }] })).rejects.toThrow(
          /outside the project/,
        )
        // A plain path outside every root is refused the same way.
        await expect(send({ text: "Here it is", attachments: [{ path: path.join(outside, "secret.txt") }] })).rejects.toThrow(
          /outside the project/,
        )
      },
      async (dir) => {
        await fs.symlink(path.join(outside, "secret.txt"), path.join(dir, "link.txt"))
      },
    )
    expect(calls).toEqual([])
  })

  test("credential files are refused; their template variants are not", async () => {
    await inProject(
      async (dir) => {
        await expect(send({ text: "Config", attachments: [{ path: ".env" }] })).rejects.toThrow(/credential/)
        await expect(send({ text: "Config", attachments: [{ path: path.join(dir, "deploy", "id_ed25519") }] })).rejects.toThrow(
          /credential/,
        )
        expect(calls).toEqual([])
        const template = await send({ text: "Template", attachments: [{ path: ".env.example" }] })
        expect(template.metadata.files.map((f) => f.name)).toEqual([".env.example"])
      },
      async (dir) => {
        await fs.writeFile(path.join(dir, ".env"), "TOKEN=abc")
        await fs.writeFile(path.join(dir, ".env.example"), "TOKEN=")
        await fs.mkdir(path.join(dir, "deploy"))
        await fs.writeFile(path.join(dir, "deploy", "id_ed25519"), "key")
      },
    )
  })

  test("a file over 25 MB and a folder are refused before anything is sent", async () => {
    await inProject(
      async () => {
        await expect(send({ text: "Big", attachments: [{ path: "big.bin" }] })).rejects.toThrow(/at most 25 MB/)
        await expect(send({ text: "Folder", attachments: [{ path: "out" }] })).rejects.toThrow(/folder/)
      },
      async (dir) => {
        const handle = await fs.open(path.join(dir, "big.bin"), "w")
        await handle.truncate(25 * 1024 * 1024 + 1)
        await handle.close()
        await fs.mkdir(path.join(dir, "out"))
      },
    )
    expect(calls).toEqual([])
  })

  test("no push by default: the file goes to Agent Output and the message is the result", async () => {
    const result = await inProject(
      () => send({ text: "# Report ready\nThe Q3 numbers are in.", attachments: [{ path: "report.md", caption: "Q3" }] }),
      async (dir) => {
        await fs.writeFile(path.join(dir, "report.md"), "# Q3\n")
      },
    )
    expect(paths()).toEqual(["/user/files"])
    expect(calls[0].body).toMatchObject({ sessionId: "ses_send", caption: "Q3" })
    expect(path.isAbsolute(calls[0].body.path)).toBe(true)
    expect(result.title).toBe("Report ready")
    expect(result.metadata).toMatchObject({
      text: "# Report ready\nThe Q3 numbers are in.",
      push: "not_requested",
      files: [{ id: "file_1", name: "report.md", size: 5, display: "render", caption: "Q3" }],
    })
    expect(result.output).toBe("Delivered to the user in this thread (1 file). Push: not requested.")
  })

  test("a rate-limited push reaches the model; needs_you asks at level needs_you", async () => {
    route = (p, body) =>
      p.endsWith("/user/notify") ? Response.json({ pushed: false, reason: "rate_limited", event: "thread.needs_user" }) : ok(p, body)
    const result = await inProject(() => send({ text: "I need your sign-off on the vendor.", notify: "needs_you" }))
    expect(paths()).toEqual(["/user/notify"])
    expect(calls[0].body).toEqual({
      sessionId: "ses_send",
      kind: "message",
      level: "needs_you",
      preview: "I need your sign-off on the vendor.",
      ref: "msg_assistant",
    })
    expect(result.metadata.push).toBe("rate_limited")
    expect(result.output).toContain("Push: rate_limited")
    expect(result.output).not.toMatch(/delivered to (the|their) phone/i)
  })

  test("with the Files drive off the file is kept by path, without an id", async () => {
    route = (p, body) =>
      p.endsWith("/user/files")
        ? new Response(JSON.stringify({ error: "The Files drive is off", code: "drive_off" }), { status: 409 })
        : ok(p, body)
    const result = await inProject(
      () => send({ text: "Export attached.", attachments: [{ path: "export.csv" }], notify: "push" }),
      async (dir) => {
        await fs.writeFile(path.join(dir, "export.csv"), "a,b\n")
      },
    )
    const [file] = result.metadata.files
    expect(file.id).toBeUndefined()
    expect(file).toMatchObject({ name: "export.csv", size: 4, display: "attach", not_saved: "drive_off" })
    expect(file.path.endsWith("export.csv")).toBe(true)
    expect(result.metadata.push).toBe("sent")
    expect(result.output).toContain("the Files drive is off")
    expect(result.output).toContain("Push: requested.")
  })
})
