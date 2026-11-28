import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import crypto from "node:crypto"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { ReleaseFeed } from "../../src/shared/installation/feed"
import { GIZZI_FEED_KEY_PLACEHOLDER } from "../../src/shared/installation/feed-key"
import { Installation } from "../../src/shared/installation"
import { Updater } from "../../src/shared/installation/updater"

// The network is the edge: a fake fetch serves the feed, its signature and the
// artifact. Everything else (verify, hash, stage, swap, state file) is real.

const BASE = "https://feed.test/gizzi"
const ARTIFACT_URL = "https://github.test/gizzi-darwin-arm64"
const NEW_BINARY = Buffer.from("#!/bin/sh\necho gizzi 2.24.0\n")
const OLD_BINARY = Buffer.from("#!/bin/sh\necho gizzi 2.19.0\n")

function keypair() {
  const { publicKey, privateKey } = crypto.generateKeyPairSync("ed25519")
  return { pub: publicKey.export({ type: "spki", format: "pem" }).toString(), priv: privateKey }
}

function feedJson(over: Record<string, unknown> = {}, sha256 = crypto.createHash("sha256").update(NEW_BINARY).digest("hex")) {
  return JSON.stringify({
    schema: 1,
    channel: "stable",
    version: "2.24.0",
    publishedAt: "2026-11-27T00:00:00Z",
    minVersion: "2.10.0",
    paused: false,
    notes: { url: "https://docs.test/notes", highlights: ["Signed updates"] },
    artifacts: { "darwin-arm64": { url: ARTIFACT_URL, sha256, size: NEW_BINARY.length } },
    packages: { npm: "@allternit/gizzi-code@2.24.0", brew: "allternit/tap/gizzi-code", scoop: "gizzi-code" },
    ...over,
  })
}

function fakeFetch(body: string, signer: crypto.KeyObject, artifact = NEW_BINARY) {
  const calls: string[] = []
  const sig = crypto.sign(null, Buffer.from(body), signer).toString("base64")
  const impl = (async (input: string | URL | Request) => {
    const url = String(input instanceof Request ? input.url : input)
    calls.push(url)
    if (url === `${BASE}/stable/latest.json`) return new Response(body)
    if (url === `${BASE}/stable/latest.json.sig`) return new Response(sig)
    if (url === ARTIFACT_URL) return new Response(artifact)
    return new Response("not found", { status: 404 })
  }) as unknown as typeof fetch
  return { impl, calls }
}

let dir: string
let stateDir: string
let binary: string
const signing = keypair()

beforeEach(() => {
  dir = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "gizzi-updater-")))
  stateDir = path.join(dir, "state")
  binary = path.join(dir, "bin", "gizzi")
  fs.mkdirSync(path.dirname(binary), { recursive: true })
  fs.writeFileSync(binary, OLD_BINARY, { mode: 0o755 })
  Updater.resetForTests()
})

afterEach(() => {
  Updater.resetForTests()
  fs.rmSync(dir, { recursive: true, force: true })
})

function opts(fetchImpl: typeof fetch, over: Partial<Updater.Options> = {}): Updater.Options {
  return {
    method: "curl",
    current: "2.19.0",
    platform: "darwin-arm64",
    execPath: binary,
    env: {},
    stateDir,
    fetch: fetchImpl,
    feed: { base: BASE, publicKey: signing.pub, retryMs: 0 },
    announce: () => {},
    ...over,
  }
}

function stagedFiles() {
  return fs.readdirSync(path.dirname(binary)).filter((n) => n.startsWith(".gizzi-update-"))
}

