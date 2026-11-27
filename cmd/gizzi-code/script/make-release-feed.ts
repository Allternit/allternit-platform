#!/usr/bin/env bun
/**
 * Builds and signs the Gizzi release feed (docs/contracts/gizzi-release-feed),
 * verifies one, and renders the Homebrew formula / Scoop manifest templates.
 * Run by .github/workflows/publish-gizzi-code-npm.yml (job release-feed). Not a build.
 *
 * Build (writes <out>/latest.json + <out>/latest.json.sig):
 *   GIZZI_FEED_SIGNING_KEY="$(cat key.pem)" bun script/make-release-feed.ts \
 *     --version 2.24.0 --channel stable --checksums release-assets/checksums.txt \
 *     --assets-dir release-assets \
 *     --base-url https://github.com/Allternit/allternit-platform/releases/download/gizzi-code/v2.24.0 \
 *     [--changelog CHANGELOG.md] [--min-version 2.20.0] [--previous old/latest.json]
 *     [--notes-url URL] [--paused] [--out feed/gizzi/stable]
 *   The key is an Ed25519 private key, PKCS8 PEM. The signature is base64 Ed25519
 *   over the exact bytes of latest.json. Without the key the build fails, unless
 *   --unsigned is passed (local checks only; the workflow never passes it).
 *   minVersion: --min-version, else the previous feed's minVersion when --previous
 *   is given and its signature checks out with the same key, else 0.0.0.
 *
 * Verify (exit 0 only if the signature matches and the feed passes every check):
 *   bun script/make-release-feed.ts --verify pubkey.pem [--feed latest.json] [--sig latest.json.sig]
 *
 * Render a packaging template (@@VERSION@@, @@SHA256:<target>@@):
 *   bun script/make-release-feed.ts render --template packaging/homebrew/gizzi-code.rb \
 *     --version 2.24.0 --checksums release-assets/checksums.txt --out gizzi-code.rb
 *
 * Refuses to write a feed that the updater (src/cli/update, PR #3197) would reject:
 * every archive must hold gizzi-code[.exe] at its top level, every artifact url
 * must be https, packages.npm must be exactly @allternit/gizzi-code@<version>, and
 * sha256 + size are taken from the archive file itself and must match checksums.txt.
 */
import { createHash, createPrivateKey, createPublicKey, sign, verify, type KeyObject } from "node:crypto"
import { existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { spawnSync } from "node:child_process"

const TARGETS = ["darwin-arm64", "darwin-x64", "linux-x64", "linux-arm64", "win32-x64"] as const
type Target = (typeof TARGETS)[number]

const VERSION_RE = /^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$/
const SHA_RE = /^[0-9a-f]{64}$/
const NOTES_URL = "https://docs.allternit.com/release-notes"
const BREW = "allternit/tap/gizzi-code"
const SCOOP = "gizzi-code"

/** Release asset file name per feed target (matches the workflow's "Pack release assets"). */
export function assetName(version: string, target: Target): string {
  return target === "win32-x64"
    ? `gizzi-code-v${version}-windows-x64.zip`
    : `gizzi-code-v${version}-${target}.tar.gz`
}

type Args = { _: string[]; [k: string]: string | boolean | string[] }

function parseArgs(argv: string[]): Args {
  const out: Args = { _: [] }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]!
    if (!a.startsWith("--")) {
      out._.push(a)
      continue
    }
    const key = a.slice(2)
    const next = argv[i + 1]
    if (next === undefined || next.startsWith("--")) out[key] = true
    else {
      out[key] = next
      i++
    }
  }
  return out
}

function str(args: Args, key: string, fallback?: string): string {
  const v = args[key]
  if (typeof v === "string" && v.length > 0) return v
  if (fallback !== undefined) return fallback
  fail(`--${key} is required`)
}

function fail(msg: string): never {
  console.error(`make-release-feed: ${msg}`)
  process.exit(1)
}

/** checksums.txt from `sha256sum *`: "<hex>  <name>" (binary mode prefixes the name with "*"). */
export function parseChecksums(text: string): Map<string, string> {
  const sums = new Map<string, string>()
  for (const line of text.split(/\r?\n/)) {
    const parts = line.trim().split(/\s+/)
    if (parts.length !== 2) continue
    const [hex, rawName] = parts as [string, string]
    if (!SHA_RE.test(hex)) continue
    sums.set(rawName.replace(/^\*/, ""), hex)
  }
  return sums
}

