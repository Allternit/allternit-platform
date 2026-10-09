import fs from "fs"
import os from "os"
import path from "path"
import { readAuthProfiles } from "@/runtime/context/config/auth-profiles"
import { ALLTERNIT_GATEWAY_BASE } from "@/shared/constants/allternitGateway"
import { apiFetch } from "@/runtime/services/api/allternitApi"
import { isEssentialTrafficOnly } from "@/shared/utils/privacyLevel"
import { ROOT_INSTRUCTION_FILENAMES } from "@/shared/utils/agentFileResolver"

export type CheckStatus = "pass" | "warn" | "fail"
export type CheckTone = CheckStatus | "info"

export type DoctorCheck = {
  id: string
  section: string
  status: CheckTone
  message: string
}

/**
 * Project-instructions check. GIZZI.md is the canonical project-instructions
 * file (see ROOT_INSTRUCTION_FILENAMES in agentFileResolver); the legacy
 * CLAUDE.md/AGENTS.md/CONTEXT.md names are accepted but reported so projects
 * can migrate.
 */
export async function checkVoiceEngine(): Promise<DoctorCheck> {
  const sidecar =
    (process.env.ALLTERNIT_VOICE_URL || process.env.VOICE_URL || "http://127.0.0.1:8001").replace(
      /\/+$/,
      "",
    )
  try {
    const res = await fetch(`${sidecar}/health`, { signal: AbortSignal.timeout(1500) })
    if (res.ok) {
      const json = (await res.json().catch(() => ({}))) as {
        engine?: string
        packs?: { name: string; state: string; error?: string | null }[]
      }
      const engine = json.engine ?? "unknown"
      const failed = (json.packs ?? []).filter((p) => p.state === "error")
      const small = (json.packs ?? []).find((p) => p.name === "small")
      if (failed.length > 0) {
        return {
          id: "voice-engine",
          section: "Voice",
          status: "warn",
          message: `Voice sidecar up at ${sidecar} but voice pack download failed: ${failed
            .map((p) => `${p.name}: ${p.error ?? "error"}`)
            .join("; ")}`,
        }
      }
      return {
        id: "voice-engine",
        section: "Voice",
        status: "pass",
        message:
          small && small.state !== "ready"
            ? `Local voice sidecar healthy (${engine} at ${sidecar}); voice models download on first use`
            : `Local voice sidecar healthy (${engine} at ${sidecar})`,
      }
    }
  } catch {
    // Sidecar not reachable.
  }
  return {
    id: "voice-engine",
    section: "Voice",
    status: "warn",
    message: `Local voice engine not running at ${sidecar}. Start Allternit Desktop or run services/voice.`,
  }
}

export async function checkProjectInstructions(
  cwd: string,
  exists: (p: string) => boolean = fs.existsSync,
): Promise<DoctorCheck> {
  const canonical = path.join(cwd, "GIZZI.md")
  if (exists(canonical)) {
    return { id: "project-instructions", section: "Project", status: "pass", message: `GIZZI.md found: ${canonical}` }
  }
  const legacy = ROOT_INSTRUCTION_FILENAMES.slice(1).find((name) => exists(path.join(cwd, name)))
  if (legacy) {
    return {
      id: "project-instructions",
      section: "Project",
      status: "warn",
      message: `GIZZI.md not found; using legacy ${legacy}. Consider renaming it to GIZZI.md.`,
    }
  }
  return {
    id: "project-instructions",
    section: "Project",
    status: "warn",
    message: "GIZZI.md not found in current directory — project instructions will not be loaded",
  }
}

export type GatewayCheckDeps = {
  baseUrl?: string
  /** Injected for tests; defaults to essential-traffic detection. */
  offline?: boolean
  timeoutMs?: number
}

/**
 * Cloud reachability: GET the gateway base URL with a short timeout. Any
 * HTTP response (even an error status) proves reachability. When the user
 * runs in essential-traffic (offline) mode the check is skipped with a
 * explanatory pass.
 */
