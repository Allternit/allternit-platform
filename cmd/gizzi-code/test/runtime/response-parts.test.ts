/**
 * Response-parts contract, runtime side: each brain's native citations and
 * plans become the same stored sources / todo rows, and the bridge frames.
 */

import { describe, expect, test } from "bun:test"
import {
  planFrameForTodos,
  sourceFramesForPart,
  sourceFromStreamPart,
  sourcesFromClaudeCodeToolResult,
  sourcesFromExaText,
  todosFromNativePlan,
} from "@/runtime/session/response-parts"

describe("sources", () => {
  test("AI SDK source parts keep the provider as origin", () => {
    const part = { type: "source", sourceType: "url", id: "s1", url: "https://www.nature.com/x", title: "Nature" }
    expect(sourceFromStreamPart(part, "anthropic")).toMatchObject({ id: "s1", host: "nature.com", origin: "anthropic-citation" })
    expect(sourceFromStreamPart(part, "openai")?.origin).toBe("openai-url-citation")
    expect(sourceFromStreamPart(part, "google")?.origin).toBe("gemini-grounding")
    expect(sourceFromStreamPart({ ...part, sourceType: "document" }, "anthropic")).toBeUndefined()
  })

  test("Claude Code WebSearch tool_use_result", () => {
    const toolUseResult = {
      query: "bun release",
      results: [
        { tool_use_id: "srvtoolu_1", content: [{ title: "Bun 1.3", url: "https://bun.sh/blog/bun-v1.3" }, { title: "dup", url: "https://bun.sh/blog/bun-v1.3" }] },
        "Summary text the model wrote",
      ],
      durationSeconds: 2.1,
    }
    const sources = sourcesFromClaudeCodeToolResult(toolUseResult)
    expect(sources).toHaveLength(1)
    expect(sources[0]).toMatchObject({ title: "Bun 1.3", host: "bun.sh", origin: "claude-code-websearch" })
  })

  test("gizzi websearch (Exa) text", () => {
    const text = "Title: First\nURL: https://a.example/1\nText: …\n\nTitle: Second\nURL: https://b.example/2\n"
    expect(sourcesFromExaText(text).map((s) => s.title)).toEqual(["First", "Second"])
  })

  test("frames come from text metadata and completed tools, once per url", () => {
    const sent = new Set<string>()
    const text = { type: "text", metadata: { sources: [{ id: "s1", url: "https://a.example/1", title: "A", origin: "openai-url-citation" }] } }
    expect(sourceFramesForPart(text, "m", sent)).toEqual([
      expect.objectContaining({ type: "source", sourceId: "s1", url: "https://a.example/1", host: "a.example", origin: "openai-url-citation" }),
    ])
    expect(sourceFramesForPart(text, "m", sent)).toEqual([])
    const tool = { type: "tool", callID: "c1", state: { status: "completed", metadata: { sources: [{ url: "https://b.example/2" }] } } }
    expect(sourceFramesForPart(tool, "m", sent)[0]).toMatchObject({ toolCallId: "c1", url: "https://b.example/2" })
  })
})

describe("plans", () => {
  test("Codex turn/plan/updated, ACP entries and TodoWrite map to todo rows", () => {
    expect(todosFromNativePlan({ plan: [{ step: "Read", status: "completed" }, { step: "Edit", status: "inProgress" }] })).toEqual([
      { content: "Read", status: "completed", priority: "medium" },
      { content: "Edit", status: "in_progress", priority: "medium" },
    ])
    expect(todosFromNativePlan({ entries: [{ content: "Plan", status: "pending", priority: "high" }] })).toEqual([
      { content: "Plan", status: "pending", priority: "high" },
    ])
    expect(todosFromNativePlan({ todos: [{ content: "Drop", status: "cancelled" }] })?.[0].status).toBe("cancelled")
    expect(todosFromNativePlan({ nothing: true })).toBeUndefined()
  })

  test("plan frame carries legacy steps and contract items", () => {
    const frame = planFrameForTodos([{ content: "Read", status: "completed" }, { content: "Edit", status: "in_progress" }], "m")
    expect(frame.items[1]).toEqual({ id: "todo-1", content: "Edit", status: "in_progress" })
    expect(frame.steps.map((s) => s.status)).toEqual(["complete", "in-progress"])
  })
})
