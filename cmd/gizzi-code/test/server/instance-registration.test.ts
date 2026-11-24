// @ts-nocheck
// Smoke test for instance-registration env-token precedence (BYO-VPS flow):
// the bootstrap injects the box's runtime device token into the systemd env
// file as ALLTERNIT_API_TOKEN. A device-prefixed token (allternit_runtime_…)
// is durable, so registration must refresh on an interval; any other env
// token stays one-shot. The "registry" is a local Bun.serve recorder.
//
// Flag.GIZZI_PLATFORM_API_URL and the refresh interval are read at module
// load, so the fake registry and the env are set up once for the whole file.
import { afterAll, beforeAll, describe, expect, spyOn, test } from "bun:test"
import * as fs from "fs/promises"
import os from "os"
import path from "path"
import { Log } from "../../src/shared/util/log"

Log.init({ print: false })

type RecordedPut = { authorization: string | null; body: any }

let home: string
let server: ReturnType<typeof Bun.serve>
let recorded: RecordedPut[] = []
let putStatus = 200
let previousPlatformUrl: string | undefined
let previousXdg: { XDG_DATA_HOME?: string; XDG_CONFIG_HOME?: string } = {}

function wait(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

// Poll instead of sleeping a fixed duration: under full-suite CPU load the
// 40ms refresh interval can slip past a fixed 160ms sleep before two PUTs
// land, which made this test fail only when run alongside the whole suite.
async function waitForCount(n: number, deadlineMs = 5000): Promise<number> {
  const start = Date.now()
  while (recorded.length < n && Date.now() - start < deadlineMs) {
    await wait(20)
  }
  return recorded.length
}

beforeAll(async () => {
  // Isolate xdg state so Pairing.load()/Auth.all() find nothing and the env
  // token is the only credential in play.
  home = await fs.mkdtemp(path.join(os.tmpdir(), "gizzi-reg-test-"))
  previousXdg = {
    XDG_DATA_HOME: process.env.XDG_DATA_HOME,
    XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME,
  }
  process.env.XDG_DATA_HOME = path.join(home, "share")
  process.env.XDG_CONFIG_HOME = path.join(home, "config")
  process.env.GIZZI_REGISTER_INTERVAL_MS = "40"

  server = Bun.serve({
    port: 0,
    async fetch(req) {
      const url = new URL(req.url)
      if (req.method === "PUT" && url.pathname === "/api/v1/gizzi-instances/self") {
        recorded.push({ authorization: req.headers.get("authorization"), body: await req.json() })
        if (putStatus !== 200) return new Response("nope", { status: putStatus })
        return Response.json({ id: "gi_test", name: "byo-vps-1", status: "online" })
      }
      return new Response("not found", { status: 404 })
    },
  })
  process.env.GIZZI_PLATFORM_API_URL = `http://127.0.0.1:${server.port}`

  // Flag consts are frozen at first module import; in the full smoke suite
  // other files import Flag before this beforeAll runs, which would send the
  // PUTs to the default platform URL instead of the recorder. The Flag
  // namespace is runtime-mutable — point it at the recorder and restore in
  // afterAll.
  // Global.Path is frozen at first import too, so a real `gizzi pair` on the
  // dev machine would otherwise win over the env token. Pretend not paired.
  const { Pairing } = await import("../../src/runtime/services/pairing/pairing")
  spyOn(Pairing, "load").mockResolvedValue(undefined as any)
  spyOn(Pairing, "rotateIfNeeded").mockResolvedValue(undefined as any)
  const { Flag } = await import("../../src/runtime/context/flag/flag")
  previousPlatformUrl = Flag.GIZZI_PLATFORM_API_URL
  Flag.GIZZI_PLATFORM_API_URL = process.env.GIZZI_PLATFORM_API_URL
})

afterAll(async () => {
  server.stop(true)
  delete process.env.ALLTERNIT_API_TOKEN
  delete process.env.GIZZI_PLATFORM_API_URL
  const { Flag } = await import("../../src/runtime/context/flag/flag")
  Flag.GIZZI_PLATFORM_API_URL = previousPlatformUrl
  if (previousXdg.XDG_DATA_HOME === undefined) delete process.env.XDG_DATA_HOME
  else process.env.XDG_DATA_HOME = previousXdg.XDG_DATA_HOME
  if (previousXdg.XDG_CONFIG_HOME === undefined) delete process.env.XDG_CONFIG_HOME
  else process.env.XDG_CONFIG_HOME = previousXdg.XDG_CONFIG_HOME
  await fs.rm(home, { recursive: true, force: true })
})

async function registerFor(token: string, ms: number) {
  process.env.ALLTERNIT_API_TOKEN = token
  const { InstanceRegistration } = await import("../../src/runtime/server/instance-registration")
  await InstanceRegistration.register({ url: "http://100.64.0.7:4096", name: "byo-vps-1" })
  await wait(ms)
  InstanceRegistration.stop()
}

describe("InstanceRegistration env token precedence", () => {
  test("device-prefixed ALLTERNIT_API_TOKEN gets the durable refresh loop", async () => {
    recorded = []
    process.env.ALLTERNIT_API_TOKEN = "allternit_runtime_smoketest"
    const { InstanceRegistration } = await import("../../src/runtime/server/instance-registration")
    await InstanceRegistration.register({ url: "http://100.64.0.7:4096", name: "byo-vps-1" })
    const count = await waitForCount(2)
    InstanceRegistration.stop()

    expect(count).toBeGreaterThanOrEqual(2)
    for (const put of recorded) {
      expect(put.authorization).toBe("Bearer allternit_runtime_smoketest")
      expect(put.body.url).toBe("http://100.64.0.7:4096")
    }
  })

  test("non-device ALLTERNIT_API_TOKEN stays one-shot", async () => {
    recorded = []
    process.env.ALLTERNIT_API_TOKEN = "clerk-like-opaque-token"
    const { InstanceRegistration } = await import("../../src/runtime/server/instance-registration")
    await InstanceRegistration.register({ url: "http://100.64.0.7:4096", name: "byo-vps-1" })
    await waitForCount(1)
    InstanceRegistration.stop()
    // Give the would-be refresh interval (40ms) several chances to fire; a
    // one-shot token must not schedule any refresh.
    await wait(200)

    expect(recorded.length).toBe(1)
    expect(recorded[0].authorization).toBe("Bearer clerk-like-opaque-token")
  })
})

describe("InstanceRegistration rejected credential", () => {
  test("a 401 stops the refresh loop (no second PUT)", async () => {
    recorded = []
    putStatus = 401
    process.env.ALLTERNIT_API_TOKEN = "allternit_runtime_revoked"
    const { InstanceRegistration } = await import("../../src/runtime/server/instance-registration")
    await InstanceRegistration.register({ url: "http://100.64.0.7:4096", name: "byo-vps-1" })
    await wait(300) // several 40ms intervals
    InstanceRegistration.stop()
    putStatus = 200
    expect(recorded.length).toBe(1)
  })

  test("a 401 on a refresh tick stops the loop after that PUT", async () => {
    recorded = []
    putStatus = 200
    process.env.ALLTERNIT_API_TOKEN = "allternit_runtime_revoked_later"
    const { InstanceRegistration } = await import("../../src/runtime/server/instance-registration")
    await InstanceRegistration.register({ url: "http://100.64.0.7:4096", name: "byo-vps-1" })
    putStatus = 401
    await waitForCount(2)
    await wait(300)
    InstanceRegistration.stop()
    putStatus = 200
    expect(recorded.length).toBe(2)
  })

  test("a 500 keeps the loop going", async () => {
    recorded = []
    putStatus = 500
    process.env.ALLTERNIT_API_TOKEN = "allternit_runtime_flaky"
    const { InstanceRegistration } = await import("../../src/runtime/server/instance-registration")
    await InstanceRegistration.register({ url: "http://100.64.0.7:4096", name: "byo-vps-1" })
    const count = await waitForCount(3)
    InstanceRegistration.stop()
    putStatus = 200
    expect(count).toBeGreaterThanOrEqual(3)
  })

  test("register() works again after a rejection", async () => {
    recorded = []
    putStatus = 401
    process.env.ALLTERNIT_API_TOKEN = "allternit_runtime_repaired"
    const { InstanceRegistration } = await import("../../src/runtime/server/instance-registration")
    await InstanceRegistration.register({ url: "http://100.64.0.7:4096", name: "byo-vps-1" })
    putStatus = 200
    await InstanceRegistration.register({ url: "http://100.64.0.7:4096", name: "byo-vps-1" })
    const count = await waitForCount(3)
    InstanceRegistration.stop()
    expect(count).toBeGreaterThanOrEqual(3)
  })
})