export async function checkGatewayReachability(deps: GatewayCheckDeps = {}): Promise<DoctorCheck> {
  const baseUrl = (deps.baseUrl ?? ALLTERNIT_GATEWAY_BASE).replace(/\/+$/, "")
  if (deps.offline ?? isEssentialTrafficOnly()) {
    return {
      id: "cloud-gateway",
      section: "Cloud",
      status: "pass",
      message: "offline mode (essential-traffic only) — cloud reachability not checked",
    }
  }
  const timeoutMs = deps.timeoutMs ?? 5_000
  try {
    const res = await apiFetch({ baseUrl, userId: "gizzi-doctor" }, "/", {
      method: "GET",
      signal: AbortSignal.timeout(timeoutMs),
    })
    return {
      id: "cloud-gateway",
      section: "Cloud",
      status: "pass",
      message: `gateway reachable: ${baseUrl} (HTTP ${res.status})`,
    }
  } catch (e) {
    return {
      id: "cloud-gateway",
      section: "Cloud",
      status: "fail",
      message: `gateway unreachable: ${baseUrl} (${e instanceof Error ? e.message : String(e)})`,
    }
  }
}

export type CronCheckDeps = {
  isRunning: () => Promise<boolean>
  /** Supervision probe from cron supervision (slice 4); null = not wired. */
  supervised?: () => Promise<{ launchdPlist: string | null; systemdUnit: string | null; supported: boolean } | null>
  /** Read an installed unit file (tests inject); null when unreadable. */
  readUnit?: (p: string) => string | null
}

/**
 * Units written before the Factory fold run `gizzi cron start`. A hidden
 * `cron start` keeps them working, but they should be rewritten.
 */
export function isLegacyCronUnit(contents: string): boolean {
  return /<string>cron<\/string>\s*<string>start<\/string>/.test(contents) || /ExecStart=\S+ cron start\b/.test(contents)
}

export async function checkCronDaemon(deps: CronCheckDeps): Promise<DoctorCheck[]> {
  const checks: DoctorCheck[] = []
  const running = await deps.isRunning()
  checks.push(
    running
      ? { id: "cron-daemon", section: "Cron", status: "pass", message: "cron daemon is running" }
      : { id: "cron-daemon", section: "Cron", status: "warn", message: "cron daemon is not running — scheduled jobs will not fire (`gizzi workflows wake jobs start`)" },
  )
  const sup = deps.supervised ? await deps.supervised() : null
  if (sup === null) {
    checks.push({ id: "cron-autostart", section: "Cron", status: "info", message: "autostart: not configured (`gizzi workflows wake jobs enable` to supervise the daemon)" })
  } else if (sup.launchdPlist || sup.systemdUnit) {
    const unitPath = (sup.launchdPlist ?? sup.systemdUnit)!
    const read =
      deps.readUnit ??
      ((p: string) => {
        try {
          return fs.readFileSync(p, "utf8")
        } catch {
          return null
        }
      })
    const contents = read(unitPath)
    checks.push(
      contents && isLegacyCronUnit(contents)
        ? {
            id: "cron-autostart",
            section: "Cron",
            status: "warn",
            message: `autostart installed by an older Gizzi (${unitPath}) — run \`gizzi workflows wake jobs enable\` to rewrite it`,
          }
        : { id: "cron-autostart", section: "Cron", status: "pass", message: `autostart installed: ${unitPath}` },
    )
  } else if (!sup.supported) {
    checks.push({ id: "cron-autostart", section: "Cron", status: "info", message: "autostart not supported on this platform" })
  } else {
    checks.push({ id: "cron-autostart", section: "Cron", status: "info", message: "autostart: not installed (`gizzi workflows wake jobs enable`)" })
  }
  return checks
}

export type CredentialCheckDeps = {
  /** Fallback credential file (~/.gizzi/credentials.json). */
  credentialsPath: string
  configTomlPath: string
}

const modeString = (mode: number): string => `0${(mode & 0o777).toString(8)}`

/**
 * Credential hygiene: the fallback credentials.json must be 0o600, and
 * config.toml must not carry inline api_key values (they belong in the
 * credential store — see auth-profiles.ts migrateInlineApiKeys).
 */
