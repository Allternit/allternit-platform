/**
 * Subprocess Provider Discovery
 *
 * Scans PATH for known LLM CLI tools. For each one found, creates a provider
 * entry with auth_type: "subprocess". The user gets it in /model with zero
 * configuration — just having the CLI installed and logged in is enough.
 *
 * Adding a new CLI:
 *   Append an entry to SUBPROCESS_PROVIDERS below. That's it.
 */

import { which } from "bun"
import { execFile } from "node:child_process"
import { promisify } from "node:util"
import { existsSync, readdirSync, statSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import type { DiscoveredProvider, DiscoveredModel } from "./index"

const execFileAsync = promisify(execFile)

export function parseCodexModels(stdout: string): DiscoveredModel[] {
  const catalog = JSON.parse(stdout) as { models?: Array<{ slug?: string; display_name?: string; context_window?: number }> }
  return (catalog.models ?? []).filter((model) => model.slug).map((model) => ({
    id: model.slug!,
    name: model.display_name || model.slug!,
    context: model.context_window || 272000,
    output: 32768,
  }))
}

/**
 * Per-provider environment overrides matching the Multica Go runtime.
 *
 * `MULTICA_<PROVIDER>_PATH` takes precedence over PATH lookup.
 * `MULTICA_<PROVIDER>_MODEL` is available for adapters that accept a --model flag.
 */
export const PROVIDER_ENV_KEYS: Record<string, { path: string; model?: string }> = {
  "claude-cli":   { path: "MULTICA_CLAUDE_PATH",       model: "MULTICA_CLAUDE_MODEL" },
  "codex-cli":    { path: "MULTICA_CODEX_PATH",        model: "MULTICA_CODEX_MODEL" },
  opencode:       { path: "MULTICA_OPENCODE_PATH",     model: "MULTICA_OPENCODE_MODEL" },
  deveco:         { path: "MULTICA_DEVECO_PATH",       model: "MULTICA_DEVECO_MODEL" },
  openclaw:       { path: "MULTICA_OPENCLAW_PATH",     model: "MULTICA_OPENCLAW_MODEL" },
  hermes:         { path: "MULTICA_HERMES_PATH",       model: "MULTICA_HERMES_MODEL" },
  pi:             { path: "MULTICA_PI_PATH",           model: "MULTICA_PI_MODEL" },
  "cursor-agent": { path: "MULTICA_CURSOR_PATH",       model: "MULTICA_CURSOR_MODEL" },
  "kimi-cli":     { path: "MULTICA_KIMI_PATH",         model: "MULTICA_KIMI_MODEL" },
  reasonix:       { path: "MULTICA_REASONIX_PATH",     model: "MULTICA_REASONIX_MODEL" },
  "kiro-cli":     { path: "MULTICA_KIRO_PATH",         model: "MULTICA_KIRO_MODEL" },
  codebuddy:      { path: "MULTICA_CODEBUDDY_PATH",    model: "MULTICA_CODEBUDDY_MODEL" },
  qodercli:       { path: "MULTICA_QODER_PATH",        model: "MULTICA_QODER_MODEL" },
  qoderclicn:     { path: "MULTICA_QODERCLICN_PATH",   model: "MULTICA_QODERCLICN_MODEL" },
  traecli:        { path: "MULTICA_TRAECLI_PATH",      model: "MULTICA_TRAECLI_MODEL" },
  grok:           { path: "MULTICA_GROK_PATH",         model: "MULTICA_GROK_MODEL" },
  "qwen-cli":     { path: "MULTICA_QWEN_PATH",         model: "MULTICA_QWEN_MODEL" },
  qwenpaw:        { path: "MULTICA_QWENPAW_PATH" },
  mcode:          { path: "MULTICA_MCODE_PATH" },
  antigravity:    { path: "MULTICA_ANTIGRAVITY_PATH",  model: "MULTICA_ANTIGRAVITY_MODEL" },
  omp:            { path: "MULTICA_OMP_PATH",          model: "MULTICA_OMP_MODEL" },
}

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
  const spec = SUBPROCESS_PROVIDERS.find((s) => s.id === cliId)
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
  // CLIs whose installers use their own bin dir.
  const ownDir: Record<string, string> = { "kimi-cli": ".kimi-code", grok: ".grok", opencode: ".opencode" }
  if (ownDir[spec.id]) candidates.push(path.join(home, ownDir[spec.id], "bin", spec.bin))
  for (const dir of userBinDirs(home)) candidates.push(path.join(dir, spec.bin))
  return [...new Set(candidates)]
}

