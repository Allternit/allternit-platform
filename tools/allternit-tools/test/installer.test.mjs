import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import crypto from "node:crypto"
import fs from "node:fs"
import http from "node:http"
import os from "node:os"
import path from "node:path"
import { fill, pickSha, installSpec, makeContext, installOne, resolveTool, termsAccepted, recordTerms, layout, statusOf, toolsPath } from "../allternit-tools.mjs"

const BIN = "#!/bin/sh\necho fake-tool 1.2.3\n"
const SCRIPT = '#!/bin/sh\nset -e\nmkdir -p "$FAKE_DIR"\nprintf \'#!/bin/sh\\necho scripted 4.5.6\\n\' > "$FAKE_DIR/scripted"\nchmod +x "$FAKE_DIR/scripted"\n'
let server
let base
let hits = 0
let flaky = 0

before(async () => {
  server = http.createServer((req, res) => {
    hits++
    if (req.url === "/bin/fake") return res.end(BIN)
    if (req.url === "/bin/fake.sha256") return res.end(crypto.createHash("sha256").update(BIN).digest("hex") + "\n")
    if (req.url === "/bin/bad.sha256") return res.end("0".repeat(64))
    if (req.url === "/install.sh") return res.end(SCRIPT)
    if (req.url === "/flaky.sh") {
      flaky++
      if (flaky < 2) {
        res.statusCode = 503
        return res.end("busy")
      }
      return res.end(SCRIPT)
    }
    res.statusCode = 404
    res.end()
  })
  await new Promise((r) => server.listen(0, "127.0.0.1", r))
  base = `http://127.0.0.1:${server.address().port}`
})
after(() => server.close())

function fixture(tools) {
  return { version: 1, tools, _file: path.join(os.tmpdir(), "none.json") }
}
const fakeBinary = (over = {}) => ({
  id: "faketool", name: "Fake", category: "cli", bin: "faketool",
  install: { any: { method: "binary", version: "1.2.3", url: `${base}/bin/fake`, sha256Url: `${base}/bin/fake.sha256` } },
  verify: { argv: ["--version"], expect: "fake-tool" }, login: { method: "none" }, terms: { required: false }, ...over,
})

function ctxFor(manifest, extra = {}) {
  const prefix = fs.mkdtempSync(path.join(os.tmpdir(), "at-test-"))
  const events = []
  const ctx = makeContext({ prefix, manifest, emit: (e) => events.push(e), noWire: true, env: { HOME: prefix, PATH: "/usr/bin:/bin" }, ...extra })
  return { ctx, events, prefix }
}

test("template + checksum helpers", () => {
  assert.equal(fill("a/{version}/{os}-{arch}", { version: "1", os: "linux", arch: "x64" }), "a/1/linux-x64")
  assert.equal(fill("{missing}", {}), "{missing}")
  const h = "a".repeat(64)
  assert.equal(pickSha(h + "\n", "x"), h)
  assert.equal(pickSha(`${"b".repeat(64)}  other.tgz\n${h}  node.tar.gz\n`, "node.tar.gz"), h)
  assert.equal(pickSha("garbage", "x"), null)
})

test("per-OS install spec falls back to any", () => {
  const t = { install: { any: { method: "npm" }, darwin: { method: "archive" } } }
  assert.equal(installSpec(t, "darwin").method, "archive")
  assert.equal(installSpec(t, "linux").method, "npm")
  assert.equal(installSpec({ install: { darwin: {} } }, "linux"), null)
})

test("tools PATH puts the managed bin first", () => {
  const p = toolsPath("/p", "/usr/bin", "/h").split(path.delimiter)
  assert.equal(p[0], "/p/bin")
  assert.ok(p.includes("/h/.local/bin"))
})

test("binary install: download, checksum, link, verify, then idempotent", async () => {
  const m = fixture([fakeBinary()])
  const { ctx, events, prefix } = ctxFor(m)
  const r = await installOne(m.tools[0], ctx)
  assert.equal(r.ok, true, JSON.stringify(events.filter((e) => e.type === "error")))
  assert.ok(events.some((e) => e.type === "step" && e.step === "checksum_ok"))
  assert.ok(fs.lstatSync(path.join(prefix, "bin", "faketool")).isSymbolicLink())
  const state = JSON.parse(fs.readFileSync(layout(prefix).state, "utf8"))
  assert.equal(state.tools.faketool.version, "1.2.3")
  // Second run: nothing downloaded.
  const { ctx: ctx2, events: ev2 } = ctxFor(m)
  ctx2.paths = ctx.paths
  ctx2.envPath = ctx.envPath
  ctx2.env = ctx.env
  const before = hits
  const r2 = await installOne(m.tools[0], ctx2)
  assert.equal(r2.skipped, true)
  assert.equal(ev2.find((e) => e.type === "skipped").reason, "already_installed")
  assert.equal(hits, before)
  const st = await statusOf(m.tools[0], ctx)
  assert.equal(st.installed, true)
  assert.equal(st.managed, true)
  fs.rmSync(prefix, { recursive: true, force: true })
})