describe("signed feed", () => {
  test("a bad signature refuses: nothing is downloaded or staged", async () => {
    const other = keypair()
    const { impl, calls } = fakeFetch(feedJson(), other.priv)

    const direct = await ReleaseFeed.fetchFeed({ base: BASE, channel: "stable", publicKey: signing.pub, fetch: impl, retryMs: 0 })
    expect(direct).toEqual({ ok: false, reason: "feed signature invalid" })

    const status = await Updater.check(opts(impl))
    expect(status.state).toBe("error")
    expect(status.reason).toBe("feed signature invalid")
    expect(calls).not.toContain(ARTIFACT_URL)
    expect(stagedFiles()).toEqual([])
    expect(ReleaseFeed.readState(stateDir).lastCheckResult).toBe("feed signature invalid")
  })

  test("the placeholder key does nothing and says 'feed key not set'", async () => {
    const { impl, calls } = fakeFetch(feedJson(), signing.priv)
    const status = await Updater.check(
      opts(impl, { env: { GIZZI_FEED_PUBLIC_KEY: GIZZI_FEED_KEY_PLACEHOLDER }, feed: { base: BASE, retryMs: 0 } }),
    )
    expect(status.state).toBe("key-missing")
    expect(status.reason).toBe("feed key not set")
    expect(calls).toEqual([])
    expect(fs.readFileSync(binary)).toEqual(OLD_BINARY)
  })

  test("a signed feed for another channel is refused", async () => {
    const { impl } = fakeFetch(feedJson({ channel: "latest" }), signing.priv)
    const status = await Updater.check(opts(impl))
    expect(status.state).toBe("error")
    expect(status.reason).toBe("feed unreadable")
  })
})

describe("binary update (curl / native)", () => {
  test("a bad sha256 refuses: the download is deleted and the binary is untouched", async () => {
    const { impl } = fakeFetch(feedJson({}, "0".repeat(64)), signing.priv)
    const status = await Updater.check(opts(impl))
    expect(status.state).toBe("error")
    expect(status.reason).toBe("sha256 mismatch")
    expect(stagedFiles()).toEqual([])
    expect(ReleaseFeed.readState(stateDir).staged).toBeUndefined()
    expect(Updater.applyStaged({ current: "2.19.0", stateDir })).toBe(false)
    expect(fs.readFileSync(binary)).toEqual(OLD_BINARY)
  })

  test("a verified artifact is staged, then swapped in place of the installed binary", async () => {
    const { impl } = fakeFetch(feedJson(), signing.priv)
    const status = await Updater.check(opts(impl))
    expect(status.state).toBe("staged")
    expect(status.latest).toBe("2.24.0")
    expect(fs.readFileSync(binary)).toEqual(OLD_BINARY)

    expect(Updater.applyStaged({ current: "2.19.0", stateDir, platform: "darwin" })).toBe(true)
    expect(fs.readFileSync(binary)).toEqual(NEW_BINARY)
    expect(stagedFiles()).toEqual([])
    const st = ReleaseFeed.readState(stateDir)
    expect(st.lastUpdateFrom).toBe("2.19.0")
    expect(st.lastUpdateTo).toBe("2.24.0")
  })

  test("a release archive is unpacked: gizzi-code and its helpers are swapped together", async () => {
    // Real release assets are archives holding gizzi-code + helpers (see cmd/gizzi-code/install).
    const src = path.join(dir, "src")
    fs.mkdirSync(src)
    fs.writeFileSync(path.join(src, "gizzi-code"), NEW_BINARY, { mode: 0o755 })
    fs.writeFileSync(path.join(src, "allternit-factory"), "factory 2.24.0", { mode: 0o755 })
    const archive = path.join(dir, "gizzi-darwin-arm64.tar.gz")
    expect(Bun.spawnSync(["tar", "-czf", archive, "-C", src, "gizzi-code", "allternit-factory"]).exitCode).toBe(0)
    const bytes = fs.readFileSync(archive)
    const sha = crypto.createHash("sha256").update(bytes).digest("hex")
    const url = "https://github.test/gizzi-darwin-arm64.tar.gz"
    const body = feedJson({ artifacts: { "darwin-arm64": { url, sha256: sha, size: bytes.length } } })
    const sig = crypto.sign(null, Buffer.from(body), signing.priv).toString("base64")
    const impl = (async (input: string | URL | Request) => {
      const u = String(input instanceof Request ? input.url : input)
      if (u.endsWith("/latest.json")) return new Response(body)
      if (u.endsWith("/latest.json.sig")) return new Response(sig)
      if (u === url) return new Response(bytes)
      return new Response("", { status: 404 })
    }) as unknown as typeof fetch

    const target = path.join(path.dirname(binary), "gizzi-code")
    fs.writeFileSync(target, OLD_BINARY, { mode: 0o755 })
    const status = await Updater.check(opts(impl, { execPath: target }))
    expect(status.state).toBe("staged")
    expect(Updater.applyStaged({ current: "2.19.0", stateDir, platform: "darwin" })).toBe(true)
    expect(fs.readFileSync(target)).toEqual(NEW_BINARY)
    expect(fs.readFileSync(path.join(path.dirname(target), "allternit-factory"), "utf8")).toBe("factory 2.24.0")
    expect(stagedFiles()).toEqual([])
  })

  test("Windows swap renames the running exe aside first", async () => {
    const { impl } = fakeFetch(feedJson(), signing.priv)
    await Updater.check(opts(impl))
    expect(Updater.applyStaged({ current: "2.19.0", stateDir, platform: "win32" })).toBe(true)
    expect(fs.readFileSync(binary)).toEqual(NEW_BINARY)
    const aside = fs.readdirSync(path.dirname(binary)).filter((n) => n.startsWith("gizzi.old-"))
    expect(aside.length).toBe(1)
    Updater.cleanupAside(binary)
    expect(fs.readdirSync(path.dirname(binary)).filter((n) => n.startsWith("gizzi.old-"))).toEqual([])
  })

  test("a staged file changed after download is not swapped in", async () => {
    const { impl } = fakeFetch(feedJson(), signing.priv)
    await Updater.check(opts(impl))
    const staged = ReleaseFeed.readState(stateDir).staged!
    fs.writeFileSync(staged.files[0]!.from, "tampered")
    expect(Updater.applyStaged({ current: "2.19.0", stateDir })).toBe(false)
    expect(fs.readFileSync(binary)).toEqual(OLD_BINARY)
  })
})

