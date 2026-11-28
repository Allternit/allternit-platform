import { afterEach, describe, expect, test } from "bun:test"
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import {
  cloudModelsFromCatalog,
  fetchPickerCatalog,
  overlaySnapshot,
  PICKER_CATALOG_TTL_MS,
  type PickerCatalog,
} from "../../src/runtime/providers/discovery/picker-catalog"
import { loadRuntimeSnapshot } from "../../src/runtime/providers/discovery/runtime-registry"

const fixture = (name: string): PickerCatalog =>
  JSON.parse(readFileSync(path.join(import.meta.dir, "picker-catalog-fixtures", `${name}.json`), "utf8"))

const tmpCache = () => path.join(mkdtempSync(path.join(os.tmpdir(), "picker-catalog-")), "picker-catalog.json")
const json = (body: unknown, etag = '"v1"') =>
  new Response(JSON.stringify(body), { status: 200, headers: { etag, "content-type": "application/json" } })

afterEach(() => {
  delete process.env.GIZZI_MODELS_REFRESH
})

describe("cloud models from each fixture", () => {
  const states = (name: string) => cloudModelsFromCatalog(fixture(name)).map((m) => [m.id, m.locked?.action ?? null, !!m.hidden])

  test("signed-out: every model listed, locked with sign_in", () => {
    expect(states("signed-out")).toEqual([
      ["claude-sonnet-5.5", "sign_in", false],
      ["gpt-6.1-sol", "sign_in", false],
      ["deepseek-v4.1-flash", "sign_in", false],
      ["glm-5.3-flash", "sign_in", false],
    ])
    expect(cloudModelsFromCatalog(fixture("signed-out"))[0]?.locked?.reason).toBe("Sign in")
  })

  test("funded: normal rows, nothing locked or hidden", () => {
    expect(states("funded").every(([, action, hidden]) => action === null && hidden === false)).toBe(true)
    expect(states("funded")).toHaveLength(4)
  })

  test("no-credits: locked with add_credits and the reason", () => {
    expect(states("no-credits").every(([, action]) => action === "add_credits")).toBe(true)
    expect(cloudModelsFromCatalog(fixture("no-credits"))[0]?.locked?.reason).toBe("No credits left")
  })

  test("hidden-model: the hidden row is flagged hidden and ranked last", () => {
    const models = cloudModelsFromCatalog(fixture("hidden-model"))
    expect(models.map((m) => [m.id, !!m.hidden])).toEqual([
      ["claude-sonnet-5.5", false],
      ["gpt-6.1-sol", false],
      ["deepseek-v4.1-flash", false],
      ["glm-5.3-flash", true],
    ])
    expect(models.map((m) => m.rank)).toEqual([0, 1, 2, 3])
  })
})

