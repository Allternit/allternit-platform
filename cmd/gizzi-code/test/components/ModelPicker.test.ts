import { describe, expect, test } from "bun:test"
import {
  buildPickerRows,
  capabilityBadges,
  formatContext,
  inferVendor,
  quotaMarker,
  quotaSummary,
  selectableValues,
  toggleFavorite,
  vendorDisplayName,
  visibleWindow,
  type PickerOption,
  type PickerProviderMeta,
} from "../../src/cli/ui/ink-app/utils/model/modelPickerModel"

const CLOUD: PickerProviderMeta = {
  providerId: "allternit",
  providerName: "Allternit Cloud",
  source: "platform",
  context: 200_000,
}
const KIMI: PickerProviderMeta = {
  providerId: "kimi-cli",
  providerName: "Kimi For Coding",
  source: "subprocess",
  context: 262_144,
}
const MLX: PickerProviderMeta = {
  providerId: "local-mlx",
  providerName: "MLX Qwen",
  source: "local",
}

function metaFor(map: Record<string, PickerProviderMeta>) {
  return (value: string) => map[value]
}

describe("inferVendor", () => {
  test("maps model ids/names to upstream vendors", () => {
    expect(inferVendor({ id: "claude-opus-4-6" })).toBe("anthropic")
    expect(inferVendor({ id: "sonnet" })).toBe("anthropic")
    expect(inferVendor({ id: "gpt-5" })).toBe("openai")
    expect(inferVendor({ id: "codex-mini" })).toBe("openai")
    expect(inferVendor({ id: "gemini-3-pro" })).toBe("google")
    expect(inferVendor({ id: "grok-4" })).toBe("xai")
    expect(inferVendor({ id: "deepseek-v3" })).toBe("deepseek")
    expect(inferVendor({ id: "kimi-k2" })).toBe("kimi")
    expect(inferVendor({ id: "qwen3-coder" })).toBe("qwen")
    expect(inferVendor({ id: "mistral-large" })).toBe("mistral")
    expect(inferVendor({ id: "llama-4" })).toBe("meta")
    expect(inferVendor({ id: "something-else" })).toBe("other")
  })

  test("matches on the display name too", () => {
    expect(inferVendor({ id: "m1", name: "Claude Opus" })).toBe("anthropic")
    expect(inferVendor({ id: "m2", name: "Moonshot K2" })).toBe("kimi")
  })
})

describe("formatContext", () => {
  test("formats K and M windows", () => {
    expect(formatContext(200_000)).toBe("200K")
    expect(formatContext(8_192)).toBe("8K")
    expect(formatContext(1_000_000)).toBe("1.0M")
    expect(formatContext(1_048_576)).toBe("1.0M")
    expect(formatContext(500)).toBe("500")
  })

  test("returns null for unknown or invalid values", () => {
    expect(formatContext(undefined)).toBeNull()
    expect(formatContext(null)).toBeNull()
    expect(formatContext(0)).toBeNull()
    expect(formatContext(-5)).toBeNull()
    expect(formatContext("nope")).toBeNull()
  })
})

describe("capabilityBadges", () => {
  test("tags reasoning, vision, and local with positive signals only", () => {
    expect(capabilityBadges({ id: "deepseek-r1" })).toContain("reasoning")
    expect(capabilityBadges({ id: "pixtral-12b" })).toContain("vision")
    expect(capabilityBadges({ id: "qwen3", source: "local" })).toEqual(["local"])
    expect(capabilityBadges({ id: "claude-opus-4-6" })).toEqual([])
  })
})

