import { describe, expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import { join } from "node:path"
import { createModelPoolRoutes } from "../../src/runtime/server/routes/model-pool"
import { buildModelPool, viewsFromProviders, type ProviderModelView } from "../../src/runtime/model-pool/pool"

// Registry data only: concrete ids live in the fixture, never in a plan.
const PROVIDERS = {
  allternit: {
    models: {
      "vendor-a/reasoner": {
        id: "vendor-a/reasoner", providerID: "allternit", api: { url: "https://api.example.test/v1" }, status: "active",
        capabilities: { reasoning: true, toolcall: true, input: { image: false }, output: { text: true } },
        limit: { context: 200000 }, cost: { input: 1.1, output: 4.4 },
      },
    },
  },
  "local-runner": {
    models: {
      "small-7b": {
        id: "small-7b", providerID: "local-runner", api: { url: "http://127.0.0.1:11434/v1" },
        capabilities: { reasoning: false, toolcall: false, input: { image: true }, output: { text: true } },
        limit: { context: 32000 }, cost: { input: 0, output: 0 },
      },
      "old": {
        id: "old", providerID: "local-runner", api: { url: "http://127.0.0.1:11434/v1" }, status: "deprecated",
        capabilities: { output: { text: true } }, limit: { context: 1 }, cost: { input: 0, output: 0 },
      },
    },
  },
}

const source = async (): Promise<ProviderModelView[]> => viewsFromProviders(PROVIDERS)
const app = createModelPoolRoutes(source)

const schema = JSON.parse(
  readFileSync(join(import.meta.dir, "../../../../spec/Contracts/kernel/v1/schemas/capability.schema.json"), "utf8"),
)
const entryDef = schema.$defs.ModelPoolEntryV1

describe("ModelPool HTTP", () => {
  test("GET / returns ABI ModelPoolEntryV1 entries from providers + the model catalog", async () => {
    const res = await app.request("/")
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.schema_version).toBe("1.0.0")
    expect(body.entries).toHaveLength(2) // deprecated one dropped
    for (const e of body.entries) {
      for (const k of Object.keys(e)) expect(Object.keys(entryDef.properties)).toContain(k)
      for (const k of entryDef.required) expect(e).toHaveProperty(k)
      expect(e.schema_id).toBe("allternit.kernel.ModelPoolEntryV1")
      expect(e.backend_id).toMatch(/^be\.[0-9a-f]{16}$/) // opaque: no vendor/model name
      for (const x of Object.keys(e.extensions)) expect(x).toMatch(/^x-[a-z0-9_.-]+$/)
    }
    const remote = body.entries.find((e: any) => e.residency === "REMOTE")
    const local = body.entries.find((e: any) => e.residency === "WARM")
    expect(remote.extensions["x-source"]).toBe("allternit.model_catalog")
    expect(remote.modes).toEqual(["M5.GENERATIVE", "M6.DEEP_SOLVER"])
    expect(remote.cognitive_roles).toEqual(["S2", "S3"])
    expect(local.extensions["x-source"]).toBe("gizzi.provider")
    expect(local.trust_tags).toContain("SECRET")
    expect(remote.trust_tags).not.toContain("SECRET")
  })

  test("filters by capability, role and mode", async () => {
    const byCap = await (await app.request("/?capability=cap.reason.deep")).json()
    expect(byCap.entries).toHaveLength(1)
    expect(byCap.entries[0].capabilities).toContain("cap.reason.deep")
    const s3 = await (await app.request("/?role=S3")).json()
    expect(s3.entries).toHaveLength(1)
    const vision = await (await app.request("/?capability=cap.vision.read&mode=M5.GENERATIVE")).json()
    expect(vision.entries[0].residency).toBe("WARM")
    expect((await app.request("/?capability=Bad Cap")).status).toBe(400)
    expect((await app.request("/?role=S9")).status).toBe(400)
  })

  test("GET /capabilities returns the capability summary", async () => {
    const res = await app.request("/capabilities")
    expect(res.status).toBe(200)
    const { capabilities } = await res.json()
    const ids = capabilities.map((c: any) => c.capability)
    expect(ids).toEqual(["cap.agent.tool_use", "cap.code.edit", "cap.reason.deep", "cap.text.generate", "cap.vision.read"])
    const edit = capabilities.find((c: any) => c.capability === "cap.code.edit")
    expect(edit.backends).toBe(2)
    expect(edit.residency).toEqual({ REMOTE: 1, WARM: 1 })
  })

  test("pool is deterministic and deduplicated", () => {
    const views = viewsFromProviders(PROVIDERS)
    const a = buildModelPool([...views, ...views])
    expect(a).toHaveLength(2)
    expect(buildModelPool(views)).toEqual(a)
  })
})
