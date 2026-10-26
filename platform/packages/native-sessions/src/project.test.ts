import { mkdirSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { tmpdir } from "node:os"
import { describe, expect, test } from "bun:test"
import { eventsAfter, showNativeSession } from "./project.js"
import { listNativeSessions, snapshotRef, inspectOrigin } from "./index.js"

describe("native-sessions projectors", () => {
  test("projects Codex rollout response_item messages", () => {
    const home = join(tmpdir(), `codex-proj-${Date.now()}`)
    const dir = join(home, ".codex", "sessions", "2026", "09", "06")
    mkdirSync(dir, { recursive: true })
    const id = "019e0283-5626-7071-bfec-9de27bc1e686"
    const file = join(dir, `rollout-2026-09-06T12-00-00-${id}.jsonl`)
    writeFileSync(
      file,
      [
        JSON.stringify({ type: "session_meta", payload: { id, cwd: "/tmp/app" } }),
        JSON.stringify({ type: "response_item", payload: { type: "message", role: "user", content: [{ type: "input_text", text: "ship the catalog" }] } }),
        JSON.stringify({ type: "response_item", payload: { type: "message", role: "assistant", content: [{ type: "output_text", text: "working on it" }] } }),
      ].join("\n") + "\n",
    )
    const listed = listNativeSessions({ home, harnesses: ["codex"] })
    expect(listed.some((s) => s.sessionId === id)).toBe(true)
    const shown = showNativeSession("codex", id, { home })
    expect(shown.events.some((e) => e.role === "user" && e.text?.includes("catalog"))).toBe(true)
    expect(shown.events.some((e) => e.role === "assistant")).toBe(true)
  })

  test("keeps a Claude tool call's input and its result", () => {
    const home = join(tmpdir(), `claude-tools-${Date.now()}`)
    const dir = join(home, ".claude", "projects", "-tmp")
    mkdirSync(dir, { recursive: true })
    const id = "aaaaaaaa-bbbb-cccc-dddd-ffffffffffff"
    writeFileSync(
      join(dir, `${id}.jsonl`),
      [
        JSON.stringify({ type: "user", uuid: "u1", sessionId: id, message: { role: "user", content: "what host?" } }),
        JSON.stringify({ type: "assistant", uuid: "a1", sessionId: id, message: { role: "assistant", content: [{ type: "tool_use", id: "toolu_1", name: "Bash", input: { command: "hostname" } }] } }),
        JSON.stringify({ type: "user", uuid: "u2", sessionId: id, message: { role: "user", content: [{ type: "tool_result", tool_use_id: "toolu_1", content: "box-1" }] } }),
      ].join("\n") + "\n",
    )
    const shown = showNativeSession("claude", id, { home })
    expect(shown.events.find((e) => e.kind === "tool_call")).toMatchObject({ toolName: "Bash", toolId: "toolu_1", input: { command: "hostname" } })
    expect(shown.events.find((e) => e.kind === "tool_result")).toMatchObject({ toolId: "toolu_1", text: "box-1" })
    expect(shown.events.filter((e) => e.kind === "message" && e.role === "user")).toHaveLength(1)
  })

  test("eventsAfter returns only native turns past the snapshot leaf", () => {
    const home = join(tmpdir(), `after-${Date.now()}`)
    const dir = join(home, ".claude", "projects", "-tmp")
    mkdirSync(dir, { recursive: true })
    const id = "bbbbbbbb-bbbb-cccc-dddd-eeeeeeeeeeee"
    const file = join(dir, `${id}.jsonl`)
    writeFileSync(
      file,
      [
        JSON.stringify({ type: "user", uuid: "u1", sessionId: id, message: { role: "user", content: "one" } }),
        JSON.stringify({ type: "assistant", uuid: "a1", sessionId: id, message: { role: "assistant", content: "ok" } }),
      ].join("\n") + "\n",
    )
    const ref = snapshotRef("claude", id, { home })
    expect(ref?.eventId).toBe("a1")
    writeFileSync(
      file,
      [
        JSON.stringify({ type: "user", uuid: "u1", sessionId: id, message: { role: "user", content: "one" } }),
        JSON.stringify({ type: "assistant", uuid: "a1", sessionId: id, message: { role: "assistant", content: "ok" } }),
        JSON.stringify({ type: "user", uuid: "u2", sessionId: id, message: { role: "user", content: "two" } }),
      ].join("\n") + "\n",
    )
    const delta = inspectOrigin(ref!, { home })
    expect(delta.divergence).toBe("native_ahead")
    expect(delta.events.some((e) => e.sourceId === "u2")).toBe(true)
    expect(eventsAfter(showNativeSession("claude", id, { home }), "a1").some((e) => e.sourceId === "u2")).toBe(true)
  })

  test("projects a Cline task (OpenAI-style history with tool calls)", () => {
    const home = join(tmpdir(), `cline-proj-${Date.now()}`)
    const task = join(home, "Library", "Application Support", "Code", "User", "globalStorage", "saoudrizwan.claude-dev", "tasks", "task-9")
    mkdirSync(task, { recursive: true })
    writeFileSync(
      join(task, "api_conversation_history.json"),
      JSON.stringify([
        { role: "user", content: "fix the flaky test" },
        { role: "assistant", content: [{ type: "text", text: "checking the suite" }], tool_calls: [{ id: "call-1", type: "function", function: { name: "run_tests", arguments: "{\"suite\":\"unit\"}" } }] },
        { role: "tool", tool_call_id: "call-1", content: "2 failed" },
        { role: "assistant", content: "found the bug" },
      ]),
    )
    const listed = listNativeSessions({ home, harnesses: ["cline"] })
    expect(listed.some((s) => s.sessionId === "task-9")).toBe(true)
    const shown = showNativeSession("cline", "task-9", { home })
    expect(shown.events.some((e) => e.kind === "message" && e.role === "user" && e.text?.includes("flaky"))).toBe(true)
    expect(shown.events.find((e) => e.kind === "tool_call" && e.toolName === "run_tests")?.input).toEqual({ suite: "unit" })
    expect(shown.events.some((e) => e.kind === "tool_result" && e.text === "2 failed")).toBe(true)
    expect(shown.events.some((e) => e.kind === "message" && e.role === "assistant" && e.text?.includes("found the bug"))).toBe(true)
  })

  test("projects an Amp thread (Anthropic-style content blocks)", () => {
    const home = join(tmpdir(), `amp-proj-${Date.now()}`)
    const threads = join(home, ".local", "share", "amp", "threads")
    mkdirSync(threads, { recursive: true })
    const id = "T-652c6d27-8ae1-4e61-923d-a4d40071cb4b"
    writeFileSync(
      join(threads, `${id}.json`),
      JSON.stringify({
        created_at: "2026-09-01T12:00:00Z",
        messages: [
          { role: "user", content: [{ type: "text", text: "rename the module" }] },
          { role: "assistant", content: [{ type: "text", text: "renaming now" }, { type: "tool_use", id: "tu-1", name: "edit_file", input: {} }] },
          { role: "user", content: [{ type: "tool_result", tool_use_id: "tu-1", content: "done" }] },
        ],
      }),
    )
    const listed = listNativeSessions({ home, harnesses: ["amp"] })
    expect(listed.some((s) => s.sessionId === id)).toBe(true)
    const shown = showNativeSession("amp", id, { home })
    expect(shown.events.some((e) => e.kind === "message" && e.role === "user" && e.text?.includes("rename"))).toBe(true)
    expect(shown.events.some((e) => e.kind === "tool_call" && e.toolName === "edit_file")).toBe(true)
    expect(shown.events.some((e) => e.kind === "tool_result" && e.text === "done")).toBe(true)
  })
})
