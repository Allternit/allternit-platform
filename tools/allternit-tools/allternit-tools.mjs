#!/usr/bin/env node
// allternit-tools — install + verify every CLI tool / agent in manifest.json
// (plan D0). Zero dependencies: runs on any Node >= 18, including Electron's
// bundled Node (ELECTRON_RUN_AS_NODE=1), so Desktop main and allternit-api can
// call it on Mac and Linux without anything else installed.
//
//   allternit-tools install --all | --only a,b | --selected
//                   [--accept-terms a,b] [--select] [--json] [--prefix DIR] [--dry-run] [--force]
//   allternit-tools status [--only a,b] [--json]
//   allternit-tools list --json                 manifest + per-tool state (for UIs)
//   allternit-tools select --set a,b | --add a,b | --remove a,b | --show
//   allternit-tools uninstall --only a,b
//   allternit-tools env                          prints `export PATH=...`
//
// Everything lands in a user-writable prefix (default ~/.allternit/tools,
// override with --prefix or ALLTERNIT_TOOLS_PREFIX). No sudo, ever: OS
// packages (method "system") are installed only when we are already root
// (image builds) or via Homebrew on a Mac; otherwise they are reported.

import { spawn } from "node:child_process"
import crypto from "node:crypto"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"

const HERE = path.dirname(fileURLToPath(import.meta.url))

// ── paths / platform ─────────────────────────────────────────────────────────

export function expandHome(p, home = os.homedir()) {
  if (typeof p !== "string") return p
  return p === "~" ? home : p.startsWith("~/") ? path.join(home, p.slice(2)) : p
}

export function platformInfo(platform = process.platform, arch = process.arch) {
  const osName = platform === "darwin" ? "darwin" : platform === "linux" ? "linux" : platform
  const archName = arch === "x64" || arch === "amd64" ? "x64" : arch === "arm64" || arch === "aarch64" ? "arm64" : arch
  return { os: osName, arch: archName }
}

export function layout(prefix) {
  return {
    prefix,
    bin: path.join(prefix, "bin"),
    venvs: path.join(prefix, "venvs"),
    opt: path.join(prefix, "opt"),
    locks: path.join(prefix, "locks"),
    logs: path.join(prefix, "logs"),
    state: path.join(prefix, "state.json"),
    terms: path.join(prefix, "accepted-terms.json"),
    selection: path.join(prefix, "selection.json"),
  }
}

export function defaultPrefix(env = process.env) {
  return expandHome(env.ALLTERNIT_TOOLS_PREFIX || "~/.allternit/tools")
}

/** PATH the tools run with: our bin first, then common user bin dirs, then inherited PATH. */
export function toolsPath(prefix, basePath = process.env.PATH || "", home = os.homedir()) {
  const dirs = [
    path.join(prefix, "bin"),
    path.join(home, ".local", "bin"),
    "/opt/homebrew/bin",
    "/usr/local/bin",
    ...basePath.split(path.delimiter),
    "/usr/bin",
    "/bin",
  ].filter(Boolean)
  return [...new Set(dirs)].join(path.delimiter)
}

// ── manifest ─────────────────────────────────────────────────────────────────

export function manifestPath(env = process.env) {
  if (env.ALLTERNIT_TOOLS_MANIFEST) return env.ALLTERNIT_TOOLS_MANIFEST
  return path.join(HERE, "manifest.json")
}

export function loadManifest(file = manifestPath()) {
  const m = JSON.parse(fs.readFileSync(file, "utf8"))
  m._file = file
  return m
}

/** harness.json (ao) holds the wiring data; shipped next to the script in Desktop, in-repo otherwise. */
export function loadHarness(manifest, env = process.env) {
  const candidates = [
    env.ALLTERNIT_TOOLS_HARNESS,
    path.join(path.dirname(manifest._file || manifestPath()), "harness.json"),
    path.resolve(HERE, "..", "..", manifest.harness || ""),
  ].filter(Boolean)
  for (const c of candidates) {
    try {
      if (fs.statSync(c).isFile()) return JSON.parse(fs.readFileSync(c, "utf8"))
    } catch {}
  }
  return null
}

export function resolveTool(manifest, idOrAlias) {
  const key = String(idOrAlias || "").trim().toLowerCase()
  return (
    manifest.tools.find((t) => t.id === key) ||
    manifest.tools.find((t) => (t.aliases || []).includes(key) || t.providerId === key) ||
    null
  )
}

export function installSpec(tool, osName = platformInfo().os) {
  const i = tool.install || {}
  return i[osName] || i.any || null
}

export function fill(str, vars) {
  return String(str).replace(/\{(\w+)\}/g, (m, k) => (vars[k] !== undefined ? String(vars[k]) : m))
}

function templateVars(spec, ctx) {
  const arch = (spec.archMap && spec.archMap[ctx.plat.arch]) || ctx.plat.arch
  return { version: spec.version, os: ctx.plat.os, arch, bin: ctx.paths.bin, prefix: ctx.paths.prefix, home: ctx.home }
}

// ── json state files ─────────────────────────────────────────────────────────

