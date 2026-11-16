// @ts-nocheck
/**
 * /projects pad text: Notes, Checklist and Links render from the runtime pad;
 * a null pad says "No pad yet"; an old runtime (404) says to update.
 */
import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { Writable } from "node:stream"
import React from "react"

const stripAnsi = (s: string) => s.replace(/\x1B\[[0-?]*[ -/]*[@-~]/g, "")
const realFetch = globalThis.fetch

beforeEach(() => {
  process.env.ALLTERNIT_API_URL = "http://gw.test"
})
afterEach(() => {
  globalThis.fetch = realFetch
})

async function paint(result) {
  const { render } = await import("../../src/cli/ui/ink-app/ink")
  const { PadText } = await import("../../src/cli/ui/ink-app/commands/work/projects")
  let frames = ""
  const stdout = new Writable({
    write(chunk, _enc, cb) {
      frames += chunk.toString()
      cb()
    },
  })
  const instance = await render(React.createElement(PadText, { result }), {
    stdout,
    exitOnCtrlC: false,
    patchConsole: false,
  })
  await new Promise(r => setTimeout(r, 150))
  instance.unmount()
  return stripAnsi(frames)
}

describe("project pad text", () => {
  test("pad with notes, checklist and links renders the three sections", async () => {
    const plain = await paint({
      unsupported: false,
      pad: {
        data: {
          notes: "Ship the folder view\nsecond line",
          checklist: [
            { id: "a", text: "Write docs", done: true },
            { id: "b", text: "Review PR", done: false },
          ],
          links: [{ title: "Design", url: "https://example.com/design" }],
        },
      },
    })
    expect(plain).toContain("Notes")
    expect(plain).toContain("Ship the folder view")
    expect(plain).toContain("Checklist")
    expect(plain).toContain("[x] Write docs")
    expect(plain).toContain("[ ] Review PR")
    expect(plain).toContain("Links")
    expect(plain).toContain("Design — example.com")
  })

  test("null pad renders No pad yet", async () => {
    expect(await paint({ pad: null, unsupported: false })).toContain("No pad yet")
  })

  test("old runtime renders the update line", async () => {
    expect(await paint({ pad: null, unsupported: true })).toContain("Update your computer to sync pads")
  })
})

describe("getProjectPad", () => {
  test("GETs the encoded scope and unwraps pad; 404 is unsupported", async () => {
    const { getProjectPad, projectPadScope } = await import("../../src/cli/work/api")
    const urls: string[] = []
    let status = 200
    globalThis.fetch = (async (input: any) => {
      urls.push(String(input))
      return new Response(JSON.stringify({ pad: status === 200 ? { data: { notes: "hi" } } : null }), { status })
    }) as typeof fetch
    const ok = await getProjectPad(projectPadScope("p1"))
    expect(urls[0]).toContain("/api/v1/project-pad/project%3Achat%3Ap1")
    expect(ok.pad?.data?.notes).toBe("hi")
    status = 404
    expect(await getProjectPad("project:chat:p1")).toEqual({ pad: null, unsupported: true })
  })
})
