import { describe, test, expect } from "bun:test"
import { call, planQuotaLines } from "../../src/cli/ui/ink-app/commands/usage/usage"

describe("/usage slash command", () => {
  test("call returns an inline usage dashboard", async () => {
    const result = await call("", {
      getAppState: () => ({
        messages: [],
        toolPermissionContext: { mode: "ask" },
      }),
      options: {
        mainLoopModel: "claude-sonnet-4-6-20251001",
        mcpClients: [],
      },
    } as any)

    expect(result.type).toBe("text")
    const value = (result as any).value as string
    expect(value).toContain("Usage")
    expect(value).toContain("Model:")
    expect(value).toContain("Directory:")
    expect(value).toContain("Session:")
    expect(value).toContain("Context window")
    expect(value).toContain("Session totals")
    expect(value).toContain("Tokens:")
  })

  // fef505401 moved the per-model "Plan quota" block from /usage to /context
  // (/usage now shows the Allternit account and each CLI brain's limits);
  // planQuotaLines is what /context renders.
  test("plan quota section is honest when the provider reports nothing", async () => {
    const value = (await planQuotaLines("claude-sonnet-4-6-20251001")).join("\n")
    expect(value).toContain("Plan quota")
    // claude-* has no quota fetcher — the section must say so, not guess.
    expect(value).toContain("Not reported for this provider.")
    expect(value).not.toContain("% used")
  })
})
