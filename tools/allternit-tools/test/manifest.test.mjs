import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { loadManifest, loadHarness, resolveTool, installSpec } from "../allternit-tools.mjs"

const here = path.dirname(fileURLToPath(import.meta.url))
const manifest = loadManifest(path.join(here, "..", "manifest.json"))
const harness = loadHarness(manifest, {})

const PINNED = new Set(["npm", "pip-venv", "binary", "archive"])

test("every tool has the required fields", () => {
  for (const t of manifest.tools) {
    assert.ok(t.id && t.name && t.bin, `${t.id}: id/name/bin`)
    assert.ok(["cli", "local-model", "gateway-vendor", "base"].includes(t.category), `${t.id}: category`)
    assert.ok(t.install && Object.keys(t.install).length, `${t.id}: install`)
    assert.ok(Array.isArray(t.verify?.argv), `${t.id}: verify.argv`)
    assert.ok(t.login?.method, `${t.id}: login.method`)
    assert.equal(typeof t.terms?.required, "boolean", `${t.id}: terms.required`)
    if (t.terms.required) assert.match(t.terms.url || "", /^https:\/\//, `${t.id}: terms url`)
  }
})

test("ids and aliases are unique", () => {
  const seen = new Map()
  for (const t of manifest.tools) {
    for (const k of [t.id, ...(t.aliases || [])]) {
      assert.ok(!seen.has(k), `${k} used by ${seen.get(k)} and ${t.id}`)
      seen.set(k, t.id)
    }
  }
})

test("nothing is unpinned or @latest", () => {
  for (const t of manifest.tools) {
    for (const [osKey, spec] of Object.entries(t.install)) {
      if (!PINNED.has(spec.method)) continue
      assert.ok(spec.version && spec.version !== "latest", `${t.id}/${osKey}: version`)
      for (const p of spec.packages || []) {
        if (spec.method === "npm") assert.match(p, /^(@[^/]+\/)?[^@]+@\d[\w.-]*$/, `${t.id}: npm pin ${p}`)
      }
      if (spec.method === "pip-venv") assert.match(spec.packages[0], /==\d/, `${t.id}: pip pin`)
      if (spec.method === "npm") assert.ok(spec.packages[0].endsWith(`@${spec.version}`), `${t.id}: version matches package`)
      if (spec.method === "binary") assert.ok(spec.sha256Url, `${t.id}: binary downloads are checksummed`)
    }
  }
})

test("pins agree with ao harness.json (no second source of truth)", () => {
  assert.ok(harness, "harness.json resolvable")
  for (const t of manifest.tools.filter((x) => x.harness)) {
    const h = harness.tools[t.harness]
    assert.ok(h, `${t.id}: harness entry ${t.harness}`)
    const spec = installSpec(t, "linux")
    // Vendor scripts are rolling (the version is a record, not a pin).
    if (!PINNED.has(spec.method)) continue
    assert.equal(spec.version, h.install.pinnedVersion, `${t.id}: pin`)
    if (spec.packages) assert.equal(spec.packages[0], h.install.installArgs[0], `${t.id}: primary package`)
  }
})

test("appendix conflicts are resolved", () => {
  assert.equal(installSpec(resolveTool(manifest, "pi"), "linux").packages[0].split("@").slice(0, 2).join("@"), "@earendil-works/pi-coding-agent")
  assert.equal(installSpec(resolveTool(manifest, "dsh"), "linux").method, "pip-venv")
  assert.equal(installSpec(resolveTool(manifest, "agy"), "darwin").method, "script")
  assert.equal(installSpec(resolveTool(manifest, "gemini"), "darwin").method, "npm")
})

test("OpenCode and Droid are first-class and resolve by provider id", () => {
  assert.equal(resolveTool(manifest, "opencode").providerId, "opencode")
  assert.equal(resolveTool(manifest, "droid").providerId, "droid")
  assert.equal(resolveTool(manifest, "factory").id, "droid")
  assert.equal(resolveTool(manifest, "claude-cli").id, "claude")
  assert.equal(resolveTool(manifest, "antigravity").id, "agy")
  assert.equal(resolveTool(manifest, "gemini-cli").id, "gemini")
})

test("a web copy of the manifest, if present, is in sync", () => {
  const web = process.env.ALLTERNIT_AI_PATH && path.join(process.env.ALLTERNIT_AI_PATH, "src/lib/providers/allternit-tools.manifest.json")
  if (!web || !fs.existsSync(web)) return
  assert.deepEqual(JSON.parse(fs.readFileSync(web, "utf8")), JSON.parse(fs.readFileSync(manifest._file, "utf8")))
})
