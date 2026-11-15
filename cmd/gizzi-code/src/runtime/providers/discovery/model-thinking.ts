/**
 * Thinking settings per model (model-thinking.json): the levels a model
 * takes, its default and the vendor it belongs to. allternit-api serves the
 * same table to the pickers and checks the chosen level; gizzi turns a level
 * into the vendor's own option (transform.variants) or the CLI's flag.
 */
import table from "./model-thinking.json"

export namespace ModelThinking {
  export type Vendor = "claude" | "openai" | "google" | "deepseek" | "zai"
  export interface Entry {
    vendor: Vendor
    levels: string[]
    default: string
    fast: boolean
  }

  /** `anthropic/claude-opus-5.5` → `claude-opus-5-5`. */
  function normalize(modelID: string): string {
    const bare = modelID.slice(modelID.lastIndexOf("/") + 1)
    return bare.trim().toLowerCase().replaceAll(".", "-")
  }

  export function of(modelID: string): Entry | undefined {
    const id = normalize(modelID)
    const hit = table.models.find((m) =>
      m.match.some((p) => (p.endsWith("*") ? id.startsWith(p.slice(0, -1)) : id === p)),
    )
    if (!hit) return undefined
    return { vendor: hit.vendor as Vendor, levels: hit.levels, default: hit.default, fast: Boolean((hit as { fast?: boolean }).fast) }
  }

  export interface Described {
    /** The vendor's word for it: Effort, Reasoning, Thinking level… */
    term: string
    levels: { id: string; label: string }[]
    default: string
  }

  /** Levels with the vendor's own labels, for pickers (same table as Desktop's). */
  export function describe(modelID: string): Described | undefined {
    const entry = of(modelID)
    if (!entry || entry.levels.length === 0) return undefined
    const vendor = (table.vendors as Record<string, { term?: string; labels?: Record<string, string> }>)[entry.vendor]
    return {
      term: vendor?.term ?? "Effort",
      levels: entry.levels.map((id) => ({ id, label: vendor?.labels?.[id] ?? id })),
      default: entry.default,
    }
  }

  /** The level when this model takes it, else undefined (the model's default applies). */
  export function check(modelID: string, level: string | undefined): string | undefined {
    if (!level) return undefined
    const want = level.trim().toLowerCase()
    return of(modelID)?.levels.find((l) => l === want)
  }
}