describe("feed controls", () => {
  test("paused does nothing, even with a newer version and a minVersion above the running one", async () => {
    const { impl, calls } = fakeFetch(feedJson({ paused: true, minVersion: "2.20.0" }), signing.priv)
    const status = await Updater.check(opts(impl))
    expect(status.state).toBe("paused")
    expect(calls).not.toContain(ARTIFACT_URL)
    expect(stagedFiles()).toEqual([])
    expect(fs.readFileSync(binary)).toEqual(OLD_BINARY)
  })

  test("paused drops an update staged by an earlier check", async () => {
    await Updater.check(opts(fakeFetch(feedJson(), signing.priv).impl))
    expect(stagedFiles().length).toBe(1)
    await Updater.check(opts(fakeFetch(feedJson({ paused: true }), signing.priv).impl))
    expect(stagedFiles()).toEqual([])
    expect(Updater.applyStaged({ current: "2.19.0", stateDir })).toBe(false)
  })

  test("auto-update off: a newer version is only reported", async () => {
    const { impl, calls } = fakeFetch(feedJson(), signing.priv)
    const status = await Updater.check(opts(impl, { env: { GIZZI_DISABLE_AUTOUPDATE: "1" } }))
    expect(status.state).toBe("available")
    expect(status.required).toBe(false)
    expect(calls).not.toContain(ARTIFACT_URL)
  })

  test("minVersion forces the update even with auto-update off", async () => {
    const { impl } = fakeFetch(feedJson({ minVersion: "2.20.0" }), signing.priv)
    const status = await Updater.check(opts(impl, { env: { GIZZI_DISABLE_AUTOUPDATE: "1" } }))
    expect(status.required).toBe(true)
    expect(status.state).toBe("staged")
    expect(status.message).toContain("required")
  })

  test("same version is current and stages nothing", async () => {
    const { impl, calls } = fakeFetch(feedJson(), signing.priv)
    const status = await Updater.check(opts(impl, { current: "2.24.0" }))
    expect(status.state).toBe("current")
    expect(calls).not.toContain(ARTIFACT_URL)
  })
})