function readJson(file, fallback) {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"))
  } catch {
    return fallback
  }
}

function writeJson(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true })
  const tmp = `${file}.${process.pid}.tmp`
  fs.writeFileSync(tmp, JSON.stringify(value, null, 2) + "\n")
  fs.renameSync(tmp, file)
}

export function readSelection(paths) {
  const s = readJson(paths.selection, { tools: [] })
  return Array.isArray(s.tools) ? s.tools : []
}

export function writeSelection(paths, tools) {
  writeJson(paths.selection, { tools: [...new Set(tools)].sort(), updatedAt: new Date().toISOString() })
}

/** Terms acceptance is pin-scoped: a pin bump re-asks. */
export function termsAccepted(paths, tool, spec) {
  if (!tool.terms?.required) return true
  const rec = readJson(paths.terms, {})[tool.id]
  return !!rec && rec.version === (spec?.version ?? null)
}

export function recordTerms(paths, tool, spec) {
  const all = readJson(paths.terms, {})
  all[tool.id] = { version: spec?.version ?? null, url: tool.terms?.url ?? null, acceptedAt: new Date().toISOString() }
  writeJson(paths.terms, all)
}

// ── process helpers ──────────────────────────────────────────────────────────

export function run(cmd, args, { env, cwd, timeoutMs = 15 * 60_000, onLine } = {}) {
  return new Promise((resolve) => {
    let out = ""
    let err = ""
    let done = false
    let child
    try {
      child = spawn(cmd, args, { env, cwd, stdio: ["ignore", "pipe", "pipe"] })
    } catch (e) {
      resolve({ code: -1, stdout: "", stderr: String(e?.message || e) })
      return
    }
    const timer = setTimeout(() => {
      if (!done) child.kill("SIGKILL")
      err += `\n[allternit-tools] timed out after ${timeoutMs}ms`
    }, timeoutMs)
    const feed = (chunk, which) => {
      const s = chunk.toString()
      if (which === "out") out += s
      else err += s
      if (onLine) for (const l of s.split(/\r?\n/)) if (l.trim()) onLine(l.slice(0, 400))
      if (out.length > 1_000_000) out = out.slice(-500_000)
      if (err.length > 1_000_000) err = err.slice(-500_000)
    }
    child.stdout.on("data", (c) => feed(c, "out"))
    child.stderr.on("data", (c) => feed(c, "err"))
    child.on("error", (e) => {
      done = true
      clearTimeout(timer)
      resolve({ code: -1, stdout: out, stderr: err + String(e?.message || e) })
    })
    child.on("close", (code) => {
      done = true
      clearTimeout(timer)
      resolve({ code: code ?? -1, stdout: out, stderr: err })
    })
  })
}

export async function withRetries(fn, { attempts = 3, delayMs = 1500, onRetry } = {}) {
  let last
  for (let i = 1; i <= attempts; i++) {
    try {
      return await fn(i)
    } catch (e) {
      last = e
      if (i < attempts) {
        onRetry?.(i, e)
        await new Promise((r) => setTimeout(r, delayMs * i))
      }
    }
  }
  throw last
}

export async function download(url, dest, { attempts = 3, onRetry } = {}) {
  return withRetries(
    async () => {
      const res = await fetch(url, { redirect: "follow" })
      if (!res.ok) throw new Error(`GET ${url} -> HTTP ${res.status}`)
      const buf = Buffer.from(await res.arrayBuffer())
      fs.mkdirSync(path.dirname(dest), { recursive: true })
      fs.writeFileSync(dest, buf)
      return crypto.createHash("sha256").update(buf).digest("hex")
    },
    { attempts, onRetry },
  )
}

async function fetchText(url, attempts = 3) {
  return withRetries(async () => {
    const res = await fetch(url, { redirect: "follow" })
    if (!res.ok) throw new Error(`GET ${url} -> HTTP ${res.status}`)
    return res.text()
  }, { attempts })
}

/** sha256 from either a bare `<hex>` file or a SHASUMS256.txt listing. */
export function pickSha(text, fileName) {
  const lines = String(text).trim().split(/\r?\n/)
  if (lines.length === 1 && /^[0-9a-f]{64}(\s|$)/i.test(lines[0])) return lines[0].slice(0, 64).toLowerCase()
  for (const l of lines) {
    const [hex, name] = l.trim().split(/\s+/)
    if (name && name.replace(/^\*/, "") === fileName) return hex.toLowerCase()
  }
  return null
}

function which(bin, envPath) {
  if (bin.includes("/")) return isExec(bin) ? bin : null
  for (const d of envPath.split(path.delimiter)) {
    if (!d) continue
    const p = path.join(d, bin)
    if (isExec(p)) return p
  }
  return null
}

function isExec(p) {
  try {
    const st = fs.statSync(p)
    return st.isFile() && (st.mode & 0o111) !== 0
  } catch {
    return false
  }
}

function linkInto(binDir, name, target) {
  fs.mkdirSync(binDir, { recursive: true })
  const link = path.join(binDir, name)
  if (path.resolve(link) === path.resolve(target)) return link
  try {
    fs.rmSync(link, { force: true })
  } catch {}
  fs.symlinkSync(target, link)
  return link
}

