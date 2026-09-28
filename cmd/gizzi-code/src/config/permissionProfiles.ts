/**
 * Gizzi Code Permission Profiles
 *
 * Filesystem-based permission profiles that extend the inline
 * `permission_profiles` config field with standalone profile files.
 *
 * Profile files live in:
 *   ~/.config/gizzi-code/permission-profiles/<name>.json   (user scope)
 *   .gizzi/permission-profiles/<name>.json            (project scope)
 *
 * Each profile file defines a set of permission rules that are merged into
 * the active permission ruleset when the profile is activated.
 *
 * Activation order: explicit config `permission_profiles.active_profile` wins,
 * then GIZZI_PERMISSION_PROFILE env var, then project-level active marker,
 * then user-level active marker.
 */

import { Log } from "@/shared/util/log"
import { Filesystem } from "@/shared/util/filesystem"
import { Global } from "@/runtime/context/global"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import path from "path"
import fs from "fs/promises"
import { existsSync } from "fs"
import z from "zod/v4"

const log = Log.create({ service: "permission-profiles" })

export namespace PermissionProfiles {
  export const Scope = z.enum(["user", "project"])
  export type Scope = z.infer<typeof Scope>

  export const FileEntry = z.object({
    name: z.string(),
    description: z.string().optional(),
    rules: z.record(z.string(), z.union([z.string(), z.record(z.string(), z.string())])),
    sandbox: z
      .object({
        enabled: z.boolean().optional(),
        allow_network: z.boolean().optional(),
      })
      .optional(),
  })
  export type FileEntry = z.infer<typeof FileEntry>

  const PRESETS: Record<string, FileEntry> = {
    "read-only": {
      name: "read-only",
      description: "Read-only access — no file writes, no shell commands",
      rules: {
        read: "allow",
        glob: "allow",
        grep: "allow",
        list: "allow",
        websearch: "allow",
        webfetch: "allow",
        codesearch: "allow",
        lsp: "allow",
        edit: "deny",
        write: "deny",
        bash: "deny",
        patch: "deny",
        multiedit: "deny",
      },
    },
    "developer": {
      name: "developer",
      description: "Full developer access — reads, writes, and shell",
      rules: {
        read: "allow",
        glob: "allow",
        grep: "allow",
        list: "allow",
        edit: "allow",
        write: "allow",
        patch: "allow",
        multiedit: "allow",
        bash: "ask",
        websearch: "allow",
        webfetch: "allow",
        codesearch: "allow",
        lsp: "allow",
        todowrite: "allow",
        todoread: "allow",
        question: "allow",
      },
    },
    "ci-safe": {
      name: "ci-safe",
      description: "CI/CD pipeline — auto-allow reads/writes, deny interactive prompts",
      rules: {
        read: "allow",
        glob: "allow",
        grep: "allow",
        list: "allow",
        edit: "allow",
        write: "allow",
        patch: "allow",
        multiedit: "allow",
        bash: "allow",
        websearch: "allow",
        webfetch: "allow",
        codesearch: "allow",
        lsp: "allow",
        todowrite: "allow",
        todoread: "allow",
        question: "deny",
        skill: "allow",
      },
      sandbox: {
        enabled: true,
        allow_network: false,
      },
    },
    "restricted": {
      name: "restricted",
      description: "Minimal access — reads only, no shell, no external",
      rules: {
        read: "allow",
        glob: "allow",
        grep: "allow",
        list: "allow",
        edit: "deny",
        write: "deny",
        bash: "deny",
        patch: "deny",
        multiedit: "deny",
        websearch: "deny",
        webfetch: "deny",
        external_directory: "deny",
      },
    },
  }

  function profileDir(scope: Scope): string {
    if (scope === "user") {
      return path.join(Global.Path.config, "permission-profiles")
    }
    return path.join(process.cwd(), ".gizzi", "permission-profiles")
  }

  function activeMarkerPath(scope: Scope): string {
    return path.join(profileDir(scope), ".active")
  }

  /**
   * Get a preset profile by name.
   */
  export function getPreset(name: string): FileEntry | null {
    return PRESETS[name] ?? null
  }

  /**
   * List all preset names.
   */
  export function listPresets(): string[] {
    return Object.keys(PRESETS)
  }