export async function checkCredentialSecurity(deps: CredentialCheckDeps): Promise<DoctorCheck[]> {
  const checks: DoctorCheck[] = []

  try {
    const stat = fs.statSync(deps.credentialsPath)
    if ((stat.mode & 0o777) === 0o600) {
      checks.push({ id: "credentials-permissions", section: "Credentials", status: "pass", message: `credentials.json permissions are 0600: ${deps.credentialsPath}` })
    } else {
      checks.push({
        id: "credentials-permissions",
        section: "Credentials",
        status: "fail",
        message: `credentials.json has mode ${modeString(stat.mode)}, expected 0600 — run: chmod 600 ${deps.credentialsPath}`,
      })
    }
  } catch {
    checks.push({ id: "credentials-permissions", section: "Credentials", status: "pass", message: "no fallback credentials file (keys live in the OS keyring or are not configured)" })
  }

  try {
    const auth = await readAuthProfiles(deps.configTomlPath)
    const inline = Object.entries(auth.profiles)
      .filter(([, profile]) => typeof profile.api_key === "string" && profile.api_key.length > 0)
      .map(([name]) => name)
    if (inline.length > 0) {
      checks.push({
        id: "config-inline-api-key",
        section: "Credentials",
        status: "fail",
        message: `config.toml contains inline api_key for profile(s): ${inline.join(", ")} — run \`gizzi auth login\` to move the key into the credential store`,
      })
    } else {
      checks.push({ id: "config-inline-api-key", section: "Credentials", status: "pass", message: "config.toml has no inline api keys" })
    }
  } catch (e) {
    checks.push({ id: "config-inline-api-key", section: "Credentials", status: "warn", message: `could not inspect config.toml: ${e instanceof Error ? e.message : String(e)}` })
  }

  return checks
}

export type FactoryEngineCheckDeps = {
  locate?: () => { path: string; source: string } | null
  version?: (enginePath: string) => string | null
  /** Resolves to an HTTP status when something answers on the engine port, null otherwise. */
  probe?: (url: string) => Promise<number | null>
  socketExists?: (p: string) => boolean
  env?: NodeJS.ProcessEnv
  home?: string
}

/**
 * `gizzi doctor` — the Allternit Factory engine: found (and where), its
 * version, and whether `allternit-factory serve` answers on its port.
 */
export async function checkFactoryEngine(deps: FactoryEngineCheckDeps = {}): Promise<DoctorCheck[]> {
  const section = "Factory"
  const env = deps.env ?? process.env
  const engine = await import("@/cli/factory/engine")
  const found = (deps.locate ?? (() => engine.locateEngine({ env, home: deps.home })))()
  const checks: DoctorCheck[] = []
  if (!found) {
    const badEnv = env.ALLTERNIT_FACTORY_BIN ? ` (ALLTERNIT_FACTORY_BIN=${env.ALLTERNIT_FACTORY_BIN} isn't an executable)` : ""
    checks.push({
      id: "factory-engine",
      section,
      status: "warn",
      message: `${engine.ENGINE_MISSING_FACT}${badEnv} — gizzi agents/orchestration/workflows/workspace need it. ${engine.ENGINE_MISSING_ACTION}`,
    })
    return checks
  }
  checks.push({ id: "factory-engine", section, status: "pass", message: `Engine found: ${found.path} (${found.source})` })
  const version = (deps.version ?? engine.engineVersion)(found.path)
  checks.push(
    version
      ? { id: "factory-engine-version", section, status: "info", message: `Engine version: ${version}` }
      : { id: "factory-engine-version", section, status: "warn", message: `\`${found.path} --version\` didn't answer — the engine may be damaged; reinstall it` },
  )
  const port = Number(env.ALLTERNIT_FACTORY_PORT) || 3011
  const url = `http://127.0.0.1:${port}/api/factory/agents`
  const probe =
    deps.probe ??
    (async (u: string) => {
      try {
        const res = await fetch(u, { signal: AbortSignal.timeout(1500) })
        return res.status
      } catch {
        return null
      }
    })
  const status = await probe(url)
  const home = deps.home ?? (await import("os")).homedir()
  const sock = path.join(home, ".allternit", "factory", "factory.sock")
  const sockExists = (deps.socketExists ?? fs.existsSync)(sock)
  if (status !== null) {
    checks.push({ id: "factory-serve", section, status: "pass", message: `Engine server answering on 127.0.0.1:${port} (HTTP ${status})` })
  } else if (sockExists) {
    checks.push({
      id: "factory-serve",
      section,
      status: "warn",
      message: `Engine socket ${sock} exists but nothing answers on 127.0.0.1:${port} — the server may have crashed; restart Allternit Desktop`,
    })
  } else {
    checks.push({
      id: "factory-serve",
      section,
      status: "info",
      message: `Engine server not running on 127.0.0.1:${port} (Allternit Desktop starts it; CLI commands run the engine directly)`,
    })
  }
  return checks
}

