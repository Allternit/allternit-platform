import { afterAll, afterEach, beforeEach, describe, expect, test } from "bun:test"
import fs from "fs"
import os from "os"
import path from "path"
import type { Tool as AITool } from "ai"
import type { MessageV2 } from "@/runtime/session/message-v2"
import { ToolFamilies } from "@/runtime/primitives/tool-families"
import { DISABLE_TOOL_SEARCH_ENV } from "@/runtime/primitives/wave-b-types"

const stateDir = fs.mkdtempSync(path.join(os.tmpdir(), "tool-families-"))
afterAll(() => fs.rmSync(stateDir, { recursive: true, force: true }))

const BASE = "Load tools that are not in your tool list yet."

/** A step's tool record, as resolveTools builds it (a new object per step). */
function step(): Record<string, AITool> {
  const ids = [
    "invalid",
    "read",
    "bash",
    "task",
    "tool_search",
    "send_agent_email",
    "get_agent_email_status",
    "automation",
    "scratchpad_read",
    "scratchpad_write",
    "mcp__github__create_issue",
    "mcp__github__list_prs",
    "mcp__linear__search",
    "my_plugin_tool",
  ]
  const tools: Record<string, AITool> = {}
  for (const id of ids) {
    tools[id] = {
      description:
        id === "tool_search" ? BASE : id === "mcp__linear__search" ? "Find tickets in the tracker" : `The ${id} tool`,
    } as unknown as AITool
  }
  return tools
}

let n = 0
let sessionID = ""
const run = (messages: MessageV2.WithParts[] = []) => ToolFamilies.filter({ sessionID, tools: step(), messages })
const search = (query: string) => ToolFamilies.search({ sessionID, messages: [], query })

beforeEach(() => {
  sessionID = `ses_test_${++n}`
  ToolFamilies.setStateDirectory(stateDir)
  ToolFamilies.reset()
  delete process.env[DISABLE_TOOL_SEARCH_ENV]
})
afterEach(() => {
  ToolFamilies.setStateDirectory(undefined)
  delete process.env[DISABLE_TOOL_SEARCH_ENV]
})

describe("ToolFamilies.filter", () => {
  test("keeps always-loaded and unmapped tools, holds back deferred families and MCP servers", () => {
    const result = run()
    expect(Object.keys(result.tools).sort()).toEqual(["bash", "invalid", "my_plugin_tool", "read", "task", "tool_search"])
    expect(result.deferred.sort()).toEqual([
      "automation",
      "get_agent_email_status",
      "mcp__github__create_issue",
      "mcp__github__list_prs",
      "mcp__linear__search",
      "scratchpad_read",
      "scratchpad_write",
      "send_agent_email",
    ])
  })

  test("tool_search description lists deferred names by group, names only", () => {
    const description = (run().tools.tool_search as { description: string }).description
    expect(description.startsWith(BASE)).toBe(true)
    expect(description).toContain("- email: get_agent_email_status, send_agent_email")
    expect(description).toContain("- mcp:github: mcp__github__create_issue, mcp__github__list_prs")
    expect(description).toContain("- mcp:linear: mcp__linear__search")
    expect(description).not.toContain("The send_agent_email tool")
    expect(description).not.toContain("bash")
    // Generating twice on the same text does not stack lists.
    expect(ToolFamilies.description(description, ["automation"])).toBe(
      `${BASE}\n\nTools you can load (by group; call tool_search with keywords or the group name):\n- automation: automation`,
    )
  })

  test("the always-loaded set never overlaps a deferred family", () => {
    for (const id of ToolFamilies.ALWAYS_LOADED) expect(ToolFamilies.familyOf(id)).toBeUndefined()
    expect(ToolFamilies.familyOf("mcp__github__create_issue")).toBe("mcp")
    expect(ToolFamilies.groupOf("mcp__github__create_issue")).toBe("mcp:github")
  })
})