/** Wrapper instead of a symlink when the tool needs env defaults (e.g. DSH_HOME). */
function writeShim(binDir, name, target, shimEnv) {
  fs.mkdirSync(binDir, { recursive: true })
  const file = path.join(binDir, name)
  fs.rmSync(file, { force: true })
  const lines = Object.entries(shimEnv).map(([k, v]) => `export ${k}="\${${k}:-${v}}"`)
  fs.writeFileSync(file, `#!/bin/sh\n${lines.join("\n")}\nexec '${target.replace(/'/g, "'\\''")}' "$@"\n`, { mode: 0o755 })
  return file
}

// ── lock (Desktop autostart and the API route can race) ──────────────────────

function acquireLock(paths, id) {
  fs.mkdirSync(paths.locks, { recursive: true })
  const file = path.join(paths.locks, `${id}.lock`)
  for (let i = 0; i < 2; i++) {
    try {
      fs.writeFileSync(file, String(process.pid), { flag: "wx" })
      return () => fs.rmSync(file, { force: true })
    } catch {
      const pid = Number(readJson(file, NaN)) || Number(fs.readFileSync(file, "utf8"))
      let alive = false
      try {
        process.kill(pid, 0)
        alive = true
      } catch {}
      if (alive && pid !== process.pid) return null
      fs.rmSync(file, { force: true })
    }
  }
  return null
}

// ── verify ───────────────────────────────────────────────────────────────────

export function locateBin(tool, ctx) {
  const spec = installSpec(tool, ctx.plat.os) || {}
  const own = path.join(ctx.paths.bin, tool.bin)
  if (isExec(own)) return own
  const onPath = which(tool.bin, ctx.envPath)
  if (onPath) return onPath
  for (const c of spec.binCandidates || []) {
    const p = expandHome(fill(c, templateVars(spec, ctx)), ctx.home)
    if (isExec(p)) return p
  }
  return null
}

export async function verifyTool(tool, ctx) {
  const bin = locateBin(tool, ctx)
  if (!bin) return { ok: false, reason: "not_found" }
  const argv = tool.verify?.argv || ["--version"]
  const r = await run(bin, argv, { env: ctx.env, timeoutMs: 60_000 })
  const text = `${r.stdout}\n${r.stderr}`.trim()
  if (r.code !== 0) return { ok: false, bin, reason: `exit ${r.code}`, output: text.slice(-400) }
  if (tool.verify?.expect && !new RegExp(tool.verify.expect, "im").test(text)) {
    return { ok: false, bin, reason: "unexpected_output", output: text.slice(-400) }
  }
  const version = (text.match(/\d+\.\d+(?:\.\d+)?(?:[-.\w]*)?/) || [null])[0]
  return { ok: true, bin, version, output: text.split("\n")[0].slice(0, 200) }
}

// ── install methods ──────────────────────────────────────────────────────────

class InstallError extends Error {
  constructor(code, message, detail) {
    super(message)
    this.code = code
    this.detail = detail
  }
}

async function mustRun(ctx, tool, cmd, args, opts = {}) {
  ctx.emit({ type: "step", tool: tool.id, step: "exec", command: [cmd, ...args].join(" ") })
  const r = await run(cmd, args, {
    env: { ...ctx.env, ...(opts.env || {}) },
    cwd: opts.cwd,
    timeoutMs: opts.timeoutMs,
    onLine: (line) => ctx.emit({ type: "log", tool: tool.id, line }),
  })
  if (r.code !== 0) {
    throw new InstallError(opts.code || "exec_failed", `${path.basename(cmd)} exited ${r.code}`, (r.stderr || r.stdout).slice(-1500))
  }
  return r
}

async function ensureDep(depId, ctx, tool) {
  const dep = resolveTool(ctx.manifest, depId)
  const res = await installOne(dep, ctx, { dependency: true })
  if (!res.ok) throw new InstallError("prerequisite_failed", `${tool.id} needs ${depId}: ${res.error || res.reason}`)
}

function npmBin(ctx) {
  const own = path.join(ctx.paths.bin, "npm")
  return isExec(own) ? own : which("npm", ctx.envPath) || "npm"
}

