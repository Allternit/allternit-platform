// Pure TS port of cmd/allternit-api/src/agency_api/evals/graders.rs.
// Same semantics, same param names (the Rust names win). No imports from other
// runtime modules: this file must stay usable from the CLI, the engine and tests.

export interface Step {
  node: string
  primitive: string
  ok: boolean
}

export interface Effect {
  node: string
  tool: string
  committed: boolean
}

/** What a run did, in the order it did it. Field names match the Rust struct. */
export interface Trajectory {
  status: string
  verifier?: boolean | null
  steps: Step[]
  effects: Effect[]
  texts?: string[]
  stopped_at?: string | null
  missing?: string[]
  cost_usd: number
  tokens: number
  latency_ms: number
}

export type GraderSpec =
  | { type: "step_order"; steps: string[] }
  | { type: "required_steps"; steps: string[] }
  | { type: "forbidden_step"; steps: string[] }
  | { type: "effect_count"; min?: number; max?: number; tool?: string }
  | { type: "must_cite"; min: number; marker: string }
  | { type: "verifier_pass" }
  | { type: "cost_ceiling"; max_usd?: number; max_tokens?: number }
  | { type: "latency_ceiling"; max_ms: number }
  | { type: "status"; equals: string }
  | { type: "stopped_at"; step: string }
  | { type: "missing_evidence"; equals: string[] }

export interface Grade {
  type: GraderSpec["type"]
  pass: boolean
  detail: string
}

export const GRADER_TYPES = [
  "step_order",
  "required_steps",
  "forbidden_step",
  "effect_count",
  "must_cite",
  "verifier_pass",
  "cost_ceiling",
  "latency_ceiling",
  "status",
  "stopped_at",
  "missing_evidence",
] as const

type Obj = Record<string, unknown>

function stepMatches(s: Step, spec: string): boolean {
  return s.node === spec || s.primitive === spec
}

function strings(v: Obj, key: string, ty: string): string[] {
  const a = v[key]
  if (!Array.isArray(a) || a.length === 0 || a.length > 64) throw new Error(`${ty}.${key} must list 1-64 step ids`)
  return a.map((s) => {
    if (typeof s !== "string" || s.length === 0 || s.length > 128) throw new Error(`${ty}.${key} entries must be step ids`)
    return s
  })
}

function known(v: Obj, ty: string, keys: string[]) {
  const k = Object.keys(v).find((k) => k !== "type" && !keys.includes(k))
  if (k !== undefined) throw new Error(`${ty}: unknown field \`${k}\``)
}

function num(v: Obj, key: string, ty: string): number | undefined {
  const x = v[key]
  if (x === undefined || x === null) return undefined
  if (typeof x !== "number" || !Number.isFinite(x) || x < 0) throw new Error(`${ty}.${key} must be a non-negative number`)
  return x
}

/** Validate a grader spec (same rules as Rust `parse`). Throws a clear Error. */
export function parseGrader(json: unknown): GraderSpec {
  if (typeof json !== "object" || json === null || Array.isArray(json)) throw new Error("a grader needs a type")
  const v = json as Obj
  const ty = v.type
  if (typeof ty !== "string") throw new Error("a grader needs a type")
  switch (ty) {
    case "step_order":
    case "required_steps":
    case "forbidden_step":
      known(v, ty, ["steps"])
      return { type: ty, steps: strings(v, "steps", ty) }
    case "effect_count": {
      known(v, ty, ["min", "max", "tool"])
      const min = num(v, "min", ty)
      const max = num(v, "max", ty)
      if (min === undefined && max === undefined) throw new Error("effect_count needs min or max")
      const lo = min === undefined ? undefined : Math.trunc(min)
      const hi = max === undefined ? undefined : Math.trunc(max)
      if (lo !== undefined && hi !== undefined && lo > hi) throw new Error("effect_count.min is above max")
      const out: Extract<GraderSpec, { type: "effect_count" }> = { type: ty }
      if (lo !== undefined) out.min = lo
      if (hi !== undefined) out.max = hi
      if (typeof v.tool === "string") out.tool = v.tool
      return out
    }
    case "must_cite": {
      known(v, ty, ["min", "marker"])
      const marker = typeof v.marker === "string" ? v.marker : "[src:"
      if (marker.length === 0 || marker.length > 32) throw new Error("must_cite.marker must be 1-32 characters")
      return { type: ty, min: Math.trunc(num(v, "min", ty) ?? 1), marker }
    }
    case "verifier_pass":
      known(v, ty, [])
      return { type: ty }
    case "cost_ceiling": {
      known(v, ty, ["max_usd", "max_tokens"])
      const max_usd = num(v, "max_usd", ty)
      const max_tokens = num(v, "max_tokens", ty)
      if (max_usd === undefined && max_tokens === undefined) throw new Error("cost_ceiling needs max_usd or max_tokens")
      const out: Extract<GraderSpec, { type: "cost_ceiling" }> = { type: ty }
      if (max_usd !== undefined) out.max_usd = max_usd
      if (max_tokens !== undefined) out.max_tokens = Math.trunc(max_tokens)
      return out
    }
    case "latency_ceiling": {
      known(v, ty, ["max_ms"])
      const max_ms = num(v, "max_ms", ty)
      if (max_ms === undefined) throw new Error("latency_ceiling needs max_ms")
      return { type: ty, max_ms }
    }
    case "status":
      known(v, ty, ["equals"])
      if (typeof v.equals !== "string" || v.equals.length === 0) throw new Error("status needs equals")
      return { type: ty, equals: v.equals }
    case "stopped_at":
      known(v, ty, ["step"])
      if (typeof v.step !== "string" || v.step.length === 0) throw new Error("stopped_at needs step")
      return { type: ty, step: v.step }
    case "missing_evidence": {
      known(v, ty, ["equals"])
      if (!Array.isArray(v.equals)) throw new Error("missing_evidence needs equals (a list of criteria)")
      const equals = v.equals.map((s) => {
        if (typeof s !== "string") throw new Error("missing_evidence.equals lists criteria ids")
        return s
      })
      return { type: ty, equals }
    }
    default:
      throw new Error(`unknown grader \`${ty}\` (known: ${GRADER_TYPES.join(", ")})`)
  }
}

