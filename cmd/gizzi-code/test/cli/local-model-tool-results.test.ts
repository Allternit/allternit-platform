import { describe, expect, test } from "bun:test"
import { groupToolResults } from "../../src/cli/ui/ink-app/services/api/localModel"

const call = (id: string) => ({ id, type: "function", function: { name: "Bash", arguments: "{}" } })

describe("local model request: tool results follow their calls", () => {
  test("merges a split assistant turn and puts results before user text", () => {
    const out = groupToolResults([
      { role: "user", content: "run it" },
      { role: "assistant", content: "Running." },
      { role: "assistant", content: "", tool_calls: [call("a")] },
      { role: "assistant", content: "", tool_calls: [call("b")] },
      { role: "user", content: "<system-reminder>todo</system-reminder>" },
      { role: "tool", tool_call_id: "b", content: "B" },
      { role: "tool", tool_call_id: "a", content: "A" },
    ])
    expect(out.map((m) => m.role)).toEqual(["user", "assistant", "tool", "tool", "user"])
    expect(out[1]!.content).toBe("Running.")
    expect(out[1]!.tool_calls).toHaveLength(2)
    expect(out.slice(2, 4).map((m) => m.tool_call_id)).toEqual(["a", "b"])
  })

  test("answers a call with no result and drops orphan results", () => {
    const out = groupToolResults([
      { role: "tool", tool_call_id: "x", content: "orphan" },
      { role: "assistant", content: "", tool_calls: [call("a")] },
      { role: "user", content: "next" },
    ])
    expect(out.map((m) => m.role)).toEqual(["assistant", "tool", "user"])
    expect(out[1]!.tool_call_id).toBe("a")
  })
})

describe("strict role alternation only for servers on this machine", () => {
  test("isLoopbackURL", async () => {
    const { isLoopbackURL } = await import("../../src/cli/ui/ink-app/services/api/localModel")
    expect(isLoopbackURL("http://127.0.0.1:8080/v1")).toBe(true)
    expect(isLoopbackURL("http://localhost:11434/v1")).toBe(true)
    expect(isLoopbackURL("http://[::1]:8000/v1")).toBe(true)
    expect(isLoopbackURL("https://api.allternit.com/v1")).toBe(false)
    expect(isLoopbackURL("not a url")).toBe(false)
  })
})

describe("chat request retries a stale keep-alive socket once", () => {
  test("retries on a closed socket, not on other errors", async () => {
    const { fetchRetryingStaleSocket } = await import("../../src/cli/ui/ink-app/services/api/localModel")
    const real = globalThis.fetch
    let calls = 0
    globalThis.fetch = (async () => {
      calls++
      if (calls === 1) throw new Error("The socket connection was closed unexpectedly.")
      return new Response("ok")
    }) as unknown as typeof fetch
    try {
      expect(await (await fetchRetryingStaleSocket("http://x/chat/completions", {})).text()).toBe("ok")
      expect(calls).toBe(2)
      calls = 0
      globalThis.fetch = (async () => {
        calls++
        throw new Error("401 Unauthorized")
      }) as unknown as typeof fetch
      await expect(fetchRetryingStaleSocket("http://x/chat/completions", {})).rejects.toThrow("401")
      expect(calls).toBe(1)
    } finally {
      globalThis.fetch = real
    }
  })
})
