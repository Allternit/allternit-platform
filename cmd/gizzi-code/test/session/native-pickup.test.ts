import { describe, expect, test } from "bun:test"
import { mkdirSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { NativeSource } from "../../src/runtime/session/native-source"
import { tmpdir } from "../fixture/fixture"

describe("native session pickup", () => {
  test("writes tool calls as tool steps with their input and output", async () => {
    await using tmp = await tmpdir({ git: true })
    const claudeHome = join(tmp.path, ".claude-home")
    const id = "aaaaaaaa-bbbb-cccc-dddd-111111111111"
    mkdirSync(join(claudeHome, "projects", "-tmp"), { recursive: true })
    writeFileSync(
      join(claudeHome, "projects", "-tmp", `${id}.jsonl`),
      [
        { type: "user", uuid: "u1", sessionId: id, message: { role: "user", content: "what host is this?" } },
        { type: "assistant", uuid: "a1", sessionId: id, message: { role: "assistant", content: [{ type: "text", text: "Checking." }, { type: "tool_use", id: "toolu_1", name: "Bash", input: { command: "hostname", description: "Show host name" } }] } },
        { type: "user", uuid: "u2", sessionId: id, message: { role: "user", content: [{ type: "tool_result", tool_use_id: "toolu_1", content: "box-1" }] } },
        { type: "assistant", uuid: "a2", sessionId: id, message: { role: "assistant", content: [{ type: "text", text: "It is box-1." }] } },
      ]
        .map((r) => JSON.stringify(r))
        .join("\n") + "\n",
    )
    const previous = process.env.CLAUDE_CONFIG_DIR
    process.env.CLAUDE_CONFIG_DIR = claudeHome
    try {
      await Instance.provide({
        directory: tmp.path,
        fn: async () => {
          const { session } = await NativeSource.pickup({ harness: "claude", sessionId: id })
          const msgs = await Session.messages({ sessionID: session.id })
          expect(msgs.map((m) => m.info.role)).toEqual(["user", "assistant"])
          const parts = msgs[1]!.parts
          expect(parts.map((p) => p.type)).toEqual(["text", "tool", "text"])
          const tool = parts[1]!
          if (tool.type !== "tool") throw new Error("expected a tool part")
          expect(tool.tool).toBe("Bash")
          expect(tool.callID).toBe("toolu_1")
          expect(tool.state).toMatchObject({ status: "completed", input: { command: "hostname" }, output: "box-1", title: "Show host name" })
          expect(parts.some((p) => p.type === "text" && p.text.includes("[native tool"))).toBe(false)
          await Session.remove(session.id)
        },
      })
    } finally {
      if (previous === undefined) delete process.env.CLAUDE_CONFIG_DIR
      else process.env.CLAUDE_CONFIG_DIR = previous
    }
  })
})