const METHODS = {
  async npm(tool, spec, ctx) {
    await ensureDep("node", ctx, tool)
    const args = ["install", "-g", "--prefix", ctx.paths.prefix, "--no-fund", "--no-audit", "--loglevel=error", ...spec.packages]
    await withRetries(() => mustRun(ctx, tool, npmBin(ctx), args, { env: { npm_config_yes: "true" }, code: "npm_failed" }), {
      onRetry: (i, e) => ctx.emit({ type: "retry", tool: tool.id, attempt: i, error: e.message }),
    })
    if (spec.postInstall) {
      const [cmd, ...rest] = spec.postInstall.map((a) => fill(a, templateVars(spec, ctx)))
      await withRetries(() => mustRun(ctx, tool, cmd, rest, { code: "post_install_failed" }), {
        onRetry: (i, e) => ctx.emit({ type: "retry", tool: tool.id, attempt: i, error: e.message }),
      })
    }
  },

  async "pip-venv"(tool, spec, ctx) {
    await ensureDep("uv", ctx, tool)
    const uv = locateBin(resolveTool(ctx.manifest, "uv"), ctx) || "uv"
    const venv = path.join(ctx.paths.venvs, tool.id)
    const env = { UV_PYTHON_INSTALL_DIR: path.join(ctx.paths.prefix, "python"), UV_CACHE_DIR: path.join(ctx.paths.prefix, "cache", "uv") }
    await mustRun(ctx, tool, uv, ["venv", "--clear", "--python", spec.python || "3.12", venv], { env, code: "venv_failed" })
    const py = path.join(venv, "bin", "python")
    await withRetries(() => mustRun(ctx, tool, uv, ["pip", "install", "--python", py, ...spec.packages], { env, code: "pip_failed" }), {
      onRetry: (i, e) => ctx.emit({ type: "retry", tool: tool.id, attempt: i, error: e.message }),
    })
    for (const b of spec.bins || [tool.bin]) {
      const target = path.join(venv, "bin", b)
      if (!isExec(target)) throw new InstallError("bin_missing", `${b} not found in venv after install`)
      if (spec.shimEnv) writeShim(ctx.paths.bin, b, target, spec.shimEnv)
      else linkInto(ctx.paths.bin, b, target)
    }
  },

  async binary(tool, spec, ctx) {
    const vars = templateVars(spec, ctx)
    const url = fill(spec.url, vars)
    const dir = path.join(ctx.paths.opt, tool.id, spec.version)
    const dest = path.join(dir, tool.bin)
    ctx.emit({ type: "step", tool: tool.id, step: "download", url })
    const sha = await download(url, dest, { onRetry: (i, e) => ctx.emit({ type: "retry", tool: tool.id, attempt: i, error: e.message }) })
    if (spec.sha256Url) {
      const want = pickSha(await fetchText(fill(spec.sha256Url, vars)), path.basename(url))
      if (!want || want !== sha) throw new InstallError("checksum_mismatch", `sha256 mismatch for ${url}`, `want ${want} got ${sha}`)
      ctx.emit({ type: "step", tool: tool.id, step: "checksum_ok", sha256: sha })
    }
    fs.chmodSync(dest, 0o755)
    linkInto(ctx.paths.bin, tool.bin, dest)
  },

  async archive(tool, spec, ctx) {
    const vars = templateVars(spec, ctx)
    const url = fill(spec.url, vars)
    const dir = path.join(ctx.paths.opt, tool.id, spec.version)
    const file = path.join(ctx.paths.opt, tool.id, path.basename(url))
    ctx.emit({ type: "step", tool: tool.id, step: "download", url })
    const sha = await download(url, file, { onRetry: (i, e) => ctx.emit({ type: "retry", tool: tool.id, attempt: i, error: e.message }) })
    if (spec.sha256Url) {
      const want = pickSha(await fetchText(fill(spec.sha256Url, vars)), path.basename(url))
      if (!want || want !== sha) throw new InstallError("checksum_mismatch", `sha256 mismatch for ${url}`, `want ${want} got ${sha}`)
      ctx.emit({ type: "step", tool: tool.id, step: "checksum_ok", sha256: sha })
    }
    fs.rmSync(dir, { recursive: true, force: true })
    fs.mkdirSync(dir, { recursive: true })
    const tarArgs = ["-xf", file, "-C", dir]
    if (spec.strip) tarArgs.push(`--strip-components=${spec.strip}`)
    await mustRun(ctx, tool, "tar", tarArgs, { code: "extract_failed" })
    fs.rmSync(file, { force: true })
    for (const [name, rel] of Object.entries(spec.links || { [tool.bin]: tool.bin })) {
      const target = path.join(dir, rel)
      if (!fs.existsSync(target)) throw new InstallError("bin_missing", `${rel} not in ${path.basename(url)}`)
      linkInto(ctx.paths.bin, name, target)
    }
  },

  async script(tool, spec, ctx) {
    const vars = templateVars(spec, ctx)
    const url = fill(spec.url, vars)
    const file = path.join(ctx.paths.prefix, "cache", "scripts", `${tool.id}.sh`)
    ctx.emit({ type: "step", tool: tool.id, step: "download", url })
    await download(url, file, { onRetry: (i, e) => ctx.emit({ type: "retry", tool: tool.id, attempt: i, error: e.message }) })
    const env = { CI: "1", NONINTERACTIVE: "1", DEBIAN_FRONTEND: "noninteractive" }
    for (const [k, v] of Object.entries(spec.env || {})) env[k] = fill(v, vars)
    const args = (spec.args || []).map((a) => fill(a, vars))
    fs.mkdirSync(ctx.paths.bin, { recursive: true })
    await withRetries(() => mustRun(ctx, tool, spec.shell || "sh", [file, ...args], { env, code: "script_failed", timeoutMs: 20 * 60_000 }), {
      onRetry: (i, e) => ctx.emit({ type: "retry", tool: tool.id, attempt: i, error: e.message }),
    })
    const own = path.join(ctx.paths.bin, tool.bin)
    if (!isExec(own)) {
      const found = (spec.binCandidates || []).map((c) => expandHome(fill(c, vars), ctx.home)).find(isExec)
      if (!found) throw new InstallError("bin_missing", `install script finished but ${tool.bin} was not found`, (spec.binCandidates || []).join(", "))
      linkInto(ctx.paths.bin, tool.bin, found)
    }
  },

  async system(tool, spec, ctx) {
    const isRoot = typeof process.getuid === "function" && process.getuid() === 0
    if (ctx.plat.os === "linux" && (spec.apt || spec.aptDeb)) {
      const apt = which("apt-get", ctx.envPath)
      if (!isRoot || !apt) {
        const cmd = spec.aptDeb ? `apt-get install -y ${spec.aptDeb}` : `apt-get install -y ${spec.apt.join(" ")}`
        throw new InstallError("needs_system_package", `${tool.name} is an OS package; run as root in the image build: ${cmd}`)
      }
      const env = { DEBIAN_FRONTEND: "noninteractive" }
      await withRetries(() => mustRun(ctx, tool, apt, ["update", "-y"], { env }), {})
      const pkgs = [...(spec.apt || [])]
      if (spec.aptDeb) {
        const deb = path.join(os.tmpdir(), `${tool.id}.deb`)
        await download(spec.aptDeb, deb)
        pkgs.push(deb)
      }
      await withRetries(() => mustRun(ctx, tool, apt, ["install", "-y", "--no-install-recommends", ...pkgs], { env, code: "apt_failed" }), {})
      for (const [from, to] of Object.entries(spec.aptBinAlias || {})) {
        const t = which(from, ctx.envPath)
        if (t) linkInto(ctx.paths.bin, to, t)
      }
      return
    }
    if (ctx.plat.os === "darwin") {
      const pkgs = spec.brew || []
      const casks = spec.brewCask || []
      if (!pkgs.length && !casks.length) throw new InstallError("needs_system_package", `${tool.name} ships with macOS developer tools (xcode-select --install)`)
      const brew = which("brew", ctx.envPath)
      if (!brew) throw new InstallError("needs_system_package", `${tool.name} needs Homebrew on this Mac: brew install ${[...pkgs, ...casks].join(" ")}`)
      const env = { HOMEBREW_NO_AUTO_UPDATE: "1", HOMEBREW_NO_INSTALL_CLEANUP: "1", NONINTERACTIVE: "1" }
      if (pkgs.length) await mustRun(ctx, tool, brew, ["install", ...pkgs], { env, code: "brew_failed" })
      if (casks.length) await mustRun(ctx, tool, brew, ["install", "--cask", ...casks], { env, code: "brew_failed" })
      return
    }
    throw new InstallError("unsupported_platform", `${tool.name}: no system package for ${ctx.plat.os}`)
  },

  async unsupported(tool, spec) {
    throw new InstallError("no_installer", spec.reason || `${tool.name} has no installer yet`)
  },
}