export type FactoryHomeMoveCheckDeps = {
  env?: NodeJS.ProcessEnv
  home?: string
  exists?: (p: string) => boolean
  readFile?: (p: string) => string
}

type HomeMoveMarker = {
  from?: string
  to?: string
  at?: string
  dryRun?: boolean
  moved?: unknown[]
  deduplicated?: unknown[]
  conflicts?: unknown[]
  kept?: unknown[]
  removedOldHome?: boolean
}

/**
 * `gizzi doctor` — the engine's one-time move of the pre-Factory orchestrator
 * home into the Factory home (`$ALLTERNIT_FACTORY_HOME`, default
 * ~/.allternit/factory). Reads the engine's migration marker and checks
 * whether the old folder is still there.
 */
export function checkFactoryHomeMove(deps: FactoryHomeMoveCheckDeps = {}): DoctorCheck {
  const id = "factory-home-move"
  const section = "Factory"
  const env = deps.env ?? process.env
  const home = deps.home ?? os.homedir()
  const exists = deps.exists ?? fs.existsSync
  const readFile = deps.readFile ?? ((p: string) => fs.readFileSync(p, "utf8"))
  const factoryHome = env.ALLTERNIT_FACTORY_HOME || path.join(home, ".allternit", "factory")
  const oldHome = path.join(home, ".agent-orchestrator") // old-names: keep (the folder the engine migrates from)
  const markerPath = path.join(factoryHome, "migrated-agent-orchestrator.json")

  let marker: HomeMoveMarker | null = null
  if (exists(markerPath)) {
    try {
      marker = JSON.parse(readFile(markerPath)) as HomeMoveMarker
    } catch (e) {
      return {
        id,
        section,
        status: "warn",
        message: `Couldn't read ${markerPath}: ${e instanceof Error ? e.message : String(e)}`,
      }
    }
  }
  const oldExists = exists(oldHome)

  if (!oldExists) {
    return marker
      ? { id, section, status: "pass", message: `Moved ~/.agent-orchestrator into ${factoryHome} on ${marker.at ?? "an unknown date"}` } // old-names: keep (reports the move)
      : { id, section, status: "pass", message: "Nothing to move" }
  }
  if (!marker) {
    return {
      id,
      section,
      status: "warn",
      message: "~/.agent-orchestrator hasn't moved yet. Start the engine once (Allternit Desktop does), or run `allternit-factory internal migrate-home`", // old-names: keep (names the folder to move)
    }
  }
  const conflicts = Array.isArray(marker.conflicts) ? marker.conflicts : []
  if (conflicts.length) {
    const first = conflicts.slice(0, 3).map((c) => (typeof c === "string" ? c : JSON.stringify(c))).join(", ")
    const more = conflicts.length > 3 ? ", …" : ""
    return {
      id,
      section,
      status: "warn",
      message: `${conflicts.length} item(s) left in ~/.agent-orchestrator because ${factoryHome} has different files with the same names: ${first}${more}. Compare and remove them by hand`, // old-names: keep (names the folder holding conflicts)
    }
  }
  const kept = Array.isArray(marker.kept) ? marker.kept : []
  if (kept.length) {
    const names = kept.map((k) => (typeof k === "string" ? k : JSON.stringify(k))).join(", ")
    return {
      id,
      section,
      status: "pass",
      message: `Moved ~/.agent-orchestrator into ${factoryHome} on ${marker.at ?? "an unknown date"}; kept in place: ${names} (venvs/worktrees can't move)`, // old-names: keep (reports the move)
    }
  }
  return {
    id,
    section,
    status: "warn",
    message: "~/.agent-orchestrator came back after the move (an old ao-* script ran). Run `allternit-factory internal migrate-home --again`", // old-names: keep (names the folder that came back)
  }
}
