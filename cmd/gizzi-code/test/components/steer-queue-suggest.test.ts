import { afterEach, describe, expect, test } from "bun:test"
import {
  clearCommandQueue,
  enqueue,
  getCommandQueue,
  isQueuedForAfterTurn,
  removeLastQueuedForAfterTurn,
} from "../../src/cli/ui/ink-app/utils/messageQueueManager"
import {
  getSuggestionCostSuppressReason,
  isNoExtraCostSuggestionModel,
} from "../../src/cli/ui/ink-app/services/PromptSuggestion/promptSuggestion"
import { getRecapFileChanges } from "../../src/cli/ui/ink-app/services/awaySummary"

afterEach(() => clearCommandQueue())

describe("steer vs queue-for-after while the agent works", () => {
  test("enter (default priority) is a steer; tab ('later') waits for the turn to end", () => {
    enqueue({ value: "use vitest not bun test", mode: "prompt" })
    enqueue({ value: "then update the docs", mode: "prompt", priority: "later" })
    const [steer, after] = getCommandQueue()
    expect(isQueuedForAfterTurn(steer!)).toBe(false)
    expect(isQueuedForAfterTurn(after!)).toBe(true)
  })

  test("system notifications at 'later' are not the user's queued messages", () => {
    enqueue({ value: "<task-notification/>", mode: "task-notification", priority: "later" })
    enqueue({ value: "tick", mode: "prompt", priority: "later", isMeta: true })
    expect(getCommandQueue().some(isQueuedForAfterTurn)).toBe(false)
    expect(removeLastQueuedForAfterTurn()).toBeUndefined()
    expect(getCommandQueue()).toHaveLength(2)
  })

  test("esc removes the newest queued-for-after message and leaves steers", () => {
    enqueue({ value: "steer", mode: "prompt" })
    enqueue({ value: "first after", mode: "prompt", priority: "later" })
    enqueue({ value: "second after", mode: "prompt", priority: "later" })
    expect(removeLastQueuedForAfterTurn()?.value).toBe("second after")
    expect(getCommandQueue().map(c => c.value)).toEqual(["steer", "first after"])
  })
})

describe("next-step suggestions never spend a metered key silently", () => {
  test("CLI brains cost nothing extra", () => {
    expect(isNoExtraCostSuggestionModel("claude-cli/claude-sonnet-5")).toBe(true)
    expect(isNoExtraCostSuggestionModel("codex-cli/gpt-5.5")).toBe(true)
    expect(getSuggestionCostSuppressReason("kimi-cli/kimi-k3")).toBeNull()
  })

  test("Allternit Cloud is allowed; Anthropic and OpenRouter keys are not", () => {
    expect(getSuggestionCostSuppressReason("allternit/kimi-k3")).toBeNull()
    expect(getSuggestionCostSuppressReason("claude-sonnet-5")).toBe("metered_model")
    expect(getSuggestionCostSuppressReason("openrouter/z-ai/glm-4.7-flash")).toBe("metered_model")
    expect(getSuggestionCostSuppressReason(undefined)).toBe("metered_model")
  })
})

describe("recap file changes come from edit results", () => {
  const edit = (filePath: string, lines: string[]) =>
    ({ type: "user", toolUseResult: { filePath, structuredPatch: [{ lines }] }, message: { content: [] } }) as never

  test("sums +/- per file across edits and counts a new file's lines", () => {
    const files = getRecapFileChanges([
      edit("/r/src/auth.ts", ["+a", "-b", " c"]),
      edit("/r/src/auth.ts", ["+d", "+e"]),
      { type: "user", toolUseResult: { filePath: "/r/new.ts", type: "create", content: "x\ny", structuredPatch: [] }, message: { content: [] } } as never,
      { type: "assistant", message: { content: [] } } as never,
    ])
    expect(files).toEqual([
      { path: "/r/src/auth.ts", added: 3, removed: 1 },
      { path: "/r/new.ts", added: 2, removed: 0 },
    ])
  })
})