// ── wiring (MCP / skills / rules) ────────────────────────────────────────────

/** Wiring is ao's job (harness.json + `ao harness sync`); without ao we merge the MCP json entry ourselves when the source server exists. */
export async function wireTool(tool, ctx) {
  const harnessCfg = tool.harness && ctx.harness?.tools?.[tool.harness]
  const wire = harnessCfg || tool.wire
  if (!wire) return { wired: false, reason: "nothing_to_wire" }
  if (ctx.noWire) return { wired: false, reason: "disabled" }
  const ao = which("ao", ctx.envPath)
  if (ao && tool.harness) {
    const r = await run(ao, ["harness", "sync", `--tools=${tool.harness}`], { env: ctx.env, timeoutMs: 120_000 })
    return r.code === 0 ? { wired: true, via: "ao" } : { wired: false, reason: `ao harness sync exited ${r.code}` }
  }
  const src = ctx.harness?.source?.mcpServer
  const srcScript = src?.args?.[0] && expandHome(src.args[0], ctx.home)
  if (!src || !srcScript || !fs.existsSync(srcScript)) return { wired: false, reason: "mcp_source_absent" }
  const mcp = wire.mcp
  if (!mcp || mcp.kind !== "json") return { wired: false, reason: `mcp_kind_${mcp?.kind ?? "none"}_needs_ao` }
  const file = expandHome(mcp.path, ctx.home)
  let doc = {}
  if (fs.existsSync(file)) {
    doc = readJson(file, null)
    // JSONC or a broken file: never rewrite what we cannot parse.
    if (!doc || typeof doc !== "object" || Array.isArray(doc)) return { wired: false, reason: "config_unparseable", file }
  }
  const key = mcp.serversKey || "mcpServers"
  doc[key] = doc[key] || {}
  const args = src.args.map((a) => expandHome(a, ctx.home))
  doc[key][src.name] = mcp.commandArray ? { type: "local", command: [src.command, ...args] } : { command: src.command, args }
  if (!ctx.dryRun) writeJson(file, doc)
  return { wired: true, via: "json", file }
}

// ── install one ──────────────────────────────────────────────────────────────

