import { describe, expect, test } from "bun:test"
import path from "path"
import { SessionStartRoutes, MODE_TO_ENGINE, appModeOf, originKey } from "../../src/runtime/server/routes/session-start"
import { PermissionNext } from "../../src/runtime/tools/guard/permission/next"
import { Storage } from "../../src/runtime/session/storage/storage"
import { tmpdir } from "../fixture/fixture"

const app = SessionStartRoutes()
const origin = { surface: "ios", deviceName: "Test phone", depth: 0 }

const post = (body: unknown) =>
  app.request("/", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) })

describe("POST /session-start (engine kind)", () => {
  test("creates a session in the request folder, with its mode and origin", async () => {
    await using tmp = await tmpdir({ git: true })
    const res = await post({ folder: { path: tmp.path }, runtime: "allternit", mode: "plan", origin })
    expect(res.status).toBe(201)
    const body = await res.json()
    expect(body.kind).toBe("engine")
    expect(body.sessionId).toMatch(/^ses_/)
    expect(body.directory).toBe(path.resolve(tmp.path))
    expect(await PermissionNext.getMode(body.sessionId)).toBe("plan")
    expect((await Storage.read<any>(originKey(body.sessionId))).deviceName).toBe("Test phone")
  })

  test("a missing folder is 404 folder_missing", async () => {
    await using tmp = await tmpdir()
    const res = await post({ folder: { path: path.join(tmp.path, "gone") }, runtime: "allternit", origin })
    expect(res.status).toBe(404)
    expect((await res.json()).error).toBe("folder_missing")
  })

  test("an unknown mode is 403 mode_not_allowed; another runtime is 404 runtime_missing", async () => {
    await using tmp = await tmpdir()
    const mode = await post({ folder: { path: tmp.path }, runtime: "allternit", mode: "root", origin })
    expect(mode.status).toBe(403)
    expect((await mode.json()).error).toBe("mode_not_allowed")
    const runtime = await post({ folder: { path: tmp.path }, runtime: "claude", origin })
    expect(runtime.status).toBe(404)
    expect((await runtime.json()).error).toBe("runtime_missing")
  })
})

describe("mode ids", () => {
  test("app ids map to engine modes and back", () => {
    expect(MODE_TO_ENGINE).toEqual({ auto: "acceptEdits", default: "default", plan: "plan", bypass: "bypassPermissions" })
    for (const id of Object.keys(MODE_TO_ENGINE)) expect(appModeOf(MODE_TO_ENGINE[id as keyof typeof MODE_TO_ENGINE])).toBe(id as any)
  })
})

describe("GET /session-start/options", () => {
  test("lists folders (home last), modes and the default runtime", async () => {
    const res = await app.request("/options")
    expect(res.status).toBe(200)
    const body = await res.json()
    expect(body.defaultRuntime).toBe("allternit")
    expect(body.modes.allternit.supported).toEqual(["auto", "default", "plan", "bypass"])
    expect(body.folders.length).toBeLessThanOrEqual(30)
    expect(body.folders.at(-1).path).toBe(require("os").homedir())
  })
})