/** Top section of CHANGELOG.md for this version (else the first released section): up to 4 short lines. */
export function changelogHighlights(text: string, version: string): string[] {
  const sections: { title: string; body: string[] }[] = []
  for (const line of text.split(/\r?\n/)) {
    const h = /^##\s+(.+?)\s*$/.exec(line)
    if (h) sections.push({ title: h[1]!, body: [] })
    else sections.at(-1)?.body.push(line)
  }
  const filled = sections.filter((s) => s.body.some((l) => /^\s*[-*]\s+\S/.test(l)))
  const exact = filled.find(
    (s) => s.title === version || s.title.startsWith(`${version} `) || s.title.startsWith(`v${version} `) || s.title === `v${version}`,
  )
  const section = exact ?? filled[0]
  if (!section) return []
  if (!exact) console.warn(`make-release-feed: CHANGELOG has no "## ${version}" section; using "## ${section.title}"`)
  const out: string[] = []
  for (const line of section.body) {
    const m = /^\s*[-*]\s+(.+)$/.exec(line)
    if (!m) continue
    let item = m[1]!
    const bold = /^\*\*(.+?)\*\*/.exec(item)
    item = bold ? bold[1]! : item
    item = item.replace(/`/g, "").replace(/\*\*/g, "").replace(/\[([^\]]+)\]\([^)]+\)/g, "$1").trim()
    item = item.replace(/[.:]$/, "")
    if (item.length > 120) item = `${item.slice(0, 117).trimEnd()}...`
    if (item) out.push(item)
    if (out.length === 4) break
  }
  return out
}

function compareVersions(a: string, b: string): number {
  const core = (v: string) => v.split("-")[0]!.split(".").map(Number)
  const [x, y] = [core(a), core(b)]
  for (let i = 0; i < 3; i++) if (x[i]! !== y[i]!) return x[i]! - y[i]!
  const [pa, pb] = [a.includes("-"), b.includes("-")]
  if (pa !== pb) return pa ? -1 : 1
  return a < b ? -1 : a > b ? 1 : 0
}

/** Entries of an archive, without a leading "./". */
function archiveEntries(file: string): string[] {
  const zip = file.endsWith(".zip")
  const r = zip ? spawnSync("unzip", ["-Z1", file], { encoding: "utf8" }) : spawnSync("tar", ["-tzf", file], { encoding: "utf8" })
  if (r.status !== 0) fail(`cannot list ${file}: ${(r.stderr || r.error?.message || "").trim()}`)
  return r.stdout
    .split(/\r?\n/)
    .map((e) => e.trim().replace(/^\.\//, ""))
    .filter((e) => e.length > 0 && e !== ".")
}

export function checkArchiveLayout(file: string, target: Target): void {
  const want = target === "win32-x64" ? "gizzi-code.exe" : "gizzi-code"
  const entries = archiveEntries(file)
  if (!entries.includes(want)) {
    fail(`${file} has no top-level ${want} (entries: ${entries.slice(0, 8).join(", ") || "none"}); the updater only installs a top-level binary`)
  }
}

type Artifact = { url: string; sha256: string; size: number }
type Feed = {
  schema: 1
  channel: "stable" | "latest"
  version: string
  publishedAt: string
  minVersion: string
  paused: boolean
  notes: { url: string; highlights: string[] }
  artifacts: Record<Target, Artifact>
  packages: { npm: string; brew: string; scoop: string }
}

/** Schema rules from docs/contracts/gizzi-release-feed/schema.json plus the updater's own. Returns problems. */
export function feedProblems(feed: unknown): string[] {
  const p: string[] = []
  const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v)
  const exact = (o: Record<string, unknown>, keys: string[], where: string) => {
    for (const k of keys) if (!(k in o)) p.push(`${where}: missing ${k}`)
    for (const k of Object.keys(o)) if (!keys.includes(k)) p.push(`${where}: unexpected key ${k}`)
  }
  const httpsUrl = (v: unknown, where: string) => {
    if (typeof v !== "string") return p.push(`${where} is not a string`)
    try {
      if (new URL(v).protocol !== "https:") p.push(`${where} is not https: ${v}`)
    } catch {
      p.push(`${where} is not a URL: ${v}`)
    }
  }
  if (!isObj(feed)) return ["feed is not an object"]
  exact(feed, ["schema", "channel", "version", "publishedAt", "minVersion", "paused", "notes", "artifacts", "packages"], "feed")
  if (feed.schema !== 1) p.push("schema must be 1")
  if (feed.channel !== "stable" && feed.channel !== "latest") p.push(`channel must be stable or latest`)
  const version = typeof feed.version === "string" ? feed.version : ""
  if (!VERSION_RE.test(version)) p.push(`version ${String(feed.version)} is not semver`)
  if (typeof feed.minVersion !== "string" || !VERSION_RE.test(feed.minVersion)) p.push(`minVersion is not semver`)
  else if (VERSION_RE.test(version) && compareVersions(feed.minVersion, version) > 0) p.push(`minVersion ${feed.minVersion} is above version ${version}`)
  if (typeof feed.publishedAt !== "string" || Number.isNaN(Date.parse(feed.publishedAt))) p.push("publishedAt is not a date-time")
  if (typeof feed.paused !== "boolean") p.push("paused is not a boolean")
  if (!isObj(feed.notes)) p.push("notes is not an object")
  else {
    exact(feed.notes, ["url", "highlights"], "notes")
    httpsUrl(feed.notes.url, "notes.url")
    const h = feed.notes.highlights
    if (!Array.isArray(h) || h.length > 4 || h.some((x) => typeof x !== "string")) p.push("notes.highlights must be up to 4 strings")
  }
  if (!isObj(feed.artifacts)) p.push("artifacts is not an object")
  else {
    exact(feed.artifacts, [...TARGETS], "artifacts")
    for (const t of TARGETS) {
      const a = feed.artifacts[t]
      if (!isObj(a)) continue
      exact(a, ["url", "sha256", "size"], `artifacts.${t}`)
      httpsUrl(a.url, `artifacts.${t}.url`)
      if (typeof a.sha256 !== "string" || !SHA_RE.test(a.sha256)) p.push(`artifacts.${t}.sha256 is not 64 lowercase hex`)
      if (!Number.isInteger(a.size) || (a.size as number) < 1) p.push(`artifacts.${t}.size must be a positive integer`)
    }
  }
  if (!isObj(feed.packages)) p.push("packages is not an object")
  else {
    exact(feed.packages, ["npm", "brew", "scoop"], "packages")
    if (feed.packages.npm !== `@allternit/gizzi-code@${version}`) p.push(`packages.npm must be @allternit/gizzi-code@${version}`)
    if (typeof feed.packages.brew !== "string" || typeof feed.packages.scoop !== "string") p.push("packages.brew and packages.scoop must be strings")
  }
  return p
}

function loadPrivateKey(): KeyObject | undefined {
  const pem = process.env.GIZZI_FEED_SIGNING_KEY
  if (!pem || !pem.trim()) return undefined
  let key: KeyObject
  try {
    key = createPrivateKey({ key: pem, format: "pem" })
  } catch {
    fail("GIZZI_FEED_SIGNING_KEY is not a readable PEM private key")
  }
  if (key.asymmetricKeyType !== "ed25519") fail(`GIZZI_FEED_SIGNING_KEY is ${key.asymmetricKeyType}, expected ed25519`)
  return key
}

function verifyBytes(bytes: Buffer, sigB64: string, pub: KeyObject): boolean {
  const sig = Buffer.from(sigB64.trim(), "base64")
  if (sig.length !== 64) return false
  return verify(null, bytes, pub, sig)
}

function build(args: Args): void {
  const version = str(args, "version").replace(/^v/, "")
  if (!VERSION_RE.test(version)) fail(`--version ${version} is not semver`)
  const channel = str(args, "channel")
  if (channel !== "stable" && channel !== "latest") fail("--channel must be stable or latest")
  const assetsDir = str(args, "assets-dir")
  const baseUrl = str(args, "base-url").replace(/\/+$/, "")
  const out = str(args, "out", ".")
  const sums = parseChecksums(readFileSync(str(args, "checksums"), "utf8"))

  const artifacts = {} as Record<Target, Artifact>
  for (const t of TARGETS) {
    const name = assetName(version, t)
    const file = join(assetsDir, name)
    if (!existsSync(file)) fail(`missing release asset ${file} (the feed needs all five targets)`)
    const bytes = readFileSync(file)
    const sha256 = createHash("sha256").update(bytes).digest("hex")
    const listed = sums.get(name)
    if (!listed) fail(`checksums.txt has no line for ${name}`)
    if (listed !== sha256) fail(`${name}: sha256 ${sha256} does not match checksums.txt ${listed}`)
    checkArchiveLayout(file, t)
    artifacts[t] = { url: `${baseUrl}/${encodeURIComponent(name)}`, sha256, size: statSync(file).size }
  }

  const key = loadPrivateKey()
  if (!key && args.unsigned !== true) fail("GIZZI_FEED_SIGNING_KEY is not set (pass --unsigned for a local, unsigned check)")

  let minVersion = typeof args["min-version"] === "string" ? args["min-version"] : ""
  if (!minVersion && typeof args.previous === "string" && existsSync(args.previous)) {
    const prevBytes = readFileSync(args.previous)
    const sigPath = typeof args["previous-sig"] === "string" ? args["previous-sig"] : `${args.previous}.sig`
    const trusted = key && existsSync(sigPath) && verifyBytes(prevBytes, readFileSync(sigPath, "utf8"), createPublicKey(key))
    if (trusted) {
      const prev = JSON.parse(prevBytes.toString("utf8")) as { minVersion?: unknown }
      if (typeof prev.minVersion === "string" && VERSION_RE.test(prev.minVersion)) minVersion = prev.minVersion
    } else {
      console.warn(`make-release-feed: previous feed ${args.previous} is not signed by this key; not carrying its minVersion`)
    }
  }
  if (!minVersion) minVersion = "0.0.0"
  if (compareVersions(minVersion, version) > 0) fail(`minVersion ${minVersion} is above version ${version}`)

  const feed: Feed = {
    schema: 1,
    channel,
    version,
    publishedAt: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
    minVersion,
    paused: args.paused === true,
    notes: {
      url: str(args, "notes-url", NOTES_URL),
      highlights: typeof args.changelog === "string" ? changelogHighlights(readFileSync(args.changelog, "utf8"), version) : [],
    },
    artifacts,
    packages: { npm: `@allternit/gizzi-code@${version}`, brew: BREW, scoop: SCOOP },
  }
  const problems = feedProblems(feed)
  if (problems.length) fail(`refusing to write an invalid feed:\n  ${problems.join("\n  ")}`)

  const bytes = Buffer.from(`${JSON.stringify(feed, null, 2)}\n`, "utf8")
  mkdirSync(out, { recursive: true })
  writeFileSync(join(out, "latest.json"), bytes)
  if (key) {
    const sig = sign(null, bytes, key).toString("base64")
    if (!verifyBytes(bytes, sig, createPublicKey(key))) fail("signature did not verify right after signing")
    writeFileSync(join(out, "latest.json.sig"), `${sig}\n`)
    console.log(`wrote ${join(out, "latest.json")} + .sig (${channel} ${version}, minVersion ${minVersion})`)
  } else {
    console.log(`wrote ${join(out, "latest.json")} UNSIGNED (${channel} ${version})`)
  }
}

function verifyMode(args: Args): void {
  const pubPath = str(args, "verify")
  const feedPath = str(args, "feed", "latest.json")
  const sigPath = str(args, "sig", `${feedPath}.sig`)
  let pub: KeyObject
  try {
    pub = createPublicKey({ key: readFileSync(pubPath, "utf8"), format: "pem" })
  } catch {
    fail(`${pubPath} is not a readable PEM public key`)
  }
  if (pub.asymmetricKeyType !== "ed25519") fail(`${pubPath} is ${pub.asymmetricKeyType}, expected ed25519`)
  const bytes = readFileSync(feedPath)
  if (!verifyBytes(bytes, readFileSync(sigPath, "utf8"), pub)) fail(`signature in ${sigPath} does not match ${feedPath}`)
  const problems = feedProblems(JSON.parse(bytes.toString("utf8")))
  if (problems.length) fail(`${feedPath} is signed but invalid:\n  ${problems.join("\n  ")}`)
  console.log(`ok: ${feedPath} signature and contents check out`)
}

function render(args: Args): void {
  const version = str(args, "version").replace(/^v/, "")
  if (!VERSION_RE.test(version)) fail(`--version ${version} is not semver`)
  const template = str(args, "template")
  const sums = parseChecksums(readFileSync(str(args, "checksums"), "utf8"))
  let text = readFileSync(template, "utf8").replaceAll("@@VERSION@@", version)
  text = text.replace(/@@SHA256:([a-z0-9-]+)@@/g, (_m, t: string) => {
    if (!(TARGETS as readonly string[]).includes(t)) fail(`${template}: unknown target ${t}`)
    const name = assetName(version, t as Target)
    const hex = sums.get(name)
    if (!hex) fail(`checksums.txt has no line for ${name} (needed by ${template})`)
    return hex
  })
  if (text.includes("@@")) fail(`${template}: unrendered placeholder left`)
  writeFileSync(str(args, "out"), text)
  console.log(`rendered ${template} -> ${String(args.out)} (${version})`)
}

if (import.meta.main) {
  const args = parseArgs(process.argv.slice(2))
  if (args._[0] === "render") render(args)
  else if (typeof args.verify === "string") verifyMode(args)
  else build(args)
}
