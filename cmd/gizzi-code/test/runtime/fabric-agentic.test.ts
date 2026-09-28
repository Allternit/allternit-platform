import { describe, expect, test } from "bun:test"
import { confineToGrants, parseGrants, runAgenticLoop } from "@/runtime/fabric-transport/agentic"
import * as fs from "node:fs"
import * as os from "node:os"
import * as path from "node:path"

describe("parseGrants", () => {
  test("empty and malformed input deny everything", () => {
    expect(parseGrants(undefined)).toEqual([])
    expect(parseGrants("")).toEqual([])
    expect(parseGrants("not json")).toEqual([])
    expect(parseGrants('["/tmp/a", 7]')).toEqual(["/tmp/a"])
  })
})

describe("confineToGrants", () => {
  test("default-deny with no grants", () => {
    expect(confineToGrants("/tmp/anything", [])).toBeNull()
  })

  test("allows paths inside a grant root", () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "grants-"))
    const nested = path.join(root, "sub", "file.txt")
    expect(confineToGrants(nested, [root])).toBe(path.resolve(nested))
    expect(confineToGrants(root, [root])).toBe(path.resolve(root))
    expect(confineToGrants("/etc/passwd", [root])).toBeNull()
    expect(confineToGrants(root + "-evil/x", [root])).toBeNull()
    fs.rmSync(root, { recursive: true, force: true })
  })

  test("refuses symlinks that escape the grant", () => {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "grants-"))
    const outside = path.join(os.tmpdir(), `outside-${Date.now()}.txt`)
    fs.writeFileSync(outside, "secret")
    const link = path.join(root, "link.txt")
    fs.symlinkSync(outside, link)
    expect(confineToGrants(link, [root])).toBeNull()
    fs.rmSync(root, { recursive: true, force: true })
    fs.rmSync(outside, { force: true })
  })
})

describe("runAgenticLoop budgets", () => {
  const depsBase = {
    apiBase: "http://127.0.0.1:9",
    operatorKey: "op-test-key",
    model: "test/model",
    grants: [] as string[],
    checkpoint: async () => {},
    log: () => {},
  }

  test("a bot job's instructions lead the system prompt, worker rules still apply", async () => {
    let sent: { messages: Array<{ role: string; content: string }>; model: string } | null = null
    const fetchImpl = (async (_url: string, init: RequestInit) => {
      sent = JSON.parse(String(init.body))
      return new Response(JSON.stringify({ choices: [{ message: { role: "assistant", content: "Priced it." } }], usage: { total_tokens: 10 } }))
    }) as unknown as typeof fetch
    await runAgenticLoop("Price the H100", {
      ...depsBase,
      fetchImpl,
      model: "anthropic/claude-sonnet-5",
      system: "# Bot identity\n\nYou are Ledger, Finance analyst.",
      complete: async () => {},
    })
    const system = sent!.messages[0]
    expect(system.role).toBe("system")
    expect(system.content.startsWith("# Bot identity")).toBe(true)
    expect(system.content).toContain("# Worker rules")
    expect(system.content).toContain("Granted folders")
    expect(sent!.model).toBe("anthropic/claude-sonnet-5")
  })

  test("refuses to start without an operator key", async () => {
    let completed: { success: boolean } | null = null
    await runAgenticLoop("task", {
      ...depsBase,
      operatorKey: null,
      complete: async (success) => {
        completed = { success }
      },
    })
    expect(completed).toEqual({ success: false })
  })

  test("stops at the step budget with an honest failure result", async () => {
    // Model always asks for another tool call → the step budget must end it.
    const fetchImpl = (async () =>
      new Response(
        JSON.stringify({
          choices: [
            {
              message: {
                role: "assistant",
                content: null,
                tool_calls: [
                  {
                    id: `call-${Math.random()}`,
                    type: "function",
                    function: { name: "fs_read", arguments: '{"path":"/tmp/x"}' },
                  },
                ],
              },
              finish_reason: "tool_calls",
            },
          ],
          usage: { total_tokens: 10 },
        }),
        { status: 200 },
      )) as unknown as typeof fetch
    let completed: { success: boolean; summary: string } | null = null
    await runAgenticLoop("task", {
      ...depsBase,
      fetchImpl,
      maxSteps: 3,
      complete: async (success, summary) => {
        completed = { success, summary }
      },
    })
    expect(completed).not.toBeNull()
    expect(completed!.success).toBe(false)
    expect(completed!.summary).toContain("step budget exceeded")
  })

  test("fs_write outside grants is refused by the tool (default-deny)", async () => {
    const fetchImpl = (async () =>
      new Response(
        JSON.stringify({
          choices: [
            {
              message: {
                role: "assistant",
                content: null,
                tool_calls: [
                  {
                    id: "call-1",
                    type: "function",
                    function: { name: "fs_write", arguments: '{"path":"/etc/evil.txt","content":"x"}' },
                  },
                ],
              },
              finish_reason: "tool_calls",
            },
          ],
          usage: { total_tokens: 5 },
        }),
        { status: 200 },
      )) as unknown as typeof fetch
    const seen: string[] = []
    let completed: { success: boolean } | null = null
    await runAgenticLoop("task", {
      ...depsBase,
      fetchImpl,
      maxSteps: 1,
      complete: async (success) => {
        completed = { success }
      },
      log: (_level, _event, fields) => {
        if (fields?.ok === false) seen.push("refused")
      },
    })
    expect(seen).toContain("refused")
    expect(completed).not.toBeNull()
  })
})