  /**
   * Load a file-based profile by name and scope.
   */
  export async function load(name: string, scope: Scope): Promise<FileEntry | null> {
    const file = path.join(profileDir(scope), `${name}.json`)
    try {
      const text = await Filesystem.readText(file)
      if (!text) return null
      const parsed = FileEntry.safeParse(JSON.parse(text))
      return parsed.success ? parsed.data : null
    } catch {
      return null
    }
  }

  /**
   * Load a profile by name, searching project scope first then user scope,
   * with presets as fallback.
   */
  export async function resolve(name: string): Promise<FileEntry | null> {
    // Project scope first (higher priority)
    const projectProfile = await load(name, "project")
    if (projectProfile) return projectProfile

    // User scope
    const userProfile = await load(name, "user")
    if (userProfile) return userProfile

    // Presets
    return getPreset(name)
  }

  /**
   * Save a profile to disk.
   */
  export async function save(
    name: string,
    scope: Scope,
    entry: FileEntry,
  ): Promise<string> {
    const dir = profileDir(scope)
    await fs.mkdir(dir, { recursive: true })
    const file = path.join(dir, `${name}.json`)
    await fs.writeFile(file, JSON.stringify(entry, null, 2))
    log.info("saved permission profile", { name, scope, path: file })
    return file
  }

  /**
   * Delete a profile from disk.
   */
  export async function remove(name: string, scope: Scope): Promise<boolean> {
    const file = path.join(profileDir(scope), `${name}.json`)
    try {
      await fs.unlink(file)
      return true
    } catch {
      return false
    }
  }

  /**
   * List all profiles for a scope.
   */
  export async function listAll(scope: Scope): Promise<FileEntry[]> {
    const dir = profileDir(scope)
    if (!existsSync(dir)) return []

    const results: FileEntry[] = []
    try {
      const files = await fs.readdir(dir)
      for (const file of files) {
        if (!file.endsWith(".json")) continue
        const entry = await load(file.replace(/\.json$/, ""), scope)
        if (entry) results.push(entry)
      }
    } catch {
      // Directory may not exist
    }
    return results
  }

  /**
   * Activate a profile at the given scope.
   */
  export async function activate(name: string, scope: Scope): Promise<void> {
    const dir = profileDir(scope)
    await fs.mkdir(dir, { recursive: true })
    await fs.writeFile(activeMarkerPath(scope), name)
    log.info("activated permission profile", { name, scope })
  }

  /**
   * Get the active profile name at the given scope.
   */
  export async function getActiveName(scope: Scope): Promise<string | null> {
    try {
      const name = (await Filesystem.readText(activeMarkerPath(scope))).trim()
      return name || null
    } catch {
      return null
    }
  }

  /**
   * Resolve the effective active profile across all sources.
   * Priority: env var > project marker > user marker.
   */
  export async function getEffective(): Promise<FileEntry | null> {
    // Environment variable override
    const envProfile = process.env.GIZZI_PERMISSION_PROFILE
    if (envProfile) {
      const entry = await resolve(envProfile)
      if (entry) return entry
    }

    // Project marker
    const projectActive = await getActiveName("project")
    if (projectActive) {
      const entry = await resolve(projectActive)
      if (entry) return entry
    }

    // User marker
    const userActive = await getActiveName("user")
    if (userActive) {
      const entry = await resolve(userActive)
      if (entry) return entry
    }

    return null
  }

  /**
   * Deactivate at all scopes.
   */
  export async function deactivate(): Promise<void> {
    for (const scope of ["user", "project"] as Scope[]) {
      try {
        await fs.unlink(activeMarkerPath(scope))
      } catch {
        // Already deactivated
      }
    }
  }

  /**
   * Convert a FileEntry's rules into the PermissionNext.Ruleset format.
   */
  export function toRuleset(entry: FileEntry): PermissionNext.Ruleset {
    const ruleset: PermissionNext.Ruleset = []
    for (const [permission, action] of Object.entries(entry.rules)) {
      if (typeof action === "string") {
        ruleset.push({
          permission,
          pattern: "*",
          action: action as PermissionNext.Action,
          source: "project",
        })
      } else {
        for (const [pattern, act] of Object.entries(action)) {
          ruleset.push({
            permission,
            pattern,
            action: act as PermissionNext.Action,
            source: "project",
          })
        }
      }
    }
    return ruleset
  }
}
