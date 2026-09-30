/**
 * ModelPool over HTTP (Agency Kernel WP7).
 *
 *   GET /model-pool[?capability=&role=&mode=]  → { schema_version, entries: ModelPoolEntryV1[] }
 *   GET /model-pool/capabilities               → { schema_version, capabilities: CapabilitySummary[] }
 *
 * The commrails router reads this (`fetch_model_pool`) and routes by
 * capability; it never sees provider or model names outside entry
 * `extensions` registry data.
 */
import { Hono } from "hono"
import { lazy } from "@/shared/util/lazy"
import {
  buildModelPool,
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

const ROLES = new Set(["S0", "S1", "S2", "S3"])
const CAP = /^cap(\.[a-z0-9_]+)+$/

export function createModelPoolRoutes(source: ModelSource = defaultSource) {
  return new Hono()
    .get("/", async (c) => {
      const capability = c.req.query("capability") || undefined
      const role = c.req.query("role") || undefined
      const mode = (c.req.query("mode") || undefined) as Mode | undefined
      if (capability && !CAP.test(capability)) return c.json({ error: "invalid capability id" }, 400)
      if (role && !ROLES.has(role)) return c.json({ error: "invalid role" }, 400)
      const entries = buildModelPool(await source(), { capability, role: role as Role | undefined, mode })
      return c.json({ schema_version: SCHEMA_VERSION, entries })
    })
    .get("/capabilities", async (c) => {
      const entries = buildModelPool(await source())
      return c.json({ schema_version: SCHEMA_VERSION, capabilities: summarizeCapabilities(entries) })
    })
}

export const ModelPoolRoutes = lazy(() => createModelPoolRoutes())