describe("ToolFamilies.search", () => {
  test("keywords load the matching family for the next step", async () => {
    run()
    const result = await search("send an email to the client")
    expect(result).toEqual({ loaded: ["get_agent_email_status", "send_agent_email"], already: [], unknown: [] })
    const next = run()
    expect(next.tools.send_agent_email).toBeDefined()
    expect(next.tools.get_agent_email_status).toBeDefined()
    expect(next.tools.automation).toBeUndefined()
    expect((next.tools.tool_search as { description: string }).description).not.toContain("send_agent_email")
    expect(await search("email")).toEqual({ loaded: [], already: ["get_agent_email_status", "send_agent_email"], unknown: [] })
  })

  test("keywords match stems, MCP server names and MCP descriptions", async () => {
    run()
    expect((await search("set some reminders")).loaded).toEqual(["automation"])
    expect((await search("github")).loaded).toEqual(["mcp__github__create_issue", "mcp__github__list_prs"])
    expect((await search("tickets")).loaded).toEqual(["mcp__linear__search"])
  })

  test("select: loads exact tools (or a group by name) and reports known and unknown names", async () => {
    run()
    const result = await search("select:scratchpad_read, bash, nope, mcp:linear")
    expect(result).toEqual({ loaded: ["mcp__linear__search", "scratchpad_read"], already: ["bash"], unknown: ["nope"] })
    const next = run()
    expect(next.tools.scratchpad_read).toBeDefined()
    expect(next.tools.scratchpad_write).toBeUndefined()
    expect(next.tools.mcp__linear__search).toBeDefined()
  })
})

describe("ToolFamilies.loadOnCall", () => {
  test("a call to a deferred tool loads its family and asks for the call again", () => {
    run()
    const result = ToolFamilies.loadOnCall(sessionID, "Scratchpad_Write")
    expect(result?.group).toBe("scratchpad")
    expect(result?.tool).toBe("scratchpad_write")
    expect(result?.message).toContain("call scratchpad_write again")
    const next = run()
    expect(next.tools.scratchpad_read).toBeDefined()
    expect(next.tools.scratchpad_write).toBeDefined()
  })

  test("names that are not deferred tools of this session fall through", () => {
    run()
    expect(ToolFamilies.loadOnCall(sessionID, "made_up_tool")).toBeUndefined()
    expect(ToolFamilies.loadOnCall(sessionID, "bash")).toBeUndefined()
  })
})

describe("loaded set", () => {
  test("survives a restart (state file)", async () => {
    run()
    await search("github")
    ToolFamilies.loadOnCall(sessionID, "automation")
    ToolFamilies.reset()
    const next = run()
    expect(next.tools.mcp__github__create_issue).toBeDefined()
    expect(next.tools.automation).toBeDefined()
    expect(next.tools.send_agent_email).toBeUndefined()
  })

  test("is rebuilt from history when the state file is gone", () => {
    const messages = [
      {
        info: { id: "msg_1", role: "assistant" },
        parts: [
          {
            type: "tool",
            tool: "tool_search",
            state: { status: "completed", input: {}, output: "", title: "", metadata: { loaded: ["scratchpad_read"] }, time: { start: 0, end: 0 } },
          },
          { type: "tool", tool: "send_agent_email", state: { status: "completed", input: {}, output: "", title: "", metadata: {}, time: { start: 0, end: 0 } } },
        ],
      },
    ] as unknown as MessageV2.WithParts[]
    const next = run(messages)
    expect(next.tools.scratchpad_read).toBeDefined()
    expect(next.tools.scratchpad_write).toBeUndefined()
    expect(next.tools.get_agent_email_status).toBeDefined()
  })
})

describe("kill switch", () => {
  test("GIZZI_DISABLE_TOOL_SEARCH loads every tool, drops tool_search and turns off load on call", async () => {
    process.env[DISABLE_TOOL_SEARCH_ENV] = "1"
    const result = run()
    expect(result.deferred).toEqual([])
    expect(result.tools.tool_search).toBeUndefined()
    expect(result.tools.send_agent_email).toBeDefined()
    expect(result.tools.mcp__linear__search).toBeDefined()
    expect(ToolFamilies.loadOnCall(sessionID, "automation")).toBeUndefined()
    expect((await search("select:automation")).already).toEqual(["automation"])
  })
})