export interface SubprocessSpec {
  /** Binary name to look for in PATH */
  bin: string
  /** Provider ID in gizzi's model list */
  id: string
  name: string
  /** Icon asset key (maps to /icons/agent-clis/{icon}.svg) */
  icon?: string
  /** Command template — {prompt} is replaced with the user's message */
  cmd: string
  /** Known models surfaced by this CLI */
  models: DiscoveredModel[]
  /**
   * Optional probe — run this and check stdout to confirm auth is active.
   * If omitted, presence in PATH is treated as sufficient.
   */
  probe?: { args: string[]; expect: string | RegExp }
}

export const SUBPROCESS_PROVIDERS: SubprocessSpec[] = [
  // ── Anthropic ────────────────────────────────────────────────────────────
  {
    bin: "claude",
    id: "claude-cli",
    name: "Claude (installed CLI)",
    icon: "claude",
    cmd: "claude -p",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [
      { id: "claude-sonnet-5", name: "Claude Sonnet 5", context: 200000, output: 64000 },
      { id: "claude-opus-5",   name: "Claude Opus 5",   context: 200000, output: 32000 },
    ],
  },

  // ── Moonshot / Kimi ──────────────────────────────────────────────────────
  {
    bin: "kimi",
    id: "kimi-cli",
    name: "Kimi (CLI — subscription)",
    icon: "kimi",
    cmd: "kimi -p",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [
      { id: "kimi-k3",       name: "Kimi K3",       context: 262144, output: 16384 },
      { id: "kimi-k2.7-code", name: "Kimi K2.7 Code", context: 262144, output: 16384 },
    ],
  },

  // ── Alibaba Qwen ─────────────────────────────────────────────────────────
  {
    bin: "qwen",
    id: "qwen-cli",
    name: "Qwen Code (CLI — subscription)",
    icon: "qwen",
    cmd: "qwen -p",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [
      { id: "qwen-max",  name: "Qwen Max",  context: 131072, output: 8192 },
      { id: "qwen-plus", name: "Qwen Plus", context: 131072, output: 8192 },
    ],
  },

  // ── OpenAI Codex ─────────────────────────────────────────────────────────
  {
    bin: "codex",
    id: "codex-cli",
    name: "Codex CLI (OpenAI — subscription or API)",
    icon: "codex",
    cmd: "codex",
    probe: { args: ["--version"], expect: /codex/i },
    models: [{ id: "gpt-6-astra", name: "GPT-6-Astra", context: 272000, output: 32768 }],
  },

  // ── Google Gemini ─────────────────────────────────────────────────────────
  {
    bin: "gemini",
    id: "gemini-cli",
    name: "Gemini CLI (Google — subscription)",
    icon: "gemini",
    cmd: "gemini -p",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [
      { id: "gemini-1.5-pro-latest",  name: "Gemini 1.5 Pro",  context: 2000000, output: 8192 },
      { id: "gemini-1.5-flash-latest", name: "Gemini 1.5 Flash", context: 1000000, output: 8192 },
    ],
  },

  // ── Google Antigravity (agy CLI — subscription) ──────────────────────────
  {
    bin: "agy",
    id: "antigravity",
    name: "Antigravity (agy CLI — subscription)",
    icon: "agy",
    cmd: "agy -p",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [
      { id: "gemini-3.7-flash-high", name: "Gemini 3.7 Flash", context: 1000000, output: 65536 },
    ],
  },

  // ── GitHub Copilot ───────────────────────────────────────────────────────
  {
    bin: "gh",
    id: "copilot-cli",
    name: "GitHub Copilot (CLI — subscription)",
    icon: "copilot",
    cmd: "gh copilot suggest -t shell",
    probe: { args: ["copilot", "--version"], expect: /copilot/i },
    models: [
      { id: "copilot-gpt-4o",  name: "Copilot GPT-4o",  context: 128000, output: 4096  },
      { id: "copilot-claude",  name: "Copilot Claude",  context: 200000, output: 8192  },
    ],
  },

  // ── Simon Willison's LLM tool ────────────────────────────────────────────
  {
    bin: "llm",
    id: "llm-cli",
    name: "LLM (CLI — any configured backend)",
    icon: "llm",
    cmd: "llm prompt",
    probe: { args: ["--version"], expect: /llm/ },
    models: [
      { id: "default", name: "LLM CLI default model", context: 128000, output: 8192 },
    ],
  },

  // ── aichat ───────────────────────────────────────────────────────────────
  {
    bin: "aichat",
    id: "aichat-cli",
    name: "AIChat (CLI — any configured backend)",
    icon: "aichat",
    cmd: "aichat",
    probe: { args: ["--version"], expect: /aichat/ },
    models: [
      { id: "default", name: "AIChat default model", context: 128000, output: 8192 },
    ],
  },

  // ── Ollama (CLI — reads installed models) ────────────────────────────────
  {
    bin: "ollama",
    id: "ollama-cli",
    name: "Ollama (CLI)",
    icon: "ollama",
    cmd: "ollama run",
    probe: { args: ["list"], expect: /NAME/ },
    models: [], // populated dynamically via probeOllamaModels()
  },

  // ── fabric ───────────────────────────────────────────────────────────────
  {
    bin: "fabric",
    id: "fabric-cli",
    name: "Fabric (CLI — any configured backend)",
    icon: "fabric",
    cmd: "fabric",
    probe: { args: ["--version"], expect: /fabric/ },
    models: [
      { id: "default", name: "Fabric default model", context: 128000, output: 8192 },
    ],
  },

  // ── ChatGPT (unofficial CLIs) ────────────────────────────────────────────
  {
    bin: "chatgpt",
    id: "chatgpt-cli",
    name: "ChatGPT (CLI — Plus/Pro subscription)",
    icon: "chatgpt",
    cmd: "chatgpt",
    models: [
      { id: "gpt-4o",   name: "GPT-4o",   context: 128000, output: 16384  },
      { id: "o3",       name: "o3",       context: 200000, output: 100000 },
      { id: "o4-mini",  name: "o4-mini",  context: 200000, output: 100000 },
    ],
  },

  // ── Cursor Agent ─────────────────────────────────────────────────────────
  {
    bin: "cursor-agent",
    id: "cursor-agent",
    name: "Cursor Agent",
    icon: "cursor",
    cmd: "cursor-agent",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [{ id: "default", name: "Cursor Agent default", context: 200000, output: 64000 }],
  },

  // ── OpenCode ─────────────────────────────────────────────────────────────
  {
    bin: "opencode",
    id: "opencode",
    name: "OpenCode",
    icon: "opencode",
    cmd: "opencode",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [{ id: "default", name: "OpenCode default", context: 200000, output: 64000 }],
  },

  // ── OpenClaw ─────────────────────────────────────────────────────────────
  {
    bin: "openclaw",
    id: "openclaw",
    name: "OpenClaw",
    icon: "openclaw",
    cmd: "openclaw",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [{ id: "default", name: "OpenClaw default", context: 200000, output: 64000 }],
  },

  // ── Hermes ─────────────────────────────────────────────────────────────────
  {
    bin: "hermes",
    id: "hermes",
    name: "Hermes",
    icon: "hermes",
    cmd: "hermes",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [{ id: "default", name: "Hermes default", context: 200000, output: 64000 }],
  },

  // ── Pi ─────────────────────────────────────────────────────────────────────
  {
    bin: "pi",
    id: "pi",
    name: "Pi",
    icon: "pi",
    cmd: "pi -p --mode json",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [{ id: "default", name: "Pi default", context: 200000, output: 64000 }],
  },

  // ── CodeBuddy ──────────────────────────────────────────────────────────────
  {
    bin: "codebuddy",
    id: "codebuddy",
    name: "CodeBuddy",
    icon: "codebuddy",
    cmd: "codebuddy",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [{ id: "default", name: "CodeBuddy default", context: 200000, output: 64000 }],
  },

  // ── DevEco Code ────────────────────────────────────────────────────────────
  {
    bin: "deveco",
    id: "deveco",
    name: "DevEco Code",
    icon: "deveco",
    cmd: "deveco",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [{ id: "default", name: "DevEco Code default", context: 200000, output: 64000 }],
  },

  // ── Grok ───────────────────────────────────────────────────────────────────
  {
    bin: "grok",
    id: "grok",
    name: "Grok",
    icon: "grok",
    cmd: "grok",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [{ id: "default", name: "Grok default", context: 200000, output: 64000 }],
  },

  // ── Kiro CLI ───────────────────────────────────────────────────────────────
  {
    bin: "kiro-cli",
    id: "kiro-cli",
    name: "Kiro CLI",
    icon: "kiro",
    cmd: "kiro-cli",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [{ id: "default", name: "Kiro CLI default", context: 200000, output: 64000 }],
  },

  // ── Qoder CLI ──────────────────────────────────────────────────────────────
  {
    bin: "qodercli",
    id: "qodercli",
    name: "Qoder CLI",
    icon: "qoder",
    cmd: "qodercli",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [{ id: "default", name: "Qoder CLI default", context: 200000, output: 64000 }],
  },

  // ── Qoder CN ───────────────────────────────────────────────────────────────
  {
    bin: "qoderclicn",
    id: "qoderclicn",
    name: "Qoder CN",
    icon: "qoder-cn",
    cmd: "qoderclicn",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [{ id: "default", name: "Qoder CN default", context: 200000, output: 64000 }],
  },

  // ── QwenPaw ────────────────────────────────────────────────────────────────
  {
    bin: "qwenpaw",
    id: "qwenpaw",
    name: "QwenPaw",
    icon: "qwenpaw",
    cmd: "qwenpaw",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [{ id: "default", name: "QwenPaw default", context: 200000, output: 64000 }],
  },

  // ── Reasonix ───────────────────────────────────────────────────────────────
  {
    bin: "reasonix",
    id: "reasonix",
    name: "Reasonix",
    icon: "reasonix",
    cmd: "reasonix",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [{ id: "default", name: "Reasonix default", context: 200000, output: 64000 }],
  },

  // ── Trae CLI ─────────────────────────────────────────────────────────────────
  {
    bin: "traecli",
    id: "traecli",
    name: "Trae CLI",
    icon: "trae",
    cmd: "traecli",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [{ id: "default", name: "Trae CLI default", context: 200000, output: 64000 }],
  },

  // ── MiniMax Code ─────────────────────────────────────────────────────────────
  {
    bin: "mcode",
    id: "mcode",
    name: "MiniMax Code",
    icon: "mcode",
    cmd: "mcode acp",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [{ id: "default", name: "MiniMax Code default", context: 200000, output: 64000 }],
  },

  // ── DeepSeek Harness ─────────────────────────────────────────────────────────
  {
    bin: "dsh",
    id: "dsh",
    name: "DeepSeek Harness",
    icon: "dsh",
    cmd: "dsh --profile multica --stdio",
    probe: { args: ["--profile", "multica", "--probe"], expect: /"type":"probe"/ },
    models: [{ id: "default", name: "DSH default", context: 200000, output: 64000 }],
  },

  // ── Oh-My-Pi ─────────────────────────────────────────────────────────────────
  {
    bin: "omp",
    id: "omp",
    name: "Oh-My-Pi",
    icon: "omp",
    cmd: "omp -p --mode json",
    probe: { args: ["--version"], expect: /\d+\.\d+/ },
    models: [{ id: "default", name: "Oh-My-Pi default", context: 200000, output: 64000 }],
  },
]

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
    SUBPROCESS_PROVIDERS.map(async (spec) => {
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
        subprocess_cmd: `${binPath} ${spec.cmd.split(" ").slice(1).join(" ")}`.trim(),
        source: "subprocess",
        models,
      })
    }),
  )

  return discovered
}
