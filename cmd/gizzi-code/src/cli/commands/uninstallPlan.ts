import fs from "fs/promises"
import path from "path"
import fsSync from "fs"
import { Installation } from "@/shared/installation"

/**
 * Pure planning/detect logic for `gizzi uninstall`, kept separate from the
 * interactive command so tests can inject HOME/argv/execPath.
 */

/** Published package name — older builds referenced the upstream `@gizzi/tui`. */
export const GIZZI_PACKAGE_NAME = "@allternit/gizzi-code"

export type InstallMethod =
  | "curl"
  | "npm"
  | "pnpm"
  | "bun"
  | "yarn"
  | "brew"
  | "choco"
  | "scoop"
  | "unknown"

/** The legacy per-user install/workspace root: config, credentials, plugins, bin. */
export function gizziHome(home: string): string {
  return path.join(home, ".gizzi")
}

/**
 * Detect the install method from the binary path and argv. Path/argv
 * detection is authoritative (it never shells out to package managers);
 * `"unknown"` means the caller should fall back to `Installation.method()`.
 */
export function detectInstallMethod(execPath: string, argv: string[]): InstallMethod {
  // Normalize separators so Windows paths are matched the same way.
  const exec = execPath.toLowerCase().replace(/\\/g, "/")
  if (exec.includes("/.gizzi/bin/")) return "curl"
  if (exec.includes("/.local/bin/")) return "curl"

  // npm-style layout: the launcher shim lives inside the global
  // node_modules/<pkg> tree (invoked via a bin symlink or `npm exec`).
  const inPackageTree = [exec, ...argv.map((a) => a.toLowerCase().replace(/\\/g, "/"))].some(
    (segment) => segment.includes(`node_modules/${GIZZI_PACKAGE_NAME.toLowerCase()}`) || segment.includes("node_modules/@gizzi/tui"),
  )
  if (inPackageTree || exec.includes("/npm/") || exec.includes("/.npm/") || exec.includes("/npx/")) {
    return "npm"
  }
  if (exec.includes("/pnpm/") || exec.includes(".pnpm")) return "pnpm"
  if (exec.includes("/bun/") || exec.includes(".bun")) return "bun"
  if (exec.includes("/yarn/")) return "yarn"
  if (exec.includes("homebrew") || exec.includes("/cellar/")) return "brew"
  if (exec.includes("chocolatey")) return "choco"
  if (exec.includes("/scoop/")) return "scoop"
  return "unknown"
}

/** The correct uninstall hint/command for the detected method, or null when the package manager is not involved (curl/unknown). */
export function packageUninstallCommand(method: InstallMethod): string | null {
  switch (method) {
    case "npm":
      return `npm uninstall -g ${GIZZI_PACKAGE_NAME}`
    case "pnpm":
      return `pnpm uninstall -g ${GIZZI_PACKAGE_NAME}`
    case "bun":
      return `bun remove -g ${GIZZI_PACKAGE_NAME}`
    case "yarn":
      return `yarn global remove ${GIZZI_PACKAGE_NAME}`
    case "brew":
      return "brew uninstall gizzi"
    case "choco":
      return "choco uninstall gizzi"
    case "scoop":
      return "scoop uninstall gizzi"
    default:
      return null
  }
}

export type GizziHomeEntry = {
  name: string
  path: string
  kind: "config" | "credentials" | "plugins" | "binary" | "cron" | "workspace" | "other"
}

const KNOWN_ENTRY_KINDS: Record<string, GizziHomeEntry["kind"]> = {
  "credentials.json": "credentials",
  credentials: "credentials",
  plugins: "plugins",
  bin: "binary",
  "cron.db": "cron",
  "cron-daemon.pid": "cron",
}

function classifyEntry(name: string): GizziHomeEntry["kind"] {
  if (KNOWN_ENTRY_KINDS[name]) return KNOWN_ENTRY_KINDS[name]!
  if (name.endsWith(".toml") || name.endsWith(".json")) return "config"
  if (name === "IDENTITY.md" || name === "SOUL.md" || name === "USER.md" || name === "MEMORY.md" || name === "AGENTS.md") return "workspace"
  return "other"
}

export type GizziHomePlan = {
  path: string
  exists: boolean
  entries: GizziHomeEntry[]
}

/** List what lives under ~/.gizzi so uninstall can show exactly what will be deleted. */
export async function planGizziHome(home: string): Promise<GizziHomePlan> {
  const root = gizziHome(home)
  let names: string[] = []
  try {
    names = (await fs.readdir(root)).sort()
  } catch {
    return { path: root, exists: false, entries: [] }
  }
  const entries: GizziHomeEntry[] = []
  for (const name of names) {
    entries.push({ name, path: path.join(root, name), kind: classifyEntry(name) })
  }
  return { path: root, exists: true, entries }
}

export type UninstallPlan = {
  method: InstallMethod
  gizziHome: GizziHomePlan
  /** ~/.gizzi is removed unless --keep-config is passed. */
  removeGizziHome: boolean
  /** Printed hint when a package manager owns the install. */
  packageHint: string | null
}

export type UninstallPlanArgs = {
  keepConfig: boolean
  home: string
  execPath: string
  argv: string[]
}

export async function buildUninstallPlan(args: UninstallPlanArgs): Promise<UninstallPlan> {
  const method = detectInstallMethod(args.execPath, args.argv)
  return {
    method,
    gizziHome: await planGizziHome(args.home),
    removeGizziHome: !args.keepConfig,
    packageHint: packageUninstallCommand(method),
  }
}

export type DesktopUninstallPlan =
  | { desktop: false }
  | {
      desktop: true
      /** The ~/.local/bin/gizzi symlink to Desktop's copy, when one exists. The only thing we may remove. */
      link?: string
      /** Where Desktop's real copy lives. Never removed. */
      realPath: string
    }

export type DesktopUninstallDeps = {
  execPath?: string
  argv1?: string
  home?: string
  env?: NodeJS.ProcessEnv
  realpath?: (p: string) => string | undefined
  isSymlink?: (p: string) => boolean
}

/**
 * When the running gizzi resolves (through symlinks) to Allternit Desktop's
 * bundled copy, uninstalling must never touch that copy: Desktop owns it.
 * The only removable item is the user's ~/.local/bin/gizzi symlink.
 */
export function planDesktopUninstall(deps: DesktopUninstallDeps = {}): DesktopUninstallPlan {
  const real =
    deps.realpath ??
    ((p: string) => {
      try {
        return fsSync.realpathSync(p)
      } catch {
        return undefined
      }
    })
  const isSymlink =
    deps.isSymlink ??
    ((p: string) => {
      try {
        return fsSync.lstatSync(p).isSymbolicLink()
      } catch {
        return false
      }
    })
  const execPath = deps.execPath ?? process.execPath
  const execReal = real(execPath) ?? execPath
  const argvReal = deps.argv1 ?? process.argv[1] ? real(deps.argv1 ?? process.argv[1]!) : undefined
  if (!Installation.isDesktopManaged([execReal, argvReal], deps.env ?? process.env)) return { desktop: false }
  const link = path.join(deps.home ?? process.env.HOME ?? "", ".local", "bin", "gizzi")
  const linkReal = real(link)
  const removable =
    isSymlink(link) && !!linkReal && Installation.isDesktopManaged([linkReal], {}) && !link.toLowerCase().includes(".app/")
  return { desktop: true, realPath: execReal, link: removable ? link : undefined }
}
