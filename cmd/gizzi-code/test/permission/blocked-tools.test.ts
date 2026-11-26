import { afterEach, describe, expect, test } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { ToolDispatcher } from "../../src/runtime/tools/dispatch"
import { BlockedTools } from "../../src/runtime/tools/guard/blocked-tools"
import * as BotTurn from "../../src/runtime/bots/bot-turn"
import { tmpdir } from "../fixture/fixture"

afterEach(() => {
  BlockedTools.reset()
  BotTurn.reset()
})

describe("BlockedTools.match", () => {
  test("an MCP tool matches its own entry only, so one connection's block never covers another's", () => {
    const blocked = ["mcp__proxy__gmail__send_email"]
    expect(BlockedTools.match(blocked, "mcp__proxy__gmail__send_email")).toBe("mcp__proxy__gmail__send_email")
    expect(BlockedTools.match(blocked, "mcp__proxy__outlook__send_email")).toBeUndefined()
    expect(BlockedTools.match(blocked, "mcp__proxy__gmail__search")).toBeUndefined()
  })

  test("the execute_action form is matched on the call's actionId", () => {
    const blocked = [BlockedTools.executeActionName("gmail", "send_email")]
    expect(blocked[0]).toBe("execute_action:gmail:send_email")
    expect(BlockedTools.match(blocked, "mcp__allternit-connectors__execute_action", { actionId: "gmail.send_email", input: {} })).toBe(
      "execute_action:gmail:send_email",
    )
    expect(BlockedTools.match(blocked, "execute_action", { actionId: "Gmail.send_email" })).toBe("execute_action:gmail:send_email")
    // Another action or another connector is not blocked.
    expect(BlockedTools.match(blocked, "mcp__c__execute_action", { actionId: "gmail.search" })).toBeUndefined()
    expect(BlockedTools.match(blocked, "mcp__c__execute_action", { actionId: "slack.send_email" })).toBeUndefined()
    // The entry is only read from execute_action calls.
    expect(BlockedTools.match(blocked, "mcp__c__other_tool", { actionId: "gmail.send_email" })).toBeUndefined()
    // The full action id as <action> is accepted too.
    expect(BlockedTools.match(["execute_action:gmail:gmail.send_email"], "mcp__c__execute_action", { actionId: "gmail.send_email" })).toBe(
      "execute_action:gmail:gmail.send_email",
    )
  })

  test("no list blocks nothing", () => {
    expect(BlockedTools.match(undefined, "mcp__a__b")).toBeUndefined()
    expect(BlockedTools.match([], "mcp__c__execute_action", { actionId: "gmail.send_email" })).toBeUndefined()
  })

  test("a subagent's session inherits its parent's list", async () => {
    BlockedTools.set("ses_parent", ["mcp__a__delete"])
    const parents: Record<string, string> = { ses_child: "ses_parent" }
    expect(await BlockedTools.forSession("ses_child", async (id) => parents[id])).toEqual(["mcp__a__delete"])
    expect(await BlockedTools.forSession("ses_other", async () => undefined)).toEqual([])
    // An empty push clears the session's list.
    BlockedTools.set("ses_parent", [])
    expect(BlockedTools.get("ses_parent")).toEqual([])
  })
})

describe("tool dispatcher: the account's blocked tools", () => {
  async function inInstance(fn: () => Promise<void>) {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({ directory: tmp.path, fn })
  }

  function ctx(sessionID: string): any {
    return { sessionID, messageID: "msg_1", agent: "build", abort: new AbortController().signal, messages: [], metadata: () => {}, ask: async () => {} }
  }

  test("a blocked tool is refused before it runs, with the tool-permissions reason", () => inInstance(async () => {
    BlockedTools.set("ses_blocked", ["mcp__proxy__gmail__send_email"])
    let executed = false
    const run = ToolDispatcher.executeInitialized("mcp__proxy__gmail__send_email", {}, ctx("ses_blocked"), async () => {
      executed = true
      return { title: "ok", output: "sent", metadata: {} }
    })
    await expect(run).rejects.toThrow(BlockedTools.REASON)
    expect(executed).toBe(false)
  }))

  test("a bot marker's block holds even when the bot's allowlist has the tool", () => inInstance(async () => {
    BotTurn.mark("ses_bot", {
      id: "agent-1",
      allowedTools: ["mcp__proxy__gmail__send_email"],
      blockedTools: ["execute_action:gmail:send_email", "mcp__proxy__gmail__send_email"],
    })
    let executed = false
    const exec = async () => {
      executed = true
      return { title: "ok", output: "sent", metadata: {} }
    }
    await expect(ToolDispatcher.executeInitialized("mcp__proxy__gmail__send_email", {}, ctx("ses_bot"), exec)).rejects.toThrow(
      BlockedTools.REASON,
    )
    await expect(
      ToolDispatcher.executeInitialized("mcp__connectors__execute_action", { actionId: "gmail.send_email" }, ctx("ses_bot"), exec),
    ).rejects.toThrow(BlockedTools.REASON)
    expect(executed).toBe(false)
  }))

  test("no blockedTools: the tool runs as before", () =>
    inInstance(async () => {
        BotTurn.mark("ses_plain", { id: "agent-1", allowedTools: ["mcp__proxy__gmail__send_email"] })
        const result: any = await ToolDispatcher.executeInitialized("mcp__proxy__gmail__send_email", {}, ctx("ses_plain"), async () => ({
          title: "ok",
          output: "sent",
          metadata: {},
        }))
        expect(result.output).toBe("sent")
    }))
})
