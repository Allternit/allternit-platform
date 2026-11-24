import { describe, expect, test } from "bun:test"
import { buildShareSnapshot } from "../src/cli/ui/ink-app/commands/share/share"

describe("buildShareSnapshot", () => {
  test("keeps text and tool names only", () => {
    const snap = buildShareSnapshot([
      { type: "user", message: { content: "hello" } },
      { type: "user", isMeta: true, message: { content: "system reminder" } },
      {
        type: "assistant",
        message: {
          content: [
            { type: "thinking", thinking: "SECRET-REASONING" },
            { type: "text", text: "reading it" },
            { type: "tool_use", name: "Read", input: { file_path: "/secret/path" } },
          ],
        },
      },
      { type: "user", message: { content: [{ type: "tool_result", content: "SECRET-OUTPUT" }, { type: "image", source: { data: "IMG" } }] } },
    ])
    expect(snap).toEqual([
      { role: "user", text: "hello" },
      { role: "assistant", text: "reading it", tools: ["Read"] },
    ])
    const json = JSON.stringify(snap)
    for (const s of ["SECRET", "IMG", "file_path", "system reminder"]) expect(json).not.toContain(s)
  })
})
