/**
 * Subprocess Provider Discovery
 *
 * Scans PATH for known LLM CLI tools. For each one found, creates a provider
 * entry with auth_type: "subprocess". The user gets it in /model with zero
 * configuration — just having the CLI installed and logged in is enough.
 *
 * Adding a new CLI:
 *   Add it to cli-catalog.json (next to this file) — the one list allternit-api
 *   also reads (Settings → Connections → Coding tools, install, sign-in). A CLI
 *   that answers a one-shot prompt needs only `adapter: "text"` + `headless`;
 *   one that speaks a streaming protocol gets an adapter in local-cli-driver.ts.
 *   Tools a user adds themselves arrive through customCliFile().
 */

import { which } from "bun"
import { execFile } from "node:child_process"
import { promisify } from "node:util"
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import type { DiscoveredProvider, DiscoveredModel } from "./index"
import catalog from "./cli-catalog.json"

const execFileAsync = promisify(execFile)

/**
 * The Factory harness's managed bin (Allternit's one-click Install puts CLIs
 * and, on machines without npm, a private Node there). Mirrors
 * `harness_root()` in factory/engine/src/agents/registry.rs.
 */
export function harnessBinDir(env: NodeJS.ProcessEnv = process.env, home = env.HOME || os.homedir()): string {
  if (env.ALLTERNIT_FACTORY_HARNESS_HOME) return path.join(env.ALLTERNIT_FACTORY_HARNESS_HOME, "bin")
  const factoryHome = env.ALLTERNIT_FACTORY_HOME || path.join(home, ".allternit", "factory")
  const current = path.join(factoryHome, "harness")
  const legacy = path.join(home, ".ao", "harness")
  return path.join(!existsSync(current) && existsSync(legacy) ? legacy : current, "bin")
}

/** Put the harness bin first on PATH (idempotent) so installed CLIs, and
 *  the Node their npm shims run on, resolve here and in spawned CLIs. */
export function prependHarnessBin(env: NodeJS.ProcessEnv = process.env): void {
  const bin = harnessBinDir(env)
  const parts = (env.PATH ?? "").split(path.delimiter).filter(Boolean)
  if (parts.includes(bin)) return
  env.PATH = [bin, ...parts].join(path.delimiter)
}

/** An Allternit cloud computer is a per-user container whose desktop runs as
 *  root, and Claude Code refuses `--dangerously-skip-permissions` as root
 *  unless `IS_SANDBOX=1`. Mark it so spawned CLIs inherit it. Only on cloud
 *  computers (the image sets ALLTERNIT_PROVISIONED=1), never a user's own
 *  machine, and an explicit IS_SANDBOX wins. */
export function markCloudComputerSandbox(
  env: NodeJS.ProcessEnv = process.env,
  uid: number | undefined = process.getuid?.(),
): void {
  if (env.ALLTERNIT_PROVISIONED !== "1" || uid !== 0 || env.IS_SANDBOX !== undefined) return
  env.IS_SANDBOX = "1"
}

prependHarnessBin()
markCloudComputerSandbox()

export function parseCodexModels(stdout: string): DiscoveredModel[] {
  const catalog = JSON.parse(stdout) as { models?: Array<{ slug?: string; display_name?: string; context_window?: number }> }
  return (catalog.models ?? []).filter((model) => model.slug).map((model) => ({
    id: model.slug!,
    name: model.display_name || model.slug!,
    context: model.context_window || 272000,
    output: 32768,
  }))
}

/** One entry of cli-catalog.json (the fields gizzi uses). */
export interface CatalogTool {
  id: string
  aliases?: string[]
  name: string
  bin: string
  icon?: string
  adapter: "builtin" | "text" | "unsupported"
  probe?: { args: string[]; expect: string }
  headless?: { args: string[]; modelArgs?: string[] }
  models?: DiscoveredModel[]
  envKeys?: { path: string; model?: string }
  /** Env var that points the CLI at its own config/sign-in dir (a second account); absent = single-account. */
  accountHomeEnv?: string
  custom?: boolean
}

export const CLI_CATALOG: CatalogTool[] = (catalog as { tools: CatalogTool[] }).tools