function sameList(a: string[], b: string[]) {
  return a.length === b.length && a.every((x, i) => x === b[i])
}

function showVerifier(v: boolean | null | undefined) {
  return v === undefined || v === null ? "not reached" : v ? "pass" : "fail"
}

/** Grade one trajectory with one (already parsed) grader. */
export function grade(t: Trajectory, g: GraderSpec): Grade {
  const done = (pass: boolean, detail: string): Grade => ({ type: g.type, pass, detail })
  switch (g.type) {
    case "step_order": {
      let i = 0
      for (const s of t.steps) {
        if (!s.ok) continue
        if (i < g.steps.length && stepMatches(s, g.steps[i]!)) i++
      }
      const order = g.steps.join(" → ")
      return i >= g.steps.length
        ? done(true, `${order} in order`)
        : done(false, `${g.steps[i]} did not run (committed) in the required order ${order}`)
    }
    case "required_steps": {
      const missing = g.steps.filter((w) => !t.steps.some((s) => stepMatches(s, w)))
      return missing.length === 0 ? done(true, "all required steps ran") : done(false, `never ran: ${missing.join(", ")}`)
    }
    case "forbidden_step": {
      const hit = t.steps.filter((s) => g.steps.some((w) => stepMatches(s, w))).map((s) => s.node)
      return hit.length === 0 ? done(true, "no forbidden step ran") : done(false, `forbidden step(s) ran: ${hit.join(", ")}`)
    }
    case "effect_count": {
      const n = t.effects.filter((e) => e.committed && (g.tool === undefined || e.tool === g.tool)).length
      const ok = (g.min === undefined || n >= g.min) && (g.max === undefined || n <= g.max)
      return done(
        ok,
        `${n} committed effect(s)${g.tool !== undefined ? ` of ${g.tool}` : ""}; allowed ${g.min ?? "0"}..${g.max ?? "∞"}`,
      )
    }
    case "must_cite": {
      const cites = new Set<string>()
      for (const text of t.texts ?? []) {
        let from = 0
        for (;;) {
          const i = text.indexOf(g.marker, from)
          if (i < 0) break
          from = i + 1
          const c = text.slice(i + g.marker.length).split("]")[0]!
          if (c.trim() !== "" && c.length <= 256) cites.add(c.trim())
        }
      }
      return done(cites.size >= g.min, `${cites.size} distinct citation(s), ${g.min} required`)
    }
    case "verifier_pass": {
      const ok = t.verifier === true && t.status === "completed"
      return done(ok, `verifier ${showVerifier(t.verifier)}, status ${t.status}`)
    }
    case "cost_ceiling": {
      const ok = (g.max_usd === undefined || t.cost_usd <= g.max_usd) && (g.max_tokens === undefined || t.tokens <= g.max_tokens)
      return done(ok, `$${t.cost_usd.toFixed(4)} / ${t.tokens} tokens`)
    }
    case "latency_ceiling":
      return done(t.latency_ms <= g.max_ms, `${t.latency_ms.toFixed(0)} ms, ceiling ${g.max_ms.toFixed(0)} ms`)
    case "status":
      return done(t.status === g.equals, `status ${t.status} (want ${g.equals})`)
    case "stopped_at":
      return done(t.stopped_at === g.step, `stopped at ${JSON.stringify(t.stopped_at ?? null)} (want ${g.step})`)
    case "missing_evidence":
      return done(sameList(t.missing ?? [], g.equals), `missing ${JSON.stringify(t.missing ?? [])} (want ${JSON.stringify(g.equals)})`)
  }
}

/** Grade with every grader; the attempt passes when all pass. */
export function gradeAll(t: Trajectory, graders: GraderSpec[]): { pass: boolean; grades: Grade[] } {
  const grades = graders.map((g) => grade(t, g))
  return { pass: grades.every((x) => x.pass), grades }
}
