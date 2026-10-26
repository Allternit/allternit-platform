/**
 * User subagent catalog sync with the Allternit API.
 *
 * `GET /api/v1/subagent-types` returns only the user's rows: custom types and
 * overrides of built-ins. At serve start and every 10 minutes while signed in:
 *   - custom rows → `~/.gizzi/agents/allternit/<slug>.md` (loaded by the
 *     normal markdown agent loader); files for deleted rows are removed;
 *   - override rows (slug of a built-in, not custom) → title / face / model
 *     class / description applied in memory to the built-in (Agent.get/list).
 *
 * Auth is the same as the Artifacts store client: ALLTERNIT_API_KEY, then
 * ALLTERNIT_API_TOKEN, then the `gizzi login` / `gizzi pair` device token,
 * against ALLTERNIT_CLOUD_API_URL / GIZZI_PLATFORM_API_URL / api.allternit.com.
 * Signed out, offline, or any error → no-op (existing files are kept). Never throws.
 */
import fs from "fs/promises"
import path from "path"
import { CLOUD_URLS } from "@/shared/constants/cloudUrls"
import { Global } from "@/runtime/context/global"
import { Log } from "@/shared/util/log"
import { catalogAgent, isModelClass, permissionFromTools, type ModelClass } from "./builtin-catalog"

const log = Log.create({ service: "agents.catalog-sync" })

export namespace CatalogSync {
  export const INTERVAL_MS = 10 * 60 * 1000
  const TIMEOUT_MS = 10_000
  const SLUG = /^[a-z0-9][a-z0-9_-]{0,63}$/

  export interface Row {
    slug: string
    name?: string
    description?: string
    prompt?: string
    model_class?: string
    tools?: string[]
    max_steps?: number
    look?: { pack?: string; face?: string; accent?: string }
    is_custom?: boolean
  }

  export interface Override {
    title?: string
    face?: string
    modelClass?: ModelClass
    description?: string
  }

  export interface Custom {
    slug: string
    title?: string
    description: string
    prompt: string
    modelClass: ModelClass
    steps?: number
    face?: string
    tools?: string[]
  }

  const overrides = new Map<string, Override>()
  let customs: Custom[] = []
  let timer: ReturnType<typeof setInterval> | undefined
  let running: Promise<void> | undefined

  export function dir() {
    return path.join(Global.Path.home, ".gizzi", "agents", "allternit")
  }

  /** In-memory override for a built-in, if the user has one. */
  export function override(name: string): Override | undefined {
    return overrides.get(name)
  }

  /** The last synced custom types (for cross-brain exports). */
  export function customTypes(): Custom[] {
    return customs
  }

  function base(): string {
    const explicit =
      process.env.ALLTERNIT_CLOUD_API_URL?.trim() || process.env.GIZZI_PLATFORM_API_URL?.trim() || CLOUD_URLS.api
    return explicit.replace(/\/+$/, "")
  }

  async function tokens(): Promise<string[]> {
    const env = [process.env.ALLTERNIT_API_KEY?.trim(), process.env.ALLTERNIT_API_TOKEN?.trim()]
    const { Pairing } = await import("@/runtime/services/pairing/pairing")
    const stored = await Pairing.load().catch(() => undefined)
    const device = Pairing.tokenUsable(stored) ? stored.deviceToken : undefined
    return [...new Set([...env, device].filter((t): t is string => !!t))]
  }

  /** undefined = signed out / offline / rejected: leave everything as is. */
  async function fetchRows(): Promise<Row[] | undefined> {
    const list = await tokens()
    for (const token of list) {
      let response: Response
      try {
        response = await fetch(`${base()}/api/v1/subagent-types`, {
          headers: { Authorization: `Bearer ${token}`, Accept: "application/json" },
          signal: AbortSignal.timeout(TIMEOUT_MS),
        })
      } catch {
        return undefined
      }
      if (response.status === 401 || response.status === 403) continue
      if (!response.ok) return undefined
      const json = (await response.json().catch(() => undefined)) as { types?: unknown } | undefined
      if (!json || !Array.isArray(json.types)) return undefined
      return json.types.filter((r): r is Row => !!r && typeof (r as Row).slug === "string")
    }
    return undefined
  }

