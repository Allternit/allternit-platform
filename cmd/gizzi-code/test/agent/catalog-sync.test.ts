// @ts-nocheck
// CatalogSync must stop retrying tokens the server rejected (401/403) for the
// life of the process. fetch is stubbed; no network.
import { afterAll, beforeAll, expect, spyOn, test } from "bun:test"
import * as fs from "fs/promises"
import os from "os"
import path from "path"
import { Log } from "../../src/shared/util/log"

Log.init({ print: false })

let home: string
const saved: Record<string, string | undefined> = {}
const realFetch = globalThis.fetch
const KEYS = ["GIZZI_TEST_HOME", "XDG_DATA_HOME", "XDG_CONFIG_HOME", "ALLTERNIT_API_KEY", "ALLTERNIT_API_TOKEN"]

beforeAll(async () => {
  home = await fs.mkdtemp(path.join(os.tmpdir(), "gizzi-catsync-test-"))
  for (const k of KEYS) saved[k] = process.env[k]
  // apply() writes/removes ~/.gizzi/agents/allternit: keep it off the real home.
  process.env.GIZZI_TEST_HOME = home
  process.env.XDG_DATA_HOME = path.join(home, "share")
  process.env.XDG_CONFIG_HOME = path.join(home, "config")
  // Global.Path is frozen at first import; ignore any real `gizzi pair` here.
  const { Pairing } = await import("../../src/runtime/services/pairing/pairing")
  spyOn(Pairing, "load").mockResolvedValue(undefined as any)
})

afterAll(async () => {
  globalThis.fetch = realFetch
  for (const k of KEYS) {
    if (saved[k] === undefined) delete process.env[k]
    else process.env[k] = saved[k]
  }
  await fs.rm(home, { recursive: true, force: true })
})

test("no fetch once every token has been rejected", async () => {
  process.env.ALLTERNIT_API_KEY = "key-revoked"
  process.env.ALLTERNIT_API_TOKEN = "allternit_runtime_revoked"
  let calls = 0
  globalThis.fetch = (async () => {
    calls++
    return new Response("unauthorized", { status: 401 })
  }) as typeof fetch
  const { CatalogSync } = await import("../../src/runtime/agents/catalog-sync")
  CatalogSync.resetRejected()

  await CatalogSync.syncOnce()
  expect(calls).toBe(2) // each token tried once

  await CatalogSync.syncOnce()
  await CatalogSync.syncOnce()
  expect(calls).toBe(2) // later ticks make no network call
})

test("a 403 token is skipped but a valid token is still tried", async () => {
  process.env.ALLTERNIT_API_KEY = "key-forbidden"
  process.env.ALLTERNIT_API_TOKEN = "token-ok"
  const seen: string[] = []
  globalThis.fetch = (async (_url: string, init: any) => {
    const auth = init.headers.Authorization as string
    seen.push(auth)
    if (auth === "Bearer key-forbidden") return new Response("no", { status: 403 })
    return Response.json({ types: [] })
  }) as typeof fetch
  const { CatalogSync } = await import("../../src/runtime/agents/catalog-sync")
  CatalogSync.resetRejected()

  await CatalogSync.syncOnce()
  await CatalogSync.syncOnce()
  expect(seen).toEqual(["Bearer key-forbidden", "Bearer token-ok", "Bearer token-ok"])
})