describe("buildPickerRows", () => {
  const options: PickerOption[] = [
    { value: "allternit/claude-opus-4-6", label: "Cloud · Opus", description: "Allternit Cloud · Allternit Cloud · allternit/claude-opus-4-6" },
    { value: "allternit/gpt-5", label: "Cloud · GPT-5", description: "Allternit Cloud · Allternit Cloud · allternit/gpt-5" },
    { value: "kimi-cli/kimi-k2", label: "CLI · Kimi K2", description: "Kimi For Coding · installed CLI · kimi-cli/kimi-k2" },
    { value: "local-mlx/qwen3", label: "Qwen 3", description: "MLX Qwen · local · local-mlx/qwen3" },
    { value: "opus", label: "Opus", description: "Opus 4.6 · Most capable" },
  ]
  const metas = metaFor({
    "allternit/claude-opus-4-6": CLOUD,
    "allternit/gpt-5": CLOUD,
    "kimi-cli/kimi-k2": KIMI,
    "local-mlx/qwen3": MLX,
  })

  test("groups Cloud by vendor, then subscriptions, then Local, then Other", () => {
    const { rows } = buildPickerRows(options, { metaFor: metas })
    const headers = rows.filter(r => r.kind === "header").map(r => (r as any).title)
    expect(headers).toEqual([
      "Anthropic · via Allternit Cloud",
      "OpenAI · via Allternit Cloud",
      "Kimi For Coding · your subscription",
      "Local",
      "Other",
    ])
    expect(selectableValues(rows)).toEqual([
      "allternit/claude-opus-4-6",
      "allternit/gpt-5",
      "kimi-cli/kimi-k2",
      "local-mlx/qwen3",
      "opus",
    ])
  })

  test("headers are not selectable", () => {
    const { rows } = buildPickerRows(options, { metaFor: metas })
    expect(selectableValues(rows)).not.toContain("hdr:cloud:anthropic")
    expect(selectableValues(rows)).toHaveLength(options.length)
  })

  test("context labels and badges ride on option rows", () => {
    const { rows } = buildPickerRows(options, { metaFor: metas })
    const opus = rows.find(r => r.kind === "option" && r.value === "allternit/claude-opus-4-6") as any
    expect(opus.contextLabel).toBe("200K")
    const kimi = rows.find(r => r.kind === "option" && r.value === "kimi-cli/kimi-k2") as any
    expect(kimi.contextLabel).toBe("262K")
    const local = rows.find(r => r.kind === "option" && r.value === "local-mlx/qwen3") as any
    expect(local.badges).toContain("local")
    const legacy = rows.find(r => r.kind === "option" && r.value === "opus") as any
    expect(legacy.contextLabel).toBeNull()
    expect(legacy.badges).toEqual([])
  })

  test("favorites do not jump across sections", () => {
    const { rows } = buildPickerRows(options, {
      metaFor: metas,
      favorites: ["allternit/gpt-5", "opus"],
    })
    expect(selectableValues(rows)).toEqual([
      "allternit/claude-opus-4-6",
      "allternit/gpt-5", // favorite, but stays in its own (OpenAI) section
      "kimi-cli/kimi-k2",
      "local-mlx/qwen3",
      "opus",
    ])
    const gpt = rows.find(r => r.kind === "option" && r.value === "allternit/gpt-5") as any
    expect(gpt.favorite).toBe(true)
  })

  test("favorites move ahead of non-favorites inside the same section", () => {
    const two: PickerOption[] = [
      { value: "allternit/claude-opus-4-6", label: "Cloud · Opus" },
      { value: "allternit/claude-sonnet-4-6", label: "Cloud · Sonnet" },
    ]
    const { rows } = buildPickerRows(two, {
      metaFor: metaFor({
        "allternit/claude-opus-4-6": CLOUD,
        "allternit/claude-sonnet-4-6": CLOUD,
      }),
      favorites: ["allternit/claude-sonnet-4-6"],
    })
    expect(selectableValues(rows)).toEqual([
      "allternit/claude-sonnet-4-6",
      "allternit/claude-opus-4-6",
    ])
  })

  test("query filters across label, id, and provider name", () => {
    const byLabel = buildPickerRows(options, { metaFor: metas, query: "opus" })
    expect(selectableValues(byLabel.rows).sort()).toEqual([
      "allternit/claude-opus-4-6",
      "opus",
    ])
    const byId = buildPickerRows(options, { metaFor: metas, query: "kimi-cli" })
    expect(selectableValues(byId.rows)).toEqual(["kimi-cli/kimi-k2"])
    const byProvider = buildPickerRows(options, { metaFor: metas, query: "mlx" })
    expect(selectableValues(byProvider.rows)).toEqual(["local-mlx/qwen3"])
    const none = buildPickerRows(options, { metaFor: metas, query: "zzz" })
    expect(none.matched).toBe(0)
    expect(none.rows).toEqual([])
    expect(none.total).toBe(options.length)
  })

  test("empty sections disappear when filtered out", () => {
    const { rows } = buildPickerRows(options, { metaFor: metas, query: "kimi" })
    const headers = rows.filter(r => r.kind === "header").map(r => (r as any).title)
    expect(headers).toEqual(["Kimi For Coding · your subscription"])
  })
})