function readState(paths) {
  return readJson(paths.state, { tools: {} })
}

function writeToolState(paths, id, value) {
  const s = readState(paths)
  s.tools = s.tools || {}
  if (value === null) delete s.tools[id]
  else s.tools[id] = value
  writeJson(paths.state, s)
}

export async function installOne(tool, ctx, { dependency = false } = {}) {
  if (!tool) return { ok: false, error: "unknown_tool" }
  if (ctx.done.has(tool.id)) return ctx.done.get(tool.id)
  const spec = installSpec(tool, ctx.plat.os)
  const finish = (r) => {
    ctx.done.set(tool.id, r)
    return r
  }
  if (tool.platforms && !tool.platforms.includes(`${ctx.plat.os}-${ctx.plat.arch}`)) {
    ctx.emit({ type: "skipped", tool: tool.id, reason: "platform", platforms: tool.platforms })
    return finish({ ok: false, skipped: true, reason: "platform" })
  }
  if (!spec) {
    ctx.emit({ type: "skipped", tool: tool.id, reason: "platform" })
    return finish({ ok: false, skipped: true, reason: "platform" })
  }
  ctx.emit({ type: "start", tool: tool.id, name: tool.name, method: spec.method, version: spec.version ?? null, dependency })

  // Prerequisites that may already be satisfied by the host (system npm/uv).
  if (tool.skipIfOnPath && !ctx.force) {
    const p = which(tool.skipIfOnPath.bin, ctx.envPath)
    if (p && !p.startsWith(ctx.paths.bin + path.sep)) {
      let ok = true
      if (tool.skipIfOnPath.minNodeMajor) {
        const nodeP = which("node", ctx.envPath)
        const r = nodeP ? await run(nodeP, ["--version"], { env: ctx.env, timeoutMs: 20_000 }) : { code: 1, stdout: "" }
        const major = Number((r.stdout.match(/v(\d+)/) || [])[1] || 0)
        ok = r.code === 0 && major >= tool.skipIfOnPath.minNodeMajor
      }
      if (ok) {
        ctx.emit({ type: "skipped", tool: tool.id, reason: "host_provides", bin: p })
        return finish({ ok: true, skipped: true, reason: "host_provides" })
      }
    }
  }

  // Idempotent: installed at this pin and verifies -> nothing to do.
  const state = readState(ctx.paths).tools?.[tool.id]
  if (!ctx.force) {
    const v = await verifyTool(tool, ctx)
    const pinMatches = !spec.version || state?.version === spec.version || spec.method === "system" || spec.method === "unsupported"
    const external = v.ok && !v.bin.startsWith(ctx.paths.bin + path.sep)
    if (v.ok && (pinMatches || external)) {
      ctx.emit({ type: "skipped", tool: tool.id, reason: "already_installed", bin: v.bin, version: v.version })
      return finish({ ok: true, skipped: true, reason: "already_installed", bin: v.bin, version: v.version })
    }
  }

  if (tool.terms?.required) {
    if (ctx.acceptTerms.has(tool.id)) {
      if (!ctx.dryRun) recordTerms(ctx.paths, tool, spec)
    } else if (!termsAccepted(ctx.paths, tool, spec)) {
      ctx.emit({ type: "skipped", tool: tool.id, reason: "terms_required", url: tool.terms.url ?? null })
      return finish({ ok: false, skipped: true, reason: "terms_required", url: tool.terms.url ?? null })
    }
  }

  if (ctx.dryRun) {
    ctx.emit({ type: "planned", tool: tool.id, method: spec.method, version: spec.version ?? null })
    return finish({ ok: true, planned: true })
  }

  const release = acquireLock(ctx.paths, tool.id)
  if (!release) {
    ctx.emit({ type: "error", tool: tool.id, code: "busy", message: "another install of this tool is running" })
    return finish({ ok: false, error: "busy" })
  }
  try {
    const method = METHODS[spec.method]
    if (!method) throw new InstallError("unknown_method", `unknown install method ${spec.method}`)
    await method(tool, spec, ctx)
    const v = await verifyTool(tool, ctx)
    if (!v.ok) throw new InstallError("verify_failed", `${tool.bin} installed but did not verify (${v.reason})`, v.output)
    writeToolState(ctx.paths, tool.id, { version: spec.version ?? v.version, method: spec.method, bin: v.bin, verifiedAt: new Date().toISOString(), reported: v.output })
    ctx.emit({ type: "verified", tool: tool.id, bin: v.bin, version: v.version, output: v.output })
    const w = await wireTool(tool, ctx).catch((e) => ({ wired: false, reason: String(e?.message || e) }))
    ctx.emit({ type: w.wired ? "wired" : "wire_skipped", tool: tool.id, ...w })
    return finish({ ok: true, bin: v.bin, version: v.version })
  } catch (e) {
    const code = e instanceof InstallError ? e.code : "internal"
    ctx.emit({ type: "error", tool: tool.id, code, message: e.message, detail: e.detail ?? null })
    return finish({ ok: false, error: code, message: e.message })
  } finally {
    release()
  }
}

// ── launcher: `allternit-tools` on the tools PATH (API + shells find it) ─────

