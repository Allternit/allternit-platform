/**
 * ModelPool over HTTP (Agency Kernel WP7).
 *
 *   GET /model-pool[?capability=&role=&mode=]  → { schema_version, entries: ModelPoolEntryV1[] }
 *   GET /model-pool/capabilities               → { schema_version, capabilities: CapabilitySummary[] }
 *
 * The Factory engine router reads this (`fetch_model_pool`) and routes by
 * capability; it never sees provider or model names outside entry
 * `extensions` registry data.
 */
import { Hono } from "hono"
import { lazy } from "@/shared/util/lazy"
import {
  buildModelPool,
  decisionRuntimeEntry,
  type DecisionManifestView,
  type ModelPoolEntryV1,
  summarizeCapabilities,
  viewsFromProviders,
  SCHEMA_VERSION,
  type Mode,
  type ProviderModelView,
  type Role,
} from "@/runtime/model-pool/pool"

export type ModelSource = () => Promise<ProviderModelView[]>

const defaultSource: ModelSource = async () => {
  const { Provider } = await import("@/runtime/providers/provider")
  return viewsFromProviders((await Provider.list()) as any)
}

export type ExtraSource = () => Promise<ModelPoolEntryV1[]>

/**
 * The S1 decision runtime entry. Manifests come from ALLTERNIT_S1_MANIFESTS (a
 * JSON array of DecisionCalibrationManifestV1) — none today, matching the
 * runtime's own empty default, so it is listed uncalibrated + shadow.
 */
const defaultExtra: ExtraSource = async () => {
  let manifests: DecisionManifestView[] = []
  const path = process.env.ALLTERNIT_S1_MANIFESTS?.trim()
  if (path) {
    try {
      const parsed = JSON.parse(await Bun.file(path).text())
      if (Array.isArray(parsed)) manifests = parsed
    } catch {
      manifests = [] // unreadable manifests = uncalibrated (fail closed)
    }
  }
  const port = process.env.SYSTEM_ONE_PORT?.trim() || "7717"
  const s1Mode = process.env.ALLTERNIT_S1_MODE === "live" ? "live" : "shadow"
  return [decisionRuntimeEntry({ manifests, s1Mode, endpoint: `http://127.0.0.1:${port}/v1/decision` })]
}

const ROLES = new Set(["S0", "S1", "S2", "S3"])
const CAP = /^cap(\.[a-z0-9_]+)+$/

export function createModelPoolRoutes(source: ModelSource = defaultSource, extra: ExtraSource = defaultExtra) {
  return new Hono()
    .get("/", async (c) => {
      const capability = c.req.query("capability") || undefined
      const role = c.req.query("role") || undefined
      const mode = (c.req.query("mode") || undefined) as Mode | undefined
      if (capability && !CAP.test(capability)) return c.json({ error: "invalid capability id" }, 400)
      if (role && !ROLES.has(role)) return c.json({ error: "invalid role" }, 400)
      const entries = buildModelPool(await source(), { capability, role: role as Role | undefined, mode }, await extra())
      return c.json({ schema_version: SCHEMA_VERSION, entries })
    })
    .get("/capabilities", async (c) => {
      const entries = buildModelPool(await source(), {}, await extra())
      return c.json({ schema_version: SCHEMA_VERSION, capabilities: summarizeCapabilities(entries) })
    })
}

export const ModelPoolRoutes = lazy(() => createModelPoolRoutes())
