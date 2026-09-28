import { describe, expect, test } from "bun:test"
import {
  buildRunTelemetryLine,
  contextBlockBar,
  formatCompactTokens,
  formatResetsIn,
  formatTelemetryCost,
  formatTelemetrySeconds,
  pickTightestQuotaWindow,
  QUOTA_NA_CHIP,
  quotaChipFromResult,
  quotaWindowChip,
} from "../../src/cli/ui/ink-app/utils/telemetry/runTelemetryModel"
import {
  quotaChipForProvider,
  quotaProviderSupported,
  resolveQuotaProviderId,
} from "../../src/cli/ui/ink-app/utils/telemetry/providerQuota"
import { createRunTelemetryMessage } from "../../src/cli/ui/ink-app/utils/messages"
import {
  contextRatioFromMessages,
  reasoningTokensFromMessages,
  turnUsageEstimated,
} from "../../src/cli/ui/ink-app/utils/telemetry/turnSignals"
import type { Message } from "../../src/cli/ui/ink-app/types/message"

describe("formatCompactTokens", () => {
  test("formats k and M like the desktop compact()", () => {
    expect(formatCompactTokens(345)).toBe("345")
    expect(formatCompactTokens(1234)).toBe("1.2k")
    expect(formatCompactTokens(12345)).toBe("12k")
    expect(formatCompactTokens(1234567)).toBe("1.2M")
    expect(formatCompactTokens(12345678)).toBe("12M")
  })
})

describe("formatTelemetrySeconds", () => {
  test("sub-10s keeps one decimal, then rounds, then minutes", () => {
    expect(formatTelemetrySeconds(4321)).toBe("4.3s")
    expect(formatTelemetrySeconds(45000)).toBe("45s")
    expect(formatTelemetrySeconds(125000)).toBe("2m 5s")
  })
})

describe("formatTelemetryCost", () => {
  test("sub-cent costs keep 4 decimals", () => {
    expect(formatTelemetryCost(0.0012)).toBe("$0.0012")
    expect(formatTelemetryCost(0.25)).toBe("$0.25")
    expect(formatTelemetryCost(3.5)).toBe("$3.50")
  })
})

describe("formatResetsIn", () => {
  const now = Date.parse("2026-09-26T12:00:00Z")
  test("minutes, hours, days", () => {
    expect(formatResetsIn("2026-09-26T12:45:00Z", now)).toBe("45m")
    expect(formatResetsIn("2026-09-26T15:12:00Z", now)).toBe("3h 12m")
    expect(formatResetsIn("2026-09-29T04:00:00Z", now)).toBe("2d 16h")
  })
  test("undefined when absent, invalid, or in the past", () => {
    expect(formatResetsIn(undefined, now)).toBeUndefined()
    expect(formatResetsIn("not-a-date", now)).toBeUndefined()
    expect(formatResetsIn("2026-09-26T11:00:00Z", now)).toBeUndefined()
  })
})

describe("contextBlockBar", () => {
  test("fills proportionally with a 1-block minimum when non-zero", () => {
    expect(contextBlockBar(0)).toBe("░░░░░░░░░░")
    expect(contextBlockBar(0.62)).toBe("██████░░░░")
    expect(contextBlockBar(0.01)).toBe("█░░░░░░░░░")
    expect(contextBlockBar(1)).toBe("██████████")
    expect(contextBlockBar(1.5)).toBe("██████████")
  })
})

describe("quota window selection", () => {
  const windows = [
    { id: "5h", label: "5-hour", usedRatio: 0.38 },
    { id: "7d", label: "Weekly", usedRatio: 0.8 },
    { id: "month", label: "Monthly", usedRatio: 0.1 },
  ]
  test("pickTightestQuotaWindow picks the most-used window", () => {
    expect(pickTightestQuotaWindow(windows)?.id).toBe("7d")
    expect(pickTightestQuotaWindow([])).toBeNull()
  })
  test("quotaWindowChip labels 5h/7d/month and reports percent left", () => {
    expect(quotaWindowChip(windows[0]!)).toBe("5h 62% left")
    expect(quotaWindowChip(windows[1]!)).toBe("week 20% left")
    expect(quotaWindowChip(windows[2]!)).toBe("month 90% left")
  })
  test("quotaChipFromResult renders only for ok results with windows", () => {
    expect(quotaChipFromResult({ status: "ok", quota: { windows } })).toBe("week 20% left")
    expect(quotaChipFromResult({ status: "ok", quota: { windows: [] } })).toBeNull()
    expect(quotaChipFromResult({ status: "unsupported" })).toBeNull()
    expect(quotaChipFromResult({ status: "signed-out", message: "x" })).toBeNull()
    expect(quotaChipFromResult(undefined)).toBeNull()
  })
  test("QUOTA_NA_CHIP is the explicit no-quota-API marker", () => {
    expect(QUOTA_NA_CHIP).toBe("quota n/a")
    expect(buildRunTelemetryLine({ quotaChip: QUOTA_NA_CHIP })).toBe("quota n/a")
  })
})

