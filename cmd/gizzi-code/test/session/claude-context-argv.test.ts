import { describe, expect, test } from "bun:test"
import { claudeContextArgv } from "@/runtime/drivers/cli-session-flags"

describe("claudeContextArgv", () => {
  test("keeps the session's flags, drops the turn's stream flags, asks /context", () => {
    const turn = [
      "claude", "-p",
      "--output-format", "stream-json",
      "--input-format", "stream-json",
      "--verbose", "--include-partial-messages",
      "--permission-mode", "bypassPermissions",
      "--resume", "old",
      "--append-system-prompt", "be brief",
      "--mcp-config", "{}",
      "--model", "opus",
    ]
    expect(claudeContextArgv(turn, "vendor-1")).toEqual([
      "claude", "-p",
      "--permission-mode", "bypassPermissions",
      "--append-system-prompt", "be brief",
      "--mcp-config", "{}",
      "--model", "opus",
      "--output-format", "json", "--no-session-persistence", "--resume", "vendor-1", "/context",
    ])
  })
})

describe("claude-cli model", () => {
  test("the session's picked model reaches the CLI; placeholders do not", async () => {
    const { cliAdapterArgv } = await import("@/runtime/drivers/local-cli-driver")
    const argv = (model?: string) => cliAdapterArgv("claude-cli", ["claude"], "hi", { taskId: "t1", model })
    const flag = (args: string[]) => (args.includes("--model") ? args[args.indexOf("--model") + 1] : undefined)
    expect(flag(argv("claude-sonnet-5"))).toBe("claude-sonnet-5")
    expect(flag(argv("default"))).toBe(process.env.MULTICA_CLAUDE_MODEL || undefined)
    expect(flag(argv("claude-cli"))).toBe(process.env.MULTICA_CLAUDE_MODEL || undefined)
  })
})

describe("codexContext", () => {
  test("reads the last request against the window, not the thread total", async () => {
    const { codexContext } = await import("@/runtime/drivers/local-cli-driver")
    expect(
      codexContext({
        total: { inputTokens: 349_530, outputTokens: 1_877, totalTokens: 351_407 },
        last: { inputTokens: 57_770, outputTokens: 226, totalTokens: 57_996 },
        modelContextWindow: 475_000,
      }),
    ).toEqual({ used: 57_996, size: 475_000 })
    expect(codexContext({ last: { totalTokens: 10 }, modelContextWindow: null })).toBeUndefined()
  })
})
