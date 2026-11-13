import { Global } from "@/runtime/context/global"
import { Log } from "@/shared/util/log"
import path from "path"
import z from "zod/v4"
import { Installation } from "@/shared/installation"
import { SelfhostInference } from "@/runtime/providers/selfhost-inference"
import { Flag } from "@/runtime/context/flag/flag"
import { lazy } from "@/shared/util/lazy"
import { Filesystem } from "@/shared/util/filesystem"

// Try to import bundled snapshot (generated at build time)
// Falls back to undefined in dev mode when snapshot doesn't exist
export namespace ModelsDev {
  const log = Log.create({ service: "models.dev" })
  const filepath = path.join(Global.Path.cache, "models.json")

  export const Model = z.object({
    id: z.string(),
    name: z.string(),
    family: z.string().optional(),
    release_date: z.string(),
    attachment: z.boolean(),
    reasoning: z.boolean(),
    temperature: z.boolean().optional().default(false),
    tool_call: z.boolean(),
    interleaved: z
      .union([
        z.literal(true),
        z
          .object({
            field: z.enum(["reasoning_content", "reasoning_details"]),
          })
          .strict(),
      ])
      .optional(),
    cost: z
      .object({
        input: z.number(),
        output: z.number(),
        cache_read: z.number().optional(),
        cache_write: z.number().optional(),
        context_over_200k: z
          .object({
            input: z.number(),
            output: z.number(),
            cache_read: z.number().optional(),
            cache_write: z.number().optional(),
          })
          .optional(),
      })
      .optional(),
    limit: z.object({
      context: z.number(),
      input: z.number().optional(),
      output: z.number(),
    }),
    modalities: z
      .object({
        input: z.array(z.enum(["text", "audio", "image", "video", "pdf"])),
        output: z.array(z.enum(["text", "audio", "image", "video", "pdf"])),
      })
      .optional(),
    experimental: z.union([z.boolean(), z.record(z.string(), z.any()), z.unknown()]).optional(),
    status: z.enum(["alpha", "beta", "deprecated"]).optional(),
    options: z.record(z.string(), z.any()).optional().default({}),
    headers: z.record(z.string(), z.string()).optional(),
    provider: z.object({ npm: z.string().optional(), api: z.string().optional() }).optional(),
    variants: z.record(z.string(), z.record(z.string(), z.any())).optional(),
  })
  export type Model = z.infer<typeof Model>

  export const Provider = z.object({
    api: z.string().optional(),
    name: z.string(),
    env: z.array(z.string()),
    id: z.string(),
    npm: z.string().optional(),
    models: z.record(z.string(), Model),
  })

  export type Provider = z.infer<typeof Provider>

  /** Validate entries independently so one malformed provider cannot poison
   * model selection for every other provider. */
  export function parse(input: unknown, source = "unknown"): Record<string, Provider> {
    if (!input || typeof input !== "object" || Array.isArray(input)) {
      log.error("invalid models catalog payload", { source, reason: "expected an object" })
      return {}
    }

    const catalog: Record<string, Provider> = {}
    for (const [providerID, value] of Object.entries(input)) {
      const result = Provider.safeParse(value)
      if (!result.success) {
        log.warn("skipping invalid models catalog provider", {
          source,
          providerID,
          issues: result.error.issues.slice(0, 3).map((issue) => ({ path: issue.path, message: issue.message })),
        })
        continue
      }
      catalog[providerID] = result.data
    }
    return catalog
  }

  function populated(catalog: Record<string, Provider>) {
    return Object.keys(catalog).length > 0
  }

  function url() {
    return Flag.GIZZI_MODELS_URL || "https://models.dev"
  }

  export const Data = lazy(async () => {
    const cachePath = Flag.GIZZI_MODELS_PATH ?? filepath
    const cached = await Filesystem.readJson(cachePath).catch(() => undefined)
    const cachedCatalog = cached === undefined ? {} : parse(cached, cachePath)
    if (populated(cachedCatalog)) return cachedCatalog

    // @ts-ignore — models-snapshot is generated at build time, may not exist in dev
    const snapshot = await import("./models-snapshot")
      .then((m) => m.snapshot as Record<string, unknown>)
      .catch(() => undefined)
    const snapshotCatalog = snapshot === undefined ? {} : parse(snapshot, "bundled snapshot")
    if (populated(snapshotCatalog)) return snapshotCatalog
    if (Flag.GIZZI_DISABLE_MODELS_FETCH) return {}

    try {
      const response = await fetch(`${url()}/api.json`, {
        headers: { "User-Agent": Installation.USER_AGENT },
        signal: AbortSignal.timeout(10 * 1000),
      })
      if (!response.ok) throw new Error(`HTTP ${response.status}`)
      const catalog = parse(await response.json(), `${url()}/api.json`)
      if (!populated(catalog)) throw new Error("catalog contains no valid providers")
      return catalog
    } catch (error) {
      log.error("failed to load models catalog", { error })
      return {}
    }
  })

  export async function get() {
    // Self-hosted inference: no remote catalog, the server's model_map is the model list.
    if (SelfhostInference.enabled()) return {} as Record<string, Provider>
    const result = await Data()
    return result as Record<string, Provider>
  }

  export async function refresh() {
    if (SelfhostInference.enabled()) return
    try {
      const result = await fetch(`${url()}/api.json`, {
        headers: {
          "User-Agent": Installation.USER_AGENT,
        },
        signal: AbortSignal.timeout(10 * 1000),
      })
      if (!result.ok) throw new Error(`HTTP ${result.status}`)
      const catalog = parse(await result.json(), `${url()}/api.json`)
      if (!populated(catalog)) throw new Error("catalog contains no valid providers")
      await Filesystem.write(filepath, JSON.stringify(catalog))
      ModelsDev.Data.reset()
    } catch (error) {
      log.error("failed to refresh models catalog", { error })
    }
  }
}

if (!Flag.GIZZI_DISABLE_MODELS_FETCH && !SelfhostInference.enabled() && !process.argv.includes("--get-yargs-completions")) {
  ModelsDev.refresh()
  setInterval(
    async () => {
      await ModelsDev.refresh()
    },
    60 * 1000 * 60,
  ).unref()
}