  function yamlValue(value: string | number): string {
    return typeof value === "number" ? String(value) : JSON.stringify(value)
  }

  export function markdown(c: Custom): string {
    const front: [string, string | number | undefined][] = [
      ["name", c.slug],
      ["description", c.description],
      ["mode", "subagent"],
      ["model", c.modelClass],
      ["steps", c.steps],
      ["face", c.face],
      ["title", c.title],
    ]
    const lines = front.filter(([, v]) => v !== undefined && v !== "").map(([k, v]) => `${k}: ${yamlValue(v!)}`)
    // JSON is valid YAML flow syntax, so the ruleset survives the frontmatter parser as-is.
    lines.push(`permission: ${JSON.stringify(permissionFromTools(c.tools))}`)
    return `---\n${lines.join("\n")}\n---\n\n${c.prompt.trim()}\n`
  }

  function toClass(value: string | undefined): ModelClass | undefined {
    return isModelClass(value) ? (value!.trim().toLowerCase() as ModelClass) : undefined
  }

  /** Apply one fetched list. Exported for tests. */
  export async function apply(rows: Row[]) {
    const nextOverrides = new Map<string, Override>()
    const nextCustoms: Custom[] = []
    for (const row of rows) {
      const slug = row.slug.trim().toLowerCase()
      if (!SLUG.test(slug)) continue
      if (!row.is_custom && catalogAgent(slug)) {
        nextOverrides.set(slug, {
          title: row.name || undefined,
          face: row.look?.face || undefined,
          modelClass: toClass(row.model_class),
          description: row.description || undefined,
        })
        continue
      }
      if (!row.prompt?.trim()) continue
      nextCustoms.push({
        slug,
        title: row.name || undefined,
        description: row.description || row.name || slug,
        prompt: row.prompt,
        modelClass: toClass(row.model_class) ?? "small",
        steps: typeof row.max_steps === "number" && row.max_steps > 0 ? Math.floor(row.max_steps) : undefined,
        face: row.look?.face || undefined,
        tools: Array.isArray(row.tools) ? row.tools.filter((t) => typeof t === "string") : undefined,
      })
    }

    const target = dir()
    await fs.mkdir(target, { recursive: true })
    const keep = new Set<string>()
    for (const c of nextCustoms) {
      const file = path.join(target, `${c.slug}.md`)
      keep.add(file)
      const body = markdown(c)
      const current = await fs.readFile(file, "utf8").catch(() => undefined)
      if (current !== body) await fs.writeFile(file, body, "utf8")
    }
    for (const entry of await fs.readdir(target).catch(() => [] as string[])) {
      const file = path.join(target, entry)
      if (entry.endsWith(".md") && !keep.has(file)) await fs.rm(file, { force: true })
    }

    overrides.clear()
    for (const [k, v] of nextOverrides) overrides.set(k, v)
    customs = nextCustoms
  }

  export async function syncOnce(): Promise<void> {
    if (running) return running
    running = (async () => {
      try {
        const rows = await fetchRows()
        if (!rows) return
        await apply(rows)
        log.info("subagent catalog synced", { overrides: overrides.size, customs: customs.length })
      } catch (error) {
        log.warn("subagent catalog sync failed", { error: String(error) })
      } finally {
        running = undefined
      }
    })()
    return running
  }

  /** Start at serve start; idempotent. */
  export function start() {
    if (timer) return
    if (["1", "true"].includes((process.env.GIZZI_SUBAGENT_SYNC_OFF ?? "").toLowerCase())) return
    void syncOnce()
    timer = setInterval(() => void syncOnce(), INTERVAL_MS)
    ;(timer as { unref?: () => void }).unref?.()
  }

  export function stop() {
    if (timer) clearInterval(timer)
    timer = undefined
  }
}
