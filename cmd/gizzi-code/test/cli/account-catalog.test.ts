import { describe, expect, test } from "bun:test"
import { mkdtemp, readFile, writeFile } from "fs/promises"
import { tmpdir } from "os"
import { join } from "path"
import {
  accountLines,
  creditsChip,
  getAllternitAccount,
  resetAllternitAccountCache,
} from "../../src/cli/ui/ink-app/services/allternitAccount"
import { mcpEntry, SECTIONS, writeGizziMcp } from "../../src/cli/ui/ink-app/commands/customize/catalog"
import { appViewFor, prepareAppView } from "../../src/cli/ui/ink-app/services/mcp/appView"
import { ModelThinking } from "../../src/runtime/providers/discovery/model-thinking"
import { getDisplayedThinkingLabel, resolveAppliedEffort } from "../../src/cli/ui/ink-app/utils/effort"

// The network is the edge: /api/v1/me/usage answered as cloud-api does.
function usageFetch(body: unknown, status = 200): typeof fetch {
  return (async () => new Response(JSON.stringify(body), { status })) as unknown as typeof fetch
}

describe("Allternit account (/usage, footer chip)", () => {
  test("Max plan (id super) shows as Max with credits and the monthly meter", async () => {
    process.env.ALLTERNIT_API_TOKEN = "t"
    resetAllternitAccountCache()
    const result = await getAllternitAccount({
      fresh: true,
      fetchImpl: usageFetch({ plan: "super", status: "active", weeklyUsed: 55, weeklyLimit: 110, credits: 41.2, monthToDateUsageUsd: 12 }),
    })
    expect(result.status).toBe("ok")
    expect(creditsChip(result)).toBe("credits $41.20")
    const lines = accountLines(result, () => "##", () => "3d").join("\n")
    expect(lines).toContain("Plan:      Max")
    expect(lines).toContain("$41.20 Cloud balance")
    expect(lines).toContain("$55.00 of $110.00 · resets in 3d")
  })

  test("a rejected sign-in is signed-out, never a plan", async () => {
    process.env.ALLTERNIT_API_TOKEN = "t"
    resetAllternitAccountCache()
    const result = await getAllternitAccount({ fresh: true, fetchImpl: usageFetch({ error: "Unauthorized" }, 401) })
    expect(result.status).toBe("signed-out")
    expect(creditsChip(result)).toBeNull()
  })
})

describe("per-model thinking levels (/model second column)", () => {
  test("a level the model takes reaches it; one it doesn't falls back to the model default", () => {
    expect(String(resolveAppliedEffort("claude-cli/claude-opus-5-5", "xhigh" as never))).toBe("xhigh")
    expect(resolveAppliedEffort("claude-cli/claude-opus-5-5", "ultra" as never)).toBeUndefined()
    expect(String(resolveAppliedEffort("openai/gpt-6-sol", "ultra" as never))).toBe("ultra")
    expect(getDisplayedThinkingLabel("claude-cli/claude-opus-5-5", undefined)).toBe("medium")
    expect(ModelThinking.describe("gemini-3-pro")?.levels.map((l) => l.id)).toEqual(["low", "high"])
  })
})

describe("/customize Add", () => {
  test("writes the MCP entry into gizzi.json and keeps the rest of the file", async () => {
    const dir = await mkdtemp(join(tmpdir(), "gz-cust-"))
    await writeFile(join(dir, "gizzi.json"), JSON.stringify({ model: "x", mcp: { mine: { type: "remote", url: "u" } } }))
    const fs = SECTIONS.find((s) => s.id === "mcps")!.shelves[1]!.items.find((i) => i.name === "Filesystem")!
    const add = (fs.add as { mcp: Parameters<typeof mcpEntry>[0] }).mcp
    await writeGizziMcp(add.key, mcpEntry(add, "/home/me"), dir)
    const written = JSON.parse(await readFile(join(dir, "gizzi.json"), "utf8"))
    expect(written.model).toBe("x")
    expect(written.mcp.mine.url).toBe("u")
    expect(written.mcp[add.key].command).toContain("/home/me")
  })

  test("never rewrites a gizzi.json that isn't plain JSON", async () => {
    const dir = await mkdtemp(join(tmpdir(), "gz-cust-"))
    await writeFile(join(dir, "gizzi.json"), "{ // comment\n}")
    await expect(writeGizziMcp("k", {}, dir)).rejects.toThrow("gizzi mcp add")
    expect(await readFile(join(dir, "gizzi.json"), "utf8")).toBe("{ // comment\n}")
  })
})

describe("MCP Apps view", () => {
  test("a ui:// result gets a browser page carrying the app and the tool result", async () => {
    const content = [{ type: "text", text: "3 flights" }]
    await prepareAppView({
      readResource: async () => ({ contents: [{ text: "<div id=app>flights</div>" }] }),
      resourceUri: "ui://flights/view",
      title: "Flights",
      input: { to: "SFO" },
      result: { content, structuredContent: { n: 3 } },
    })
    const view = appViewFor(content)
    expect(view?.url.startsWith("file://")).toBe(true)
    const page = await readFile(view!.file, "utf8")
    expect(page).toContain("ui/notifications/tool-result")
    expect(page).toContain("\\u003cdiv id=app>flights\\u003c/div>")
    expect(page).toContain('"to":"SFO"')
    expect(appViewFor([{ type: "text", text: "other" }])).toBeUndefined()
  })
})