describe("fetchPickerCatalog", () => {
  test("fetch with token, store the ETag; a stale cache sends If-None-Match and 304 reuses the cache", async () => {
    const cacheFile = tmpCache()
    const seen: Array<Record<string, string>> = []
    const base = { cacheFile, origin: "https://cloud.test", token: "tok" }
    const first = await fetchPickerCatalog({
      ...base,
      now: 1000,
      fetch: (async (_u: unknown, init?: RequestInit) => {
        seen.push((init?.headers ?? {}) as Record<string, string>)
        return json(fixture("funded"))
      }) as unknown as typeof fetch,
    })
    expect(first.status === "ok" && first.from).toBe("live")
    expect(seen[0]?.Authorization).toBe("Bearer tok")

    // Fresh cache: no network at all.
    const fresh = await fetchPickerCatalog({
      ...base,
      now: 2000,
      fetch: (async () => {
        throw new Error("no network inside the TTL")
      }) as unknown as typeof fetch,
    })
    expect(fresh.status === "ok" && fresh.from).toBe("cache")

    // Past the TTL: conditional request, 304 -> the cached catalog.
    const revalidated = await fetchPickerCatalog({
      ...base,
      now: 1000 + PICKER_CATALOG_TTL_MS + 1,
      fetch: (async (_u: unknown, init?: RequestInit) => {
        seen.push((init?.headers ?? {}) as Record<string, string>)
        return new Response(null, { status: 304 })
      }) as unknown as typeof fetch,
    })
    expect(seen[1]?.["If-None-Match"]).toBe('"v1"')
    expect(revalidated.status === "ok" && revalidated.from).toBe("cache")
    expect(revalidated.status === "ok" && revalidated.catalog.cloudModels).toHaveLength(4)
  })

  test("the cache belongs to one identity: a signed-out cache is not served to a signed-in user", async () => {
    const cacheFile = tmpCache()
    const base = { cacheFile, origin: "https://cloud.test" }
    await fetchPickerCatalog({ ...base, token: null, now: 1000, fetch: (async () => json(fixture("signed-out"))) as unknown as typeof fetch })
    const signedIn = await fetchPickerCatalog({ ...base, token: "tok", now: 2000, fetch: (async () => json(fixture("funded"))) as unknown as typeof fetch })
    expect(signedIn.status === "ok" && signedIn.from).toBe("live")
    expect(signedIn.status === "ok" && signedIn.catalog.cloudModels[0]?.access.state).toBe("ok")
  })

  test("network error: stale cache when there is one, else unavailable (callers use the snapshot)", async () => {
    const cacheFile = tmpCache()
    const base = { cacheFile, origin: "https://cloud.test", token: "tok" }
    const down = (async () => {
      throw new Error("offline")
    }) as unknown as typeof fetch
    expect((await fetchPickerCatalog({ ...base, now: 1000, fetch: down })).status).toBe("unavailable")
    await fetchPickerCatalog({ ...base, now: 1000, fetch: (async () => json(fixture("funded"))) as unknown as typeof fetch })
    const stale = await fetchPickerCatalog({ ...base, now: 1000 + PICKER_CATALOG_TTL_MS + 1, fetch: down })
    expect(stale.status === "ok" && stale.from).toBe("stale-cache")
  })

  test("404 means an older server: unsupported, so discovery keeps /v1/models", async () => {
    const result = await fetchPickerCatalog({
      cacheFile: tmpCache(),
      origin: "https://cloud.test",
      token: null,
      fetch: (async () => new Response("nope", { status: 404 })) as unknown as typeof fetch,
    })
    expect(result.status).toBe("unsupported")
  })

  test("a malformed body is ignored", async () => {
    const cacheFile = tmpCache()
    writeFileSync(cacheFile, "not json")
    const result = await fetchPickerCatalog({
      cacheFile,
      origin: "https://cloud.test",
      token: null,
      fetch: (async () => json({ hello: "world" })) as unknown as typeof fetch,
    })
    expect(result.status).toBe("unavailable")
  })
})

describe("overlaySnapshot", () => {
  const snapshot = loadRuntimeSnapshot()

  test("no live catalog: the snapshot rows come back unchanged", () => {
    const out = overlaySnapshot(snapshot, undefined)
    expect(out.live).toBe(false)
    expect(out.runtimes.map((r) => r.id)).toEqual(snapshot.runtimes.map((r) => r.id))
    expect(out.tools.map((t) => t.id)).toEqual(snapshot.tools.map((t) => t.id))
  })

  test("live rows overlay by id; full rows keep their fields; unknown ids are added reduced", () => {
    const live = fixture("funded")
    const out = overlaySnapshot(snapshot, live)
    expect(out.live).toBe(true)
    const claudeTool = out.tools.find((t) => t.id === "claude-cli" || (t.aliases ?? []).includes("claude"))!
    const models = claudeTool.models as Array<{ id: string; default?: boolean }>
    expect(models.map((m) => m.id)).toEqual(live.runtimes.find((r) => r.id === "claude")!.models.map((m) => m.id))
    expect(models.find((m) => m.default)?.id).toBe("claude-sonnet-5-5")
    // The tool row keeps its CLI fields.
    expect(claudeTool.bin).toBe("claude")
    expect(claudeTool.probe).toBeDefined()
    // Every snapshot row is still there (nothing replaced wholesale).
    expect(out.runtimes).toHaveLength(snapshot.runtimes.length)
    expect(out.tools).toHaveLength(snapshot.tools.length)
    // Unknown live ids land in extraRuntimes with catalog fields only.
    const known = new Set([...snapshot.runtimes, ...snapshot.tools].flatMap((r) => [r.id, ...((r as { aliases?: string[] }).aliases ?? [])]))
    expect(out.extraRuntimes.map((r) => r.id)).toEqual(live.runtimes.map((r) => r.id).filter((id) => !known.has(id)))
  })
})