test("checksum mismatch fails and leaves nothing linked", async () => {
  const m = fixture([fakeBinary({ install: { any: { method: "binary", version: "1", url: `${base}/bin/fake`, sha256Url: `${base}/bin/bad.sha256` } } })])
  const { ctx, events, prefix } = ctxFor(m)
  const r = await installOne(m.tools[0], ctx)
  assert.equal(r.ok, false)
  assert.equal(events.find((e) => e.type === "error").code, "checksum_mismatch")
  assert.equal(fs.existsSync(path.join(prefix, "bin", "faketool")), false)
  fs.rmSync(prefix, { recursive: true, force: true })
})

test("terms-gated tools are skipped without acceptance and install with it (pin-scoped)", async () => {
  const tool = fakeBinary({ terms: { required: true, url: "https://example.com/terms" } })
  const m = fixture([tool])
  const a = ctxFor(m)
  const r = await installOne(tool, a.ctx)
  assert.equal(r.reason, "terms_required")
  assert.equal(fs.existsSync(path.join(a.prefix, "bin", "faketool")), false)
  const b = ctxFor(m, { acceptTerms: ["faketool"] })
  assert.equal((await installOne(tool, b.ctx)).ok, true)
  assert.equal(termsAccepted(b.ctx.paths, tool, installSpec(tool, "linux")), true)
  assert.equal(termsAccepted(b.ctx.paths, tool, { version: "9.9.9" }), false, "a pin bump re-asks")
  recordTerms(b.ctx.paths, tool, { version: "9.9.9" })
  assert.equal(termsAccepted(b.ctx.paths, tool, { version: "9.9.9" }), true)
  for (const p of [a.prefix, b.prefix]) fs.rmSync(p, { recursive: true, force: true })
})

test("script install runs non-interactively with env, retries a 503, links the binary", async () => {
  flaky = 0
  const tool = {
    id: "scripted", name: "Scripted", category: "cli", bin: "scripted",
    install: { any: { method: "script", version: "rolling", url: `${base}/flaky.sh`, env: { FAKE_DIR: "{prefix}/vendor" }, binCandidates: ["{prefix}/vendor/scripted"] } },
    verify: { argv: ["--version"] }, login: { method: "none" }, terms: { required: false },
  }
  const m = fixture([tool])
  const { ctx, events, prefix } = ctxFor(m)
  const r = await installOne(tool, ctx)
  assert.equal(r.ok, true, JSON.stringify(events.filter((e) => e.type === "error")))
  assert.ok(events.some((e) => e.type === "retry"), "first 503 was retried")
  assert.equal(fs.realpathSync(path.join(prefix, "bin", "scripted")), fs.realpathSync(path.join(prefix, "vendor", "scripted")))
  fs.rmSync(prefix, { recursive: true, force: true })
})

test("unsupported tools report no_installer, never a guide link", async () => {
  const tool = { id: "nope", name: "Nope", category: "cli", bin: "nope-xyz", install: { any: { method: "unsupported", reason: "none yet" } }, verify: { argv: ["--version"] }, login: { method: "none" }, terms: { required: false } }
  const m = fixture([tool])
  const { ctx, events, prefix } = ctxFor(m)
  const r = await installOne(tool, ctx)
  assert.equal(r.ok, false)
  assert.equal(events.find((e) => e.type === "error").code, "no_installer")
  fs.rmSync(prefix, { recursive: true, force: true })
})

test("platform-restricted tools are skipped elsewhere", async () => {
  const tool = { ...fakeBinary(), platforms: ["plan9-mips"] }
  const m = fixture([tool])
  const { ctx, prefix } = ctxFor(m)
  assert.equal((await installOne(tool, ctx)).reason, "platform")
  fs.rmSync(prefix, { recursive: true, force: true })
})

test("unknown ids resolve to null; aliases resolve", () => {
  const m = fixture([{ ...fakeBinary(), aliases: ["ft"], providerId: "fake-cli" }])
  assert.equal(resolveTool(m, "ft").id, "faketool")
  assert.equal(resolveTool(m, "fake-cli").id, "faketool")
  assert.equal(resolveTool(m, "--prefix"), null)
})