describe("install methods", () => {
  test("brew prints the command and never downloads", async () => {
    const { impl, calls } = fakeFetch(feedJson(), signing.priv)
    const status = await Updater.check(opts(impl, { method: "brew" }))
    expect(status.state).toBe("available")
    expect(status.command).toBe("brew upgrade allternit/tap/gizzi-code")
    expect(calls).not.toContain(ARTIFACT_URL)
  })

  test("npm schedules `npm i -g` for exit", async () => {
    const { impl } = fakeFetch(feedJson(), signing.priv)
    const status = await Updater.check(opts(impl, { method: "npm" }))
    expect(status.state).toBe("scheduled")
    const spawned: string[][] = []
    expect(Updater.runPendingPackage({ stateDir, spawn: (cmd, args) => spawned.push([cmd, ...args]) })).toBe(true)
    expect(spawned).toEqual([["npm", "i", "-g", "@allternit/gizzi-code@2.24.0"]])
  })

  test("npm refuses a feed package spec that is not ours", async () => {
    const { impl } = fakeFetch(feedJson({ packages: { npm: "evil-pkg@1.0.0" } }), signing.priv)
    const status = await Updater.check(opts(impl, { method: "npm" }))
    expect(status.state).toBe("available")
    expect(ReleaseFeed.readState(stateDir).pendingPackage).toBeUndefined()
  })

  test("a Desktop-managed copy makes no network call", async () => {
    const { impl, calls } = fakeFetch(feedJson(), signing.priv)
    const status = await Updater.check(opts(impl, { method: "desktop" }))
    expect(status.state).toBe("managed")
    expect(calls).toEqual([])
  })

  test("Desktop's bundled binary and runtime tree are detected by real path; a curl install is not", () => {
    expect(Installation.isDesktopManaged(["/Applications/Allternit.app/Contents/Resources/bin/gizzi-code"], {})).toBe(true)
    expect(
      Installation.isDesktopManaged(["/Users/u/Library/Application Support/Allternit/runtime/versions/2.24.0/bin/gizzi-code"], {}),
    ).toBe(true)
    expect(Installation.isDesktopManaged(["C:\\Program Files\\Allternit\\resources\\bin\\gizzi-code.exe"], {})).toBe(true)
    expect(Installation.isDesktopManaged(["/Users/u/.local/bin/gizzi"], {})).toBe(false)
    expect(Installation.isDesktopManaged(["/Users/u/.local/bin/gizzi"], { ALLTERNIT_DESKTOP_MANAGED: "1" })).toBe(true)
  })
})

describe("contract fixtures (docs/contracts/gizzi-release-feed, step 8 / #3192)", () => {
  const fixtures = path.join(import.meta.dir, "..", "..", "..", "..", "docs", "contracts", "gizzi-release-feed", "fixtures")

  test("every fixture parses with the client schema", () => {
    const names = fs.readdirSync(fixtures).filter((n) => n.endsWith(".json")).sort()
    expect(names).toEqual(["latest.json", "min-version-forces-update.json", "paused.json", "stable.json"])
    const parsed = Object.fromEntries(names.map((n) => [n, ReleaseFeed.parse(fs.readFileSync(path.join(fixtures, n)))]))
    expect(parsed["paused.json"]!.paused).toBe(true)
    expect(parsed["latest.json"]!.channel).toBe("latest")
    expect(parsed["min-version-forces-update.json"]!.minVersion).toBe(parsed["min-version-forces-update.json"]!.version)
    expect(Object.keys(parsed["stable.json"]!.artifacts).sort()).toEqual(["darwin-arm64", "darwin-x64", "linux-arm64", "linux-x64", "win32-x64"])
  })
})