export function writeLauncher(paths, { nodePath = process.execPath, script = fileURLToPath(import.meta.url) } = {}) {
  const q = (s) => `'${String(s).replace(/'/g, "'\\''")}'`
  const body = `#!/bin/sh\n# written by allternit-tools; points at the installer that last ran\nELECTRON_RUN_AS_NODE=1 exec ${q(nodePath)} ${q(script)} "$@"\n`
  fs.mkdirSync(paths.bin, { recursive: true })
  const file = path.join(paths.bin, "allternit-tools")
  let current = null
  try {
    current = fs.readFileSync(file, "utf8")
  } catch {}
  if (current !== body) {
    fs.writeFileSync(file, body, { mode: 0o755 })
    fs.chmodSync(file, 0o755)
  }
  fs.writeFileSync(path.join(paths.prefix, "env.sh"), `export PATH="${paths.bin}:$PATH"\n`)
  return file
}

// ── context / commands ───────────────────────────────────────────────────────

export function makeContext({ prefix = defaultPrefix(), manifest = loadManifest(), emit = () => {}, acceptTerms = [], dryRun = false, force = false, noWire = false, env = process.env } = {}) {
  const home = env.HOME || os.homedir()
  const paths = layout(prefix)
  const envPath = toolsPath(prefix, env.PATH || "", home)
  const childEnv = { ...env, PATH: envPath, ALLTERNIT_TOOLS_PREFIX: prefix }
  delete childEnv.ELECTRON_RUN_AS_NODE
  return {
    manifest,
    harness: loadHarness(manifest, env),
    paths,
    home,
    plat: platformInfo(),
    envPath,
    env: childEnv,
    emit,
    acceptTerms: new Set(acceptTerms),
    dryRun,
    force,
    noWire: noWire || env.ALLTERNIT_TOOLS_NO_WIRE === "1",
    done: new Map(),
  }
}

export function defaultAllIds(manifest) {
  return manifest.tools
    .filter((t) => t.category !== "base")
    .filter((t) => Object.values(t.install || {}).some((s) => s.method !== "unsupported"))
    .map((t) => t.id)
}

export async function statusOf(tool, ctx) {
  const spec = installSpec(tool, ctx.plat.os)
  const v = await verifyTool(tool, ctx)
  const state = readState(ctx.paths).tools?.[tool.id] || null
  return {
    id: tool.id,
    name: tool.name,
    command: tool.bin,
    category: tool.category,
    aliases: tool.aliases || [],
    providerId: tool.providerId ?? null,
    installed: v.ok,
    bin: v.bin ?? null,
    version: v.version ?? null,
    pinned: spec?.version ?? null,
    method: spec?.method ?? null,
    managed: !!(v.bin && v.bin.startsWith(ctx.paths.bin + path.sep)),
    installable: !!spec && spec.method !== "unsupported",
    reason: v.ok ? null : spec?.method === "unsupported" ? "no_installer" : v.reason,
    termsRequired: !!tool.terms?.required,
    termsAccepted: termsAccepted(ctx.paths, tool, spec),
    termsUrl: tool.terms?.url ?? null,
    login: tool.login ?? null,
    state,
  }
}

function parseArgs(argv) {
  const out = { _: [], only: [], acceptTerms: [] }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    const take = () => {
      const eq = a.indexOf("=")
      return eq > 0 ? a.slice(eq + 1) : argv[++i]
    }
    const list = (v) => String(v || "").split(",").map((s) => s.trim()).filter(Boolean)
    if (a === "--all") out.all = true
    else if (a === "--selected") out.selected = true
    else if (a === "--json") out.json = true
    else if (a === "--dry-run") out.dryRun = true
    else if (a === "--force") out.force = true
    else if (a === "--no-wire") out.noWire = true
    else if (a === "--select") out.select = true
    else if (a === "--include-base") out.includeBase = true
    else if (a === "--show") out.show = true
    else if (a.startsWith("--only")) out.only.push(...list(take()))
    else if (a.startsWith("--accept-terms")) out.acceptTerms.push(...list(take()))
    else if (a.startsWith("--prefix")) out.prefix = take()
    else if (a.startsWith("--set")) out.set = list(take())
    else if (a.startsWith("--add")) out.add = list(take())
    else if (a.startsWith("--remove")) out.remove = list(take())
    else out._.push(a)
  }
  return out
}

function emitter(json) {
  return (ev) => {
    const e = { ts: new Date().toISOString(), ...ev }
    if (json) process.stdout.write(JSON.stringify(e) + "\n")
    else if (ev.type === "log") process.stderr.write(`  ${ev.tool}: ${ev.line}\n`)
    else process.stderr.write(`[${ev.type}] ${ev.tool ?? ""} ${ev.message ?? ev.reason ?? ev.step ?? ev.version ?? ""}\n`)
  }
}