/**
 * Per-provider environment overrides matching the Multica Go runtime.
 *
 * `MULTICA_<PROVIDER>_PATH` takes precedence over PATH lookup.
 * `MULTICA_<PROVIDER>_MODEL` is available for adapters that accept a --model flag.
 */
export const PROVIDER_ENV_KEYS: Record<string, { path: string; model?: string }> = Object.fromEntries(
  CLI_CATALOG.filter((tool) => tool.envKeys).map((tool) => [tool.id, tool.envKeys!]),
)

const SHELL_CACHE_TTL_MS = 30 * 60 * 1000

let shellCacheKey = ""
let shellCachePath: string | null = null
let shellCacheExpires = 0

function envCacheKey(name: string): string {
  return `${name}:${process.env.PATH ?? ""}:${process.env.SHELL ?? ""}:${process.env.HOME ?? ""}`
}

async function resolveViaLoginShell(name: string): Promise<string | null> {
  const key = envCacheKey(name)
  const now = Date.now()
  if (shellCacheKey === key && shellCachePath !== null && now < shellCacheExpires) {
    if (existsSync(shellCachePath)) return shellCachePath
  }

  const shell = process.env.SHELL || "/bin/sh"
  try {
    const { stdout } = await execFileAsync(shell, ["-l", "-c", `command -v ${name}`], {
      timeout: 5000,
      env: process.env,
    })
    const candidates = stdout.trim().split("\n").map((s) => s.trim()).filter(Boolean)
    const resolved = candidates[candidates.length - 1] ?? ""
    if (resolved && existsSync(resolved)) {
      shellCacheKey = key
      shellCachePath = resolved
      shellCacheExpires = now + SHELL_CACHE_TTL_MS
      return resolved
    }
  } catch {
    // fall through
  }
  return null
}

function isExecutable(filePath: string): boolean {
  try {
    const s = statSync(filePath)
    return s.isFile() && (s.mode & 0o111) !== 0
  } catch {
    return false
  }
}

function findInPath(name: string): string | null {
  const pathEnv = process.env.PATH ?? ""
  for (const dir of pathEnv.split(path.delimiter)) {
    if (!dir) continue
    const candidate = path.join(dir, name)
    if (isExecutable(candidate)) return candidate
  }
  return null
}

async function resolveBinPath(name: string): Promise<string | null> {
  // Walk PATH directly first so tests (and shell-specific PATH prefixes) can
  // override cached resolutions from Bun's which().
  const fromPath = findInPath(name)
  if (fromPath) return fromPath

  try {
    const found = which(name)
    if (found && existsSync(found)) return found
  } catch {
    // fall through
  }

  // Bare names are also resolved via the user's login shell so that tools
  // installed by shell-specific package managers (e.g. mise, mise-like shims)
  // are discoverable.
  if (!name.includes("/") && !name.includes("\\")) {
    const shellPath = await resolveViaLoginShell(name)
    if (shellPath) return shellPath
  }

  return null
}

/**
 * Resolve the absolute path for a subprocess provider spec.
 *
 * Resolution order:
 *   1. `MULTICA_<PROVIDER>_PATH` env override.
 *   2. Direct PATH walk + `bun:which`.
 *   3. Login-shell PATH fallback (cached 30 minutes).
 *   4. Known install locations (the CLI's own installer, user bin dirs). A
 *      desktop app launched from Finder/Dock gets a minimal PATH, and a CLI
 *      whose launcher link went missing is still installed.
 */
export async function resolveCliPath(spec: SubprocessSpec): Promise<string | null> {
  const env = PROVIDER_ENV_KEYS[spec.id]

  if (env?.path) {
    const override = process.env[env.path]
    if (override && existsSync(override)) return override
  }

  // A custom tool may name its program by absolute path.
  if (path.isAbsolute(spec.bin)) return isExecutable(spec.bin) ? spec.bin : null

  const fromPath = await resolveBinPath(spec.bin)
  if (fromPath) return fromPath

  for (const candidate of knownInstallPaths(spec)) {
    if (isExecutable(candidate)) return candidate
  }

  return null
}

/**
 * The CLI's path now. A path found at startup can vanish while gizzi runs
 * (Claude Code's auto-update removed ~/.local/bin/claude and every turn failed
 * with ENOENT posix_spawn); then look it up again the way discovery does.
 */
