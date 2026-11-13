/**
 * What a bridged tool returned beyond its text, held for the session
 * processor. A CLI brain (Claude Code, Codex, ACP agents…) reports a bridged
 * call to gizzi as its own tool_call/tool_result events: name and text only.
 * run_code's structured result (files[], runDir, exit_code…) is what
 * allternit-api ingests (gizzi_chat_stream.rs keys on `part.tool == "run_code"`
 * and `state.metadata.files`), so the bridge stashes it here when the call
 * returns and the processor merges it into the observed tool part.
 *
 * Kept free of runtime imports: processor.ts reads it on every observed tool event.
 */
export namespace CliBridgeResults {
  /** The bridged tools whose result metadata the processor merges. */
  const CANONICAL = ["run_code"] as const
  export type Canonical = (typeof CANONICAL)[number]

  /**
   * How CLIs name a bridged tool from the "allternit" MCP server:
   * `mcp__allternit__run_code` (Claude Code, Codex app-server via
   * codexToolCall), `allternit__run_code` (Gemini's qualified name),
   * `allternit_run_code` (OpenCode), `allternit.run_code` / `allternit/run_code`
   * / `allternit:run_code` (other ACP titles), `run_code (allternit MCP Server)`
   * (Gemini's ACP title), or bare `run_code`.
   */
  function shape(tool: string) {
    return new RegExp(`^(?:mcp__)?(?:allternit(?:__|[_.:/]))?${tool}(?:\\s*\\(allternit MCP Server\\))?$`)
  }
  const SHAPES = CANONICAL.map((tool) => [tool, shape(tool)] as const)

  /**
   * gizzi's own id for a bridged tool the CLI reported under its vendor name.
   * Covers run_code only: every other name comes back unchanged (the web keys
   * on `mcp__allternit__artifact_*` as the CLI reports them).
   */
  export function canonicalTool(name: string): string {
    const trimmed = name.trim()
    for (const [tool, re] of SHAPES) if (re.test(trimmed)) return tool
    return name
  }

  export function isCanonical(name: string): name is Canonical {
    return (CANONICAL as readonly string[]).includes(name)
  }

  export interface Stashed {
    title?: string
    metadata: Record<string, unknown>
  }

  const TTL_MS = 10 * 60 * 1000
  /** Per (session, tool): a CLI that never reports the result must not grow this without bound. */
  const MAX_PER_KEY = 20

  type Entry = Stashed & { args: string; at: number }
  const stash = new Map<string, Entry[]>()

  const slot = (sessionID: string, tool: string) => `${sessionID}\u0000${tool}`

  /** JSON with object keys sorted at every level, so re-serialized args still match. */
  export function stableJson(value: unknown): string {
    const sort = (v: unknown): unknown => {
      if (Array.isArray(v)) return v.map(sort)
      if (v && typeof v === "object") {
        return Object.fromEntries(
          Object.keys(v as Record<string, unknown>)
            .sort()
            .filter((k) => (v as Record<string, unknown>)[k] !== undefined)
            .map((k) => [k, sort((v as Record<string, unknown>)[k])]),
        )
      }
      return v
    }
    return JSON.stringify(sort(value ?? {})) ?? "null"
  }

  function live(list: Entry[] | undefined, now: number) {
    return (list ?? []).filter((e) => now - e.at < TTL_MS)
  }

  /** Called by the bridge when a bridged call returns. */
  export function put(sessionID: string, tool: string, args: unknown, result: Stashed) {
    const now = Date.now()
    const key = slot(sessionID, tool)
    const list = live(stash.get(key), now)
    list.push({ ...result, args: stableJson(args), at: now })
    while (list.length > MAX_PER_KEY) list.shift()
    stash.set(key, list)
  }

  /**
   * The stashed result for this call, removed. Matched on the args; when the
   * CLI re-serialized them (dropped defaults, coerced numbers…) the oldest
   * unclaimed result for the same session and tool is taken instead: a CLI
   * reports its calls in the order it made them.
   */
  export function take(sessionID: string, tool: string, args: unknown): Stashed | undefined {
    const key = slot(sessionID, tool)
    const list = live(stash.get(key), Date.now())
    if (!list.length) {
      stash.delete(key)
      return undefined
    }
    const wanted = stableJson(args)
    const exact = list.findIndex((e) => e.args === wanted)
    const [entry] = list.splice(exact >= 0 ? exact : 0, 1)
    if (list.length) stash.set(key, list)
    else stash.delete(key)
    return entry ? { title: entry.title, metadata: entry.metadata } : undefined
  }
}