describe("toggleFavorite", () => {
  test("adds and removes without duplicates", () => {
    expect(toggleFavorite([], "a")).toEqual(["a"])
    expect(toggleFavorite(["a"], "a")).toEqual([])
    expect(toggleFavorite(["a", "b"], "a")).toEqual(["b"])
    expect(toggleFavorite(["a"], "b")).toEqual(["a", "b"])
  })
})

describe("quotaSummary", () => {
  test("shows the tightest window as remaining percent", () => {
    expect(
      quotaSummary({
        status: "ok",
        quota: {
          windows: [
            { id: "5h", label: "5-hour", usedRatio: 0.38 },
            { id: "7d", label: "Weekly", usedRatio: 0.8 },
          ],
        },
      }),
    ).toBe("7d: 20% left")
  })

  test("unreported or failed quotas render nothing", () => {
    expect(quotaSummary(undefined)).toBeNull()
    expect(quotaSummary({ status: "unsupported" })).toBeNull()
    expect(quotaSummary({ status: "signed-out" })).toBeNull()
    expect(quotaSummary({ status: "error" })).toBeNull()
    expect(quotaSummary({ status: "ok", quota: { windows: [] } })).toBeNull()
  })
})

describe("quotaMarker", () => {
  const ok = {
    status: "ok" as const,
    quota: { windows: [{ id: "5h", label: "5-hour", usedRatio: 0.38 }] },
  }

  test("real summaries win over markers", () => {
    expect(quotaMarker(ok, true)).toBe("5h: 62% left")
    expect(quotaMarker(ok, false)).toBe("5h: 62% left")
  })

  test("providers with no quota API get the explicit n/a marker", () => {
    expect(quotaMarker(undefined, false)).toBe("quota n/a")
    expect(quotaMarker({ status: "unsupported" }, false)).toBe("quota n/a")
  })

  test("known fetch outcomes get short honest statuses", () => {
    expect(quotaMarker({ status: "signed-out", message: "Sign in." }, true)).toBe("quota: signed out")
    expect(quotaMarker({ status: "expired", message: "Expired." }, true)).toBe("quota: sign-in expired")
    expect(quotaMarker({ status: "error", message: "Down." }, true)).toBe("quota unavailable")
  })

  test("an in-flight fetch says nothing yet, and never fabricates", () => {
    expect(quotaMarker(undefined, true)).toBeNull()
    expect(quotaMarker({ status: "ok", quota: { windows: [] } }, true)).toBeNull()
  })
})

describe("visibleWindow", () => {
  test("keeps the focused index inside the window", () => {
    const rows = Array.from({ length: 30 }, (_, i) => i)
    expect(visibleWindow(rows, 0, 10)).toEqual(rows.slice(0, 10))
    const w = visibleWindow(rows, 25, 10)
    expect(w).toContain(25)
    expect(w).toHaveLength(10)
    expect(visibleWindow(rows, 29, 10)).toEqual(rows.slice(20, 30))
    expect(visibleWindow([1, 2, 3], 1, 10)).toEqual([1, 2, 3])
  })
})

describe("vendorDisplayName", () => {
  test("human labels with Other fallback", () => {
    expect(vendorDisplayName("anthropic")).toBe("Anthropic")
    expect(vendorDisplayName("zai")).toBe("Z.ai")
    expect(vendorDisplayName("unknown-thing")).toBe("Other")
  })
})