export async function currentCliPath(cliId: string, known: string): Promise<string> {
  if (!path.isAbsolute(known) || existsSync(known)) return known
  const spec = findSubprocessSpec(cliId)
  if (!spec) return known
  return (await resolveCliPath(spec)) ?? known
}

/** Directories user-level installers put CLIs in, whether or not they're on PATH. */
function userBinDirs(home: string): string[] {
  return [
    path.join(home, ".local", "bin"),
    path.join(home, ".bun", "bin"),
    path.join(home, ".npm-global", "bin"),
    path.join(home, ".cargo", "bin"),
    path.join(home, ".deno", "bin"),
    path.join(home, "bin"),
    "/opt/homebrew/bin",
    "/usr/local/bin",
  ]
}

/** Newest entry of a versions directory (e.g. Claude Code's ~/.local/share/claude/versions/2.1.283). */
function newestVersion(dir: string): string | null {
  let names: string[]
  try {
    names = readdirSync(dir)
  } catch {
    return null
  }
  const parse = (v: string) => v.split(/[.-]/).map((n) => Number.parseInt(n, 10) || 0)
  const sorted = names
    .filter((n) => /^\d+(\.\d+)*/.test(n))
    .sort((a, b) => {
      const pa = parse(a)
      const pb = parse(b)
      for (let i = 0; i < Math.max(pa.length, pb.length); i += 1) {
        const d = (pb[i] ?? 0) - (pa[i] ?? 0)
        if (d) return d
      }
      return 0
    })
  for (const name of sorted) {
    const full = path.join(dir, name)
    if (isExecutable(full)) return full
  }
  return null
}

/** Where each CLI's own installer puts it, checked in order after PATH. */
export function knownInstallPaths(spec: Pick<SubprocessSpec, "id" | "bin">, home = process.env.HOME || os.homedir()): string[] {
  if (!home || spec.bin.includes("/")) return []
  const candidates: string[] = []
  if (spec.id === "claude-cli") {
    // Claude Code's native installer: the launcher in ~/.local/bin points at
    // a versioned binary; the older local install lives in ~/.claude/local.
    candidates.push(path.join(home, ".local", "bin", "claude"), path.join(home, ".claude", "local", "claude"))
    const newest = newestVersion(path.join(home, ".local", "share", "claude", "versions"))
    if (newest) candidates.push(newest)
  }
  if (spec.id === "codex-cli" && process.platform === "darwin") {
    // Codex is commonly installed as a macOS desktop app with a bundled CLI.
    candidates.push(
      "/Applications/Codex.app/Contents/MacOS/Codex",
      path.join(home, "Applications", "Codex.app", "Contents", "MacOS", "Codex"),
    )
  }
  // Allternit's one-click Install (Factory harness managed bin).
  candidates.push(path.join(harnessBinDir({ ...process.env, HOME: home }, home), spec.bin))
  // CLIs whose installers use their own bin dir.
  const ownDir: Record<string, string> = { "kimi-cli": ".kimi-code", grok: ".grok", opencode: ".opencode" }
  if (ownDir[spec.id]) candidates.push(path.join(home, ownDir[spec.id], "bin", spec.bin))
  for (const dir of userBinDirs(home)) candidates.push(path.join(dir, spec.bin))
  return [...new Set(candidates)]
}

export interface SubprocessSpec {
  /** Binary name to look for in PATH (or, for a custom tool, an absolute path) */
  bin: string
  /** Provider ID in gizzi's model list */
  id: string
  name: string
  /** Icon asset key (maps to /icons/agent-clis/{icon}.svg) */
  icon?: string
  /** Display command (the binary); the argv a turn runs comes from the adapter */
  cmd: string
  /** builtin = protocol adapter in local-cli-driver.ts; text = one-shot `headless` argv */
  adapter: CatalogTool["adapter"]
  /** For `text`: argv after the binary — exactly one element is "{prompt}" */
  headless?: { args: string[]; modelArgs?: string[] }
  /** Known models surfaced by this CLI */
  models: DiscoveredModel[]
  /**
   * Optional probe — run this and check stdout to confirm auth is active.
   * If omitted, presence in PATH is treated as sufficient.
   */
  probe?: { args: string[]; expect: string | RegExp }
  /** Added by a user (Settings → Coding tools → Add a CLI) */
  custom?: boolean
}