export async function main(argv = process.argv.slice(2)) {
  const args = parseArgs(argv)
  const cmd = args._[0] || "help"
  const manifest = loadManifest()
  const prefix = path.resolve(expandHome(args.prefix || defaultPrefix()))
  const emit = emitter(args.json)
  const resolveIds = (ids) => {
    const tools = []
    for (const id of ids) {
      const t = resolveTool(manifest, id)
      if (!t) throw new Error(`unknown tool: ${id}`)
      tools.push(t)
    }
    return tools
  }
  const acceptIds = args.acceptTerms.map((id) => resolveTool(manifest, id)?.id).filter(Boolean)
  const ctx = makeContext({ prefix, manifest, emit, acceptTerms: acceptIds, dryRun: args.dryRun, force: args.force, noWire: args.noWire })

  if (cmd !== "help" && !args.dryRun) {
    try {
      writeLauncher(ctx.paths)
    } catch (e) {
      emit({ type: "warning", message: `could not write launcher: ${e?.message || e}` })
    }
  }

  if (cmd === "install") {
    let ids = args.only
    if (args.all) ids = [...defaultAllIds(manifest), ...(args.includeBase ? manifest.tools.filter((t) => t.category === "base").map((t) => t.id) : [])]
    else if (args.selected) ids = readSelection(ctx.paths)
    if (!ids.length) {
      emit({ type: "done", ok: true, installed: [], failed: [], skipped: [], message: "nothing selected" })
      return 0
    }
    const tools = resolveIds(ids)
    if (args.select && !args.dryRun) writeSelection(ctx.paths, [...readSelection(ctx.paths), ...tools.map((t) => t.id)])
    const results = {}
    for (const t of tools) results[t.id] = await installOne(t, ctx)
    const failed = Object.entries(results).filter(([, r]) => !r.ok && !r.skipped).map(([id]) => id)
    const skipped = Object.entries(results).filter(([, r]) => r.skipped && !r.ok).map(([id, r]) => ({ id, reason: r.reason }))
    const installed = Object.entries(results).filter(([, r]) => r.ok).map(([id]) => id)
    emit({ type: "done", ok: failed.length === 0, installed, failed, skipped, path: ctx.paths.bin })
    return failed.length ? 1 : 0
  }
  if (cmd === "status" || cmd === "list") {
    const tools = args.only.length ? resolveIds(args.only) : manifest.tools
    const rows = []
    for (const t of tools) rows.push(await statusOf(t, ctx))
    const out = { prefix, bin: ctx.paths.bin, platform: ctx.plat, selected: readSelection(ctx.paths), tools: rows }
    if (args.json) process.stdout.write(JSON.stringify(out) + "\n")
    else for (const r of rows) process.stdout.write(`${r.installed ? "ok " : "-- "} ${r.id.padEnd(20)} ${r.version ?? ""} ${r.installed ? "" : `(${r.reason})`}\n`)
    return 0
  }
  if (cmd === "select") {
    let sel = readSelection(ctx.paths)
    if (args.set) sel = resolveIds(args.set).map((t) => t.id)
    if (args.add) sel = [...sel, ...resolveIds(args.add).map((t) => t.id)]
    if (args.remove) {
      const rm = new Set(resolveIds(args.remove).map((t) => t.id))
      sel = sel.filter((id) => !rm.has(id))
    }
    if (args.set || args.add || args.remove) writeSelection(ctx.paths, sel)
    process.stdout.write(JSON.stringify({ selected: readSelection(ctx.paths) }) + "\n")
    return 0
  }
  if (cmd === "uninstall") {
    for (const t of resolveIds(args.only)) {
      const spec = installSpec(t, ctx.plat.os)
      if (spec?.method === "npm") {
        const names = spec.packages.map((p) => p.replace(/@[^@/]+$/, ""))
        await run(npmBin(ctx), ["uninstall", "-g", "--prefix", prefix, ...names], { env: ctx.env })
      }
      fs.rmSync(path.join(ctx.paths.bin, t.bin), { force: true })
      fs.rmSync(path.join(ctx.paths.venvs, t.id), { recursive: true, force: true })
      fs.rmSync(path.join(ctx.paths.opt, t.id), { recursive: true, force: true })
      writeToolState(ctx.paths, t.id, null)
      emit({ type: "uninstalled", tool: t.id })
    }
    return 0
  }
  if (cmd === "env") {
    process.stdout.write(`export PATH="${ctx.paths.bin}:$PATH"\n`)
    return 0
  }
  process.stdout.write(
    "usage: allternit-tools install (--all|--only a,b|--selected) [--accept-terms a,b] [--select] [--json] [--prefix DIR] [--dry-run] [--force]\n" +
      "       allternit-tools status|list [--only a,b] [--json]\n" +
      "       allternit-tools select --set|--add|--remove a,b\n" +
      "       allternit-tools uninstall --only a,b\n" +
      "       allternit-tools env\n",
  )
  return cmd === "help" ? 0 : 2
}

const invokedDirectly = (() => {
  try {
    return process.argv[1] && fs.realpathSync(process.argv[1]) === fs.realpathSync(fileURLToPath(import.meta.url))
  } catch {
    return false
  }
})()

if (invokedDirectly) {
  main().then(
    (code) => process.exit(code),
    (e) => {
      process.stderr.write(`allternit-tools: ${e?.message || e}\n`)
      process.exit(2)
    },
  )
}
