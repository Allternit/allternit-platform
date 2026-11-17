// @ts-nocheck
import { describe, expect, test } from "bun:test"
import path from "path"
import fs from "fs/promises"
import { tmpdir } from "../fixture/fixture"
import { Instance } from "../../src/project/instance"
import { ToolRegistry } from "../../src/runtime/tools/builtins/registry"
import { Flag } from "../../src/runtime/context/flag/flag"
import { WORKER_BOT_TOOL_IDS } from "../../src/runtime/tools/builtins/worker-bot-tools"
import { PROJECT_SEND_TOOL_IDS } from "../../src/runtime/tools/builtins/project-send"

describe("tool.registry", () => {
  test("loads tools from .gizzi/tool (singular)", async () => {
    await using tmp = await tmpdir({
      init: async (dir) => {
        const gizziDir = path.join(dir, ".gizzi")
        await fs.mkdir(gizziDir, { recursive: true })

        const toolDir = path.join(gizziDir, "tool")
        await fs.mkdir(toolDir, { recursive: true })

        await Bun.write(
          path.join(toolDir, "hello.ts"),
          [
            "export default {",
            "  description: 'hello tool',",
            "  args: {},",
            "  execute: async () => {",
            "    return 'hello world'",
            "  },",
            "}",
            "",
          ].join("\n"),
        )
      },
    })

    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const ids = await ToolRegistry.ids()
        expect(ids).toContain("hello")
      },
    })
  })

  test("loads tools from .gizzi/tools (plural)", async () => {
    await using tmp = await tmpdir({
      init: async (dir) => {
        const gizziDir = path.join(dir, ".gizzi")
        await fs.mkdir(gizziDir, { recursive: true })

        const toolsDir = path.join(gizziDir, "tools")
        await fs.mkdir(toolsDir, { recursive: true })

        await Bun.write(
          path.join(toolsDir, "hello.ts"),
          [
            "export default {",
            "  description: 'hello tool',",
            "  args: {},",
            "  execute: async () => {",
            "    return 'hello world'",
            "  },",
            "}",
            "",
          ].join("\n"),
        )
      },
    })

    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const ids = await ToolRegistry.ids()
        expect(ids).toContain("hello")
      },
    })
  })

  test("loads tools with external dependencies without crashing", async () => {
    // Dependency auto-install is an explicit opt-in (see Config.directories);
    // without it the .gizzi bun install never runs and the cowsay import below
    // cannot resolve. Set it just for this test and restore afterwards.
    const prev = process.env.GIZZI_AUTO_INSTALL_DEPS
    process.env.GIZZI_AUTO_INSTALL_DEPS = "1"
    try {
      await using tmp = await tmpdir({
      init: async (dir) => {
        const gizziDir = path.join(dir, ".gizzi")
        await fs.mkdir(gizziDir, { recursive: true })

        const toolsDir = path.join(gizziDir, "tools")
        await fs.mkdir(toolsDir, { recursive: true })

        await Bun.write(
          path.join(gizziDir, "package.json"),
          JSON.stringify({
            name: "custom-tools",
            dependencies: {
              "@allternit/plugin": "^0.0.0",
              cowsay: "^1.6.0",
            },
          }),
        )

        await Bun.write(
          path.join(toolsDir, "cowsay.ts"),
          [
            "import { say } from 'cowsay'",
            "export default {",
            "  description: 'tool that imports cowsay at top level',",
            "  args: { text: { type: 'string' } },",
            "  execute: async ({ text }: { text: string }) => {",
            "    return say({ text })",
            "  },",
            "}",
            "",
          ].join("\n"),
        )
      },
    })

    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const ids = await ToolRegistry.ids()
        expect(ids).toContain("cowsay")
      },
    })
    } finally {
      if (prev === undefined) delete process.env.GIZZI_AUTO_INSTALL_DEPS
      else process.env.GIZZI_AUTO_INSTALL_DEPS = prev
    }
  })

  test("a turn with computer: true gets the computer tools; a turn without it does not", async () => {
    await using tmp = await tmpdir()
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const computerTools = ["computer", "computer_v2", "computers"]
        const withOption = await ToolRegistry.ids({ computer: true })
        for (const id of computerTools) expect(withOption).toContain(id)
        const without = await ToolRegistry.ids()
        for (const id of computerTools) {
          if (Flag.GIZZI_ENABLE_COMPUTER_TOOL) expect(without).toContain(id)
          else expect(without).not.toContain(id)
        }
        // computer: false never takes away what the env flag turned on.
        expect(await ToolRegistry.ids({ computer: false })).toEqual(without)
      },
    })
  })

  test("current_time and send_user_message reach every client, not only bot chats", async () => {
    // Primitives Wave A: Gizzi Code and plain app chats need both. The bot-only
    // lists are what SessionPrompt.resolveTools deletes outside an app bot's chat.
    const botOnly = new Set<string>([...ToolRegistry.BOT_THREAD_TOOL_IDS, ...WORKER_BOT_TOOL_IDS, ...PROJECT_SEND_TOOL_IDS, "message_agent", "suggest_task", "grant_task_permission"])
    const prev = process.env.GIZZI_CLIENT
    await using tmp = await tmpdir()
    try {
      for (const client of ["app", "cli"]) {
        process.env.GIZZI_CLIENT = client
        await Instance.provide({
          directory: tmp.path,
          fn: async () => {
            const ids = await ToolRegistry.ids()
            for (const id of ["current_time", "send_user_message"]) {
              expect(ids).toContain(id)
              expect(botOnly.has(id)).toBe(false)
            }
          },
        })
      }
    } finally {
      if (prev === undefined) delete process.env.GIZZI_CLIENT
      else process.env.GIZZI_CLIENT = prev
    }
  })

  test("Wave D recall tools and thread_search reach every client; only thread_update / thread_close stay bot-only", async () => {
    // resolveTools deletes ToolRegistry.BOT_THREAD_TOOL_IDS outside an app bot's
    // chat, so a plain chat or Gizzi Code session keeps thread_search (decision 8).
    const recall = ["memory_list", "thread_search", "thread_read", "thread_recent", "thread_wait", "resource"]
    expect([...ToolRegistry.BOT_THREAD_TOOL_IDS].sort()).toEqual(["thread_close", "thread_update"])
    const botOnly = new Set<string>([...ToolRegistry.BOT_THREAD_TOOL_IDS, ...WORKER_BOT_TOOL_IDS, ...PROJECT_SEND_TOOL_IDS, "message_agent", "suggest_task", "grant_task_permission"])
    const prev = process.env.GIZZI_CLIENT
    await using tmp = await tmpdir()
    try {
      for (const client of ["app", "cli", "desktop"]) {
        process.env.GIZZI_CLIENT = client
        await Instance.provide({
          directory: tmp.path,
          fn: async () => {
            const ids = await ToolRegistry.ids()
            for (const id of recall) {
              expect(ids).toContain(id)
              expect(botOnly.has(id)).toBe(false)
            }
          },
        })
      }
    } finally {
      if (prev === undefined) delete process.env.GIZZI_CLIENT
      else process.env.GIZZI_CLIENT = prev
    }
  })
})