function toSpec(tool: CatalogTool): SubprocessSpec {
  return {
    bin: tool.bin,
    id: tool.id,
    name: tool.name,
    icon: tool.icon ?? tool.id,
    cmd: tool.bin,
    adapter: tool.adapter,
    headless: tool.headless,
    models: tool.models ?? [],
    probe: tool.probe ? { args: tool.probe.args, expect: new RegExp(tool.probe.expect) } : undefined,
    custom: tool.custom,
  }
}

/** Every CLI in the shared catalog (cli-catalog.json). */
export const SUBPROCESS_PROVIDERS: SubprocessSpec[] = CLI_CATALOG.map(toSpec)

/** Where allternit-api mirrors the user's own CLIs (src/cli_catalog.rs `mirror_path`). */
export function customCliFile(env: NodeJS.ProcessEnv = process.env): string {
  return path.join(path.dirname(harnessBinDir(env)), "custom-clis.json")
}

const SHELL_META = /[;&|$`<>(){}[\]*?!~'"\\#\n\r\t\0]/

/**
 * A custom tool is only ever argv: a bare program name or absolute path, and
 * an argument list with exactly one "{prompt}" element. Anything else in the
 * file is dropped (allternit-api validates the same way before writing it).
 */
export function validCustomTool(tool: unknown): tool is CatalogTool {
  const t = tool as CatalogTool
  if (!t || typeof t.id !== "string" || !t.id.startsWith("custom-") || typeof t.name !== "string") return false
  if (typeof t.bin !== "string" || !t.bin || SHELL_META.test(t.bin) || t.bin.startsWith("-")) return false
  if (path.isAbsolute(t.bin) ? t.bin.split(/[\\/]/).includes("..") : !/^[A-Za-z0-9._+-]+$/.test(t.bin)) return false
  const args = t.headless?.args
  if (!Array.isArray(args) || !args.every((a) => typeof a === "string")) return false
  if (args.filter((a) => a === "{prompt}").length !== 1 || args.filter((a) => a.includes("{prompt}")).length !== 1) return false
  const modelArgs = t.headless?.modelArgs ?? []
  if (!Array.isArray(modelArgs) || !modelArgs.every((a) => typeof a === "string" && !a.includes("{prompt}"))) return false
  return true
}

/** The user-added CLIs on this computer (read fresh: they change while gizzi runs). */
export function customSubprocessSpecs(file = customCliFile()): SubprocessSpec[] {
  let parsed: { tools?: unknown[] }
  try {
    parsed = JSON.parse(readFileSync(file, "utf8"))
  } catch {
    return []
  }
  return (parsed.tools ?? []).filter(validCustomTool).map((tool) => {
    const versionArgs = Array.isArray((tool as any).verifyArgs) ? ((tool as any).verifyArgs as string[]) : []
    return toSpec({
      ...tool,
      adapter: "text",
      custom: true,
      models: [{ id: "default", name: `${tool.name} default`, context: 128000, output: 16384 }],
      // A version command, when given, must print something; else presence is enough.
      probe: versionArgs.length > 0 ? { args: versionArgs, expect: "\\S" } : undefined,
    })
  })
}

/** Catalog CLIs plus the user's own. */
export function subprocessSpecs(): SubprocessSpec[] {
  const builtIn = new Set(SUBPROCESS_PROVIDERS.map((s) => s.id))
  return [...SUBPROCESS_PROVIDERS, ...customSubprocessSpecs().filter((s) => !builtIn.has(s.id))]
}

export function findSubprocessSpec(id: string): SubprocessSpec | undefined {
  return SUBPROCESS_PROVIDERS.find((s) => s.id === id) ?? customSubprocessSpecs().find((s) => s.id === id)
}

const MODEL_ID = /^[A-Za-z0-9._:\/@+-]{1,200}$/

/**
 * The argv a `text` CLI runs for one prompt. No shell: the prompt is one argv
 * element (a leading "-" gets a space so it can't read as a flag), and
 * `{model}` only takes a plain model id.
 */
export function headlessArgv(spec: SubprocessSpec, command: string, prompt: string, model?: string): string[] {
  if (!spec.headless) throw new Error(`CLI "${spec.id}" has no headless invocation`)
  const args = spec.headless.args
  const modelInArgs = args.some((a) => a.includes("{model}"))
  const picked = model && MODEL_ID.test(model) && !model.startsWith("-") ? model : undefined
  const resolved = picked ?? (modelInArgs ? spec.models[0]?.id : undefined)
  if (modelInArgs && !resolved) throw new Error(`${spec.name} needs a model — pick one in the model menu`)
  const fill = (a: string) => (resolved ? a.split("{model}").join(resolved) : a)
  const modelArgs = picked && !modelInArgs ? (spec.headless.modelArgs ?? []).map(fill) : []
  const safePrompt = prompt.startsWith("-") ? ` ${prompt}` : prompt
  return [command, ...modelArgs, ...args.map((a) => (a === "{prompt}" ? safePrompt : fill(a)))]
}

/**
 * Max time a probe subprocess may run before we kill it and treat it as unavailable.
 * Bun's Subprocess.kill() does not reliably close piped stdout (spawned children can
 * inherit the fd), so we race the read against a timeout instead of relying on the
 * kill alone to unblock it.
 */
const PROBE_TIMEOUT_MS = 5000

async function readWithTimeout(stdout: ReadableStream, kill: () => void): Promise<string | null> {
  let timer: ReturnType<typeof setTimeout>
  const timeout = new Promise<null>((resolve) => {
    timer = setTimeout(() => resolve(null), PROBE_TIMEOUT_MS)
  })
  const out = await Promise.race([new Response(stdout).text().catch(() => null), timeout])
  clearTimeout(timer!)
  if (out === null) kill()
  return out
}

async function runProbe(bin: string, spec: SubprocessSpec): Promise<boolean> {
  if (!spec.probe) return true // presence in PATH is enough
  try {
    const proc = Bun.spawn([bin, ...spec.probe.args], {
      stdout: "pipe",
      stderr: "pipe",
    })
    const out = await readWithTimeout(proc.stdout, () => {
      try {
        proc.kill(9)
      } catch {
        // already exited
      }
    })
    if (out === null) return false // timed out — treat as not available
    const { expect } = spec.probe
    return typeof expect === "string" ? out.includes(expect) : expect.test(out)
  } catch {
    return false
  }
}

async function probeOllamaModels(binPath: string): Promise<DiscoveredModel[]> {
  try {
    const proc = Bun.spawn([binPath, "list"], { stdout: "pipe", stderr: "pipe" })
    const out = await readWithTimeout(proc.stdout, () => {
      try {
        proc.kill(9)
      } catch {
        // already exited
      }
    })
    if (out === null) return [] // timed out — treat as not available
    const lines = out.split("\n").slice(1).filter(Boolean)
    return lines.map((line) => {
      const [id] = line.trim().split(/\s+/)
      return { id, name: id, context: 128000, output: 8192 }
    })
  } catch {
    return []
  }
}

export async function discoverSubprocessProviders(): Promise<DiscoveredProvider[]> {
  const discovered: DiscoveredProvider[] = []

  await Promise.all(
    // A listed CLI gizzi can't drive yet isn't offered as a brain.
    subprocessSpecs().filter((spec) => spec.adapter !== "unsupported").map(async (spec) => {
      const binPath = await resolveCliPath(spec)
      if (!binPath) return

      const alive = await runProbe(binPath, spec)
      if (!alive) return

      let models = spec.models
      if (spec.id === "codex-cli") {
        try {
          const { stdout } = await execFileAsync(binPath, ["debug", "models"], { timeout: 5000 })
          const available = parseCodexModels(stdout)
          if (available.length > 0) models = available
        } catch {
          // Older Codex versions may not expose debug models; keep the fallback.
        }
      }
      if (spec.id === "ollama-cli" && models.length === 0) {
        models = await probeOllamaModels(binPath)
      }
      if (models.length === 0) return

      discovered.push({
        id: spec.id,
        name: spec.name,
        auth_type: "subprocess",
        subprocess_cmd: binPath,
        source: "subprocess",
        models,
      })
    }),
  )

  return discovered
}
