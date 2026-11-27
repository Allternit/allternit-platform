import { describe, expect, test } from "bun:test"
import {
  brainAccessFor,
  lockedNotice,
  nextRecentModel,
  offlineNotice,
  pushRecentModel,
} from "../../src/cli/ui/ink-app/utils/model/modelPickerModel"
import { buildModelPickerRows } from "../../src/cli/ui/ink-app/utils/model/modelOptions"
import type { ModelList } from "../../src/runtime/providers/model-list"

const row = (providerID: string, modelID: string, extra: Partial<ModelList.Row> = {}): ModelList.Row => ({
  ref: `${providerID}/${modelID}`,
  providerID,
  modelID,
  name: modelID,
  default: false,
  current: false,
  ...extra,
})

const sections: ModelList.Section[] = [
  {
    kind: "cloud",
    title: "Allternit Cloud",
    providers: [
      {
        id: "allternit",
        name: "Allternit Cloud",
        models: [
          row("allternit", "free-model"),
          row("allternit", "pro-model", { locked: { reason: "Needs credits", action: "add_credits" } }),
          row("allternit", "old-alias", { hidden: true }),
        ],
      },
    ],
  },
  {
    kind: "local",
    title: "Local",
    providers: [
      {
        id: "ollama",
        name: "Ollama",
        offline: { reason: "Nothing answered at http://localhost:11434", fix: "Start Ollama" },
        models: [],
      },
    ],
  },
]

describe("picker layers: rows", () => {
  const { rows } = buildModelPickerRows(sections, [], {})
  const options = rows.flatMap(r => (r.kind === "option" ? [r] : []))

  test("locked row carries its lock and the credits badge; Enter text points at billing", () => {
    const locked = options.find(o => o.value === "allternit/pro-model")!
    expect(locked.locked?.action).toBe("add_credits")
    expect(locked.access).toBe("credits")
    expect(lockedNotice(locked.locked!, "https://billing.example")).toContain("https://billing.example")
    expect(lockedNotice({ reason: "Sign in.", action: "sign_in" }, "x")).toContain("gizzi login")
  })

  test("offline provider is exactly one row, with no option children", () => {
    const offline = rows.filter(r => r.kind === "offline")
    expect(offline).toHaveLength(1)
    expect(offline[0]).toMatchObject({ title: "Ollama — offline", fix: "Start Ollama" })
    expect(options.some(o => o.providerId === "ollama")).toBe(false)
    expect(offlineNotice({ reason: "Down", fix: "Start it" })).toBe("Down. Start it")
  })

  test("hidden models are not rendered", () => {
    expect(options.some(o => o.value === "allternit/old-alias")).toBe(false)
  })

  test("a hidden model that is the current choice still resolves", () => {
    const again = buildModelPickerRows(sections, [], { current: "allternit/old-alias" }).rows
    expect(again.some(r => r.kind === "option" && r.value === "allternit/old-alias")).toBe(true)
  })
})

describe("picker layers: access + recents", () => {
  test("access badge per section kind", () => {
    expect(brainAccessFor("subscription")).toBe("plan")
    expect(brainAccessFor("api")).toBe("key")
    expect(brainAccessFor("local")).toBe("local")
    expect(brainAccessFor("cloud")).toBe("credits")
  })

  test("recent list keeps 5, newest first, no duplicates", () => {
    let r: string[] = []
    for (const m of ["a", "b", "c", "d", "e", "f", "c"]) r = pushRecentModel(r, m)
    expect(r).toEqual(["c", "f", "e", "d", "b"])
  })

  test("F2 cycles the recent models and wraps; skips locked ones", () => {
    const r = ["a", "b", "c"]
    expect(nextRecentModel(r, "a")).toBe("b")
    expect(nextRecentModel(r, "c")).toBe("a")
    expect(nextRecentModel(r, "zzz")).toBe("a")
    expect(nextRecentModel(r, "a", v => v !== "b")).toBe("c")
    expect(nextRecentModel([], "a")).toBeUndefined()
  })
})