describe("buildRunTelemetryLine", () => {
  test("assembles the full line", () => {
    expect(
      buildRunTelemetryLine({
        model: "Kimi K2",
        durationMs: 12345,
        inputTokens: 1234,
        outputTokens: 345,
        toolCount: 3,
        costUSD: 0.0123,
        contextRatio: 0.58,
        quotaChip: "week 20% left",
      }),
    ).toBe("Kimi K2 · 12s · 1.2k in / 345 out · 3 tools · $0.01 · ctx ██████░░░░ 58% · week 20% left")
  })

  test("marks estimated token counts with ~ and est.", () => {
    expect(
      buildRunTelemetryLine({
        model: "Kimi K2",
        inputTokens: 1234,
        outputTokens: 345,
        usageEstimated: true,
      }),
    ).toBe("Kimi K2 · ~1.2k in / ~345 out est.")
    expect(
      buildRunTelemetryLine({ contextRatio: 0.58, contextEstimated: true }),
    ).toBe("ctx ██████░░░░ ~58%")
  })

  test("omits segments whose data is absent — never zero-fills", () => {
    expect(buildRunTelemetryLine({ model: "Kimi K2" })).toBe("Kimi K2")
    expect(
      buildRunTelemetryLine({ model: "Kimi K2", inputTokens: 0, outputTokens: 0, costUSD: 0 }),
    ).toBe("Kimi K2")
    expect(buildRunTelemetryLine({ toolCount: 1 })).toBe("1 tool")
  })

  test("returns null when there is nothing to say", () => {
    expect(buildRunTelemetryLine({})).toBeNull()
    expect(buildRunTelemetryLine({ durationMs: 0 })).toBeNull()
  })
})

describe("createRunTelemetryMessage", () => {
  test("builds a system/run_telemetry message with the assembled line", () => {
    const msg = createRunTelemetryMessage({
      model: "Kimi K2",
      durationMs: 5000,
      inputTokens: 100,
      outputTokens: 50,
      contextRatio: 0.5,
    })
    expect(msg).not.toBeNull()
    expect(msg!.type).toBe("system")
    expect(msg!.subtype).toBe("run_telemetry")
    expect(msg!.content).toBe("Kimi K2 · 5.0s · 100 in / 50 out · ctx █████░░░░░ 50%")
    expect(msg!.modelDisplay).toBe("Kimi K2")
    expect(msg!.isMeta).toBe(false)
    expect(typeof msg!.uuid).toBe("string")
  })

  test("returns null when the line would be empty", () => {
    expect(createRunTelemetryMessage({})).toBeNull()
  })
})

function assistantWithUsage(usage: Record<string, unknown>, extra: Record<string, unknown> = {}): Message {
  return {
    type: "assistant",
    message: {
      role: "assistant",
      model: "test-model",
      content: [{ type: "text", text: "hi" }],
      usage: { input_tokens: 100, output_tokens: 50, ...usage },
      ...extra,
    },
  } as unknown as Message
}

describe("turnSignals", () => {
  test("turnUsageEstimated scans only messages from the baseline on", () => {
    const reported = assistantWithUsage({})
    const estimated = assistantWithUsage({}, { usageEstimated: true })
    expect(turnUsageEstimated([estimated, reported], 1)).toBe(false)
    expect(turnUsageEstimated([reported, estimated], 1)).toBe(true)
    expect(turnUsageEstimated([reported], 0)).toBe(false)
  })

  test("contextRatioFromMessages uses input + cache over the window", () => {
    const m = assistantWithUsage({
      input_tokens: 40_000,
      cache_creation_input_tokens: 5_000,
      cache_read_input_tokens: 5_000,
    })
    expect(contextRatioFromMessages([m], 200_000)).toBe(0.25)
    expect(contextRatioFromMessages([m], 0)).toBeNull()
    expect(contextRatioFromMessages([], 200_000)).toBeNull()
  })

  test("reasoningTokensFromMessages reads either reasoning field", () => {
    expect(reasoningTokensFromMessages([assistantWithUsage({ reasoning_tokens: 800 })])).toBe(800)
    expect(reasoningTokensFromMessages([assistantWithUsage({ reasoning_output_tokens: 300 })])).toBe(300)
    expect(reasoningTokensFromMessages([assistantWithUsage({})])).toBe(0)
    expect(reasoningTokensFromMessages([])).toBe(0)
  })
})


describe("providerQuota bridge", () => {
  test("quotaProviderSupported mirrors the runtime fetcher registry", () => {
    expect(quotaProviderSupported("kimi-cli")).toBe(true)
    expect(quotaProviderSupported("openrouter")).toBe(true)
    expect(quotaProviderSupported("anthropic")).toBe(false)
    // claude-cli and codex-cli gained quota readers in P3.17 (#821).
    expect(quotaProviderSupported("claude-cli")).toBe(true)
    expect(quotaProviderSupported("codex-cli")).toBe(true)
  })

  test("quotaChipForProvider returns the n/a marker for providers with no quota API, with no network", async () => {
    // "anthropic" has no fetcher — the marker must come back without any fetch.
    expect(await quotaChipForProvider("anthropic")).toBe(QUOTA_NA_CHIP)
  })

  test("resolveQuotaProviderId claims only providers with a fetcher for model-prefixed strings", () => {
    // models.dev providers never pass through Discovery, so only the
    // supported-prefix path may claim them.
    expect(resolveQuotaProviderId("openrouter/anthropic/claude-opus-4")).toBe("openrouter")
    expect(resolveQuotaProviderId("anthropic/claude-opus-4")).toBeUndefined()
    expect(resolveQuotaProviderId(undefined)).toBeUndefined()
  })
})
