/**
 * Is a coding CLI signed in?
 *
 * Mirrors `subscription_auth_check` in cmd/allternit-api/src/provider_routes.rs
 * (and its codex OPENAI_API_KEY rule in `subprocess_provider_rows`), so gizzi
 * and Desktop agree on which CLIs are ready. The Rust check also requires the
 * binary to answer `--version`; discovery has already run that probe before
 * asking here, so it is not repeated.
 *
 * true / false = the CLI's own sign-in rule; undefined = no rule for this CLI
 * (a custom tool, or a catalog tool without an `auth` block).
 */

import { existsSync, readFileSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import { cacheGet, cachePut, defaultCacheFile, refreshRequested, runForStdout } from "./live-models"
import type { SubprocessSpec } from "./subprocess"
// Read directly (not through subprocess.ts, which imports this file).
import catalog from "./cli-catalog.json"

const SIGN_IN_TTL_MS = 10 * 60 * 1000
const SIGN_IN_TIMEOUT_MS = 5000

export interface SignInOptions {
  home?: string
  env?: NodeJS.ProcessEnv
  now?: () => number
  cacheFile?: string
  /** Tests: the platform to assume and the `security` program to run (macOS Keychain check). */
  platform?: NodeJS.Platform
  securityBin?: string
}

type CatalogAuth = { files?: string[][]; env?: string[] }

/** `claude auth status`: JSON `loggedIn`, else the text rules the Rust uses. Exit != 0 = signed out. */
export function claudeStatusLoggedIn(stdout: string): boolean {
  let value: unknown
  try {
    value = JSON.parse(stdout)
  } catch {
    value = undefined // not JSON: the text rules below
  }
  if (value !== undefined) return !!value && typeof value === "object" && (value as { loggedIn?: unknown }).loggedIn === true
  const lower = stdout.toLowerCase()
  return !lower.includes('"loggedin": false') && !lower.includes("not logged") && !lower.includes("logged out")
}

export async function cliSignedIn(
  spec: Pick<SubprocessSpec, "id" | "custom">,
  binPath: string,
  options: SignInOptions = {},
): Promise<boolean | undefined> {
  const env = options.env ?? process.env
  const home = options.home ?? env.HOME ?? os.homedir()
  const has = (...parts: string[]) => existsSync(path.join(home, ...parts))
  // Rust: std::env::var(..).is_ok() — set at all, even empty.
  const envSet = (name: string) => env[name] !== undefined

  switch (spec.id) {
    case "claude-cli":
      return claudeSignedIn(binPath, home, env, options)
    case "codex-cli": {
      // CODEX_HOME (the catalog's accountHomeEnv) moves codex's own auth.json.
      const codexHome = env.CODEX_HOME || path.join(home, ".codex")
      return existsSync(path.join(codexHome, "auth.json")) || envSet("OPENAI_API_KEY")
    }
    case "qwen-cli":
      // OAuth sign-in, or an API-key setup chosen in Qwen's own settings
      // (`security.auth.selectedType`), which leaves no oauth_creds.json.
      return (
        has(".qwen", "oauth_creds.json") ||
        has(".config", "qwen", "oauth_creds.json") ||
        qwenAuthSelected(path.join(home, ".qwen", "settings.json"))
      )
    case "kimi-cli":
      return (
        has(".kimi-code", "auth.json") ||
        has(".kimi-code", "config.json") ||
        has(".kimi-code", "credentials", "kimi-code.json") ||
        has(".kimi", "credentials", "kimi-code.json")
      )
    case "antigravity":
      if (
        has(".gemini", "oauth_creds.json") ||
        has(".gemini", "antigravity-cli", "credentials.enc") ||
        has(".config", "antigravity", "auth.json")
      )
        return true
      // On macOS agy keeps its sign-in in the Keychain (service "gemini",
      // account "antigravity"), not in a file. Reading the item's attributes
      // needs no unlock prompt.
      return antigravityKeychainSignedIn(home, env, options)
    case "grok":
      return has(".grok", "auth.json") || envSet("XAI_API_KEY")
  }

  if (spec.custom) return undefined
  const tools = (catalog as unknown as { tools: Array<{ id: string; auth?: CatalogAuth | null }> }).tools
  const tool = tools.find((t) => t.id === spec.id)
  const auth = tool?.auth
  const files = auth?.files ?? []
  const vars = auth?.env ?? []
  if (files.length === 0 && vars.length === 0) return undefined
  return files.some((parts) => has(...parts)) || vars.some((name) => !!env[name])
}

function qwenAuthSelected(settingsPath: string): boolean {
  try {
    const settings = JSON.parse(readFileSync(settingsPath, "utf8")) as { security?: { auth?: { selectedType?: unknown } } }
    const selected = settings.security?.auth?.selectedType
    return typeof selected === "string" && selected.length > 0
  } catch {
    return false
  }
}

async function antigravityKeychainSignedIn(
  home: string,
  env: NodeJS.ProcessEnv,
  options: SignInOptions,
): Promise<boolean | undefined> {
  // Only macOS has the Keychain; elsewhere the missing files are the answer.
  if ((options.platform ?? process.platform) !== "darwin") return false
  const now = options.now ?? Date.now
  const file = options.cacheFile ?? defaultCacheFile()
  const key = `signin:antigravity:keychain:${home}`
  const cached = cacheGet(key, file)
  if (!refreshRequested(env) && cached && typeof cached.value === "boolean" && now() - cached.at < SIGN_IN_TTL_MS) {
    return cached.value
  }
  const result = await runForStdout(
    [options.securityBin ?? "/usr/bin/security", "find-generic-password", "-s", "gemini", "-a", "antigravity"],
    SIGN_IN_TIMEOUT_MS,
    { ...env, HOME: home },
  )
  if (result === null) return undefined // timed out: unknown, and not cached
  const signedIn = result.exitCode === 0
  cachePut(key, signedIn, now(), file)
  return signedIn
}

async function claudeSignedIn(
  binPath: string,
  home: string,
  env: NodeJS.ProcessEnv,
  options: SignInOptions,
): Promise<boolean | undefined> {
  const now = options.now ?? Date.now
  const file = options.cacheFile ?? defaultCacheFile()
  // CLAUDE_CONFIG_DIR (accountHomeEnv) picks the account, so it is part of the key.
  const key = `signin:claude-cli:${binPath}:${env.CLAUDE_CONFIG_DIR || home}`
  const cached = cacheGet(key, file)
  if (!refreshRequested(env) && cached && typeof cached.value === "boolean" && now() - cached.at < SIGN_IN_TTL_MS) {
    return cached.value
  }
  const result = await runForStdout([binPath, "auth", "status"], SIGN_IN_TIMEOUT_MS, { ...env, HOME: home })
  if (result === null) return undefined // timed out: unknown, and not cached
  const signedIn = result.exitCode === 0 && claudeStatusLoggedIn(result.stdout)
  cachePut(key, signedIn, now(), file)
  return signedIn
}
