import { Log } from "@/shared/util/log"
import path from "path"
import { pathToFileURL } from "url"
import os from "os"
import z from "zod/v4"
import { Filesystem } from "@/shared/util/filesystem"
import { ModelsDev } from "@/runtime/providers/adapters/models"
import { mergeDeep, pipe, unique } from "remeda"
import { Global } from "@/runtime/context/global"
import fs from "fs/promises"
import { lazy } from "@/shared/util/lazy"
import { NamedError } from "@allternit/gizzi-util/error.js"
import { Flag } from "@/runtime/context/flag/flag"
import { Auth } from "@/runtime/integrations/auth"
import {
  type ParseError as JsoncParseError,
  applyEdits,
  modify,
  parse as parseJsonc,
  printParseErrorCode,
} from "jsonc-parser"
import { Instance } from "@/runtime/context/project/instance"
import { LSPServer } from "@/runtime/integrations/lsp/server"
import { BunProc } from "@/shared/bun"
import { Installation } from "@/shared/installation"
import { ConfigMarkdown } from "@/runtime/context/config/markdown"
import { constants, existsSync } from "fs"
import { Bus } from "@/shared/bus"
import { GlobalBus } from "@/shared/bus/global"
import { Event } from "@/runtime/server/event"
import { Glob } from "@/shared/util/glob"
import { PackageRegistry } from "@/shared/bun/registry"
import { proxied } from "@/shared/util/proxied"
import { iife } from "@/shared/util/iife"
import { Control } from "@/shared/control"
import { xdgConfig } from "xdg-basedir"

export namespace Config {
  const ModelId = z.string()

  const log = Log.create({ service: "config" })

  // Managed settings directory for enterprise deployments (highest priority, admin-controlled)
  // These settings override all user and project settings
  function getManagedConfigDir(): string {
    switch (process.platform) {
      case "darwin":
        return "/Library/Application Support/gizzi"
      case "win32":
        return path.join(process.env.ProgramData || "C:\\ProgramData", "gizzi")
      default:
        return "/etc/gizzi"
    }
  }

  const managedConfigDir = process.env.GIZZI_TEST_MANAGED_CONFIG_DIR || getManagedConfigDir()

  // Custom merge function that concatenates array fields instead of replacing them
  function merge(target: Info, source: Info): Info {
    const merged = mergeDeep(target, source) as Info
    if (target.plugin && source.plugin) {
      merged.plugin = Array.from(new Set([...target.plugin, ...source.plugin]))
    }
    if (target.instructions && source.instructions) {
      merged.instructions = Array.from(new Set([...target.instructions, ...source.instructions]))
    }
    return merged
  }

  export const state = Instance.state(async () => {
    const auth = await Auth.all()

    // Config loading order (low -> high precedence): https://docs.gizziio.com
    // 1) Remote .well-known/gizzi (org defaults)
    // 2) Global config (~/.config/gizzi-code/gizzi.json{,c})
    // 3) Custom config (GIZZI_CONFIG)
    // 4) Project config (gizzi.json{,c})
    // 5) .gizzi directories (.gizzi/agents/, .gizzi/commands/, .gizzi/plugins/, .gizzi/gizzi.json{,c})
    // 6) Inline config (GIZZI_CONFIG_CONTENT)
    // Managed config directory is enterprise-only and always overrides everything above.
    let result: Info = {}
    for (const [key, value] of Object.entries(auth)) {
      if (value.type === "wellknown") {
        process.env[value.key] = value.token
        log.debug("fetching remote config", { url: `${key}/.well-known/gizzi` })
        let wellknown: any
        try {
          const response = await fetch(`${key}/.well-known/gizzi`, {
            signal: AbortSignal.timeout(5000),
          })
          if (!response.ok) {
            log.warn("failed to fetch remote config, skipping", { url: `${key}/.well-known/gizzi`, status: response.status })
            continue
          }
          wellknown = await response.json()
        } catch (err) {
          log.warn("failed to fetch remote config, skipping", { url: `${key}/.well-known/gizzi`, error: err })
          continue
        }
        const remoteConfig = wellknown.config ?? {}
        // Add $schema to prevent load() from trying to write back to a non-existent file
        if (!remoteConfig.$schema) remoteConfig.$schema = "https://docs.gizziio.com/config.json"
        result = merge(
          result,
          await load(JSON.stringify(remoteConfig), {
            dir: path.dirname(`${key}/.well-known/gizzi`),
            source: `${key}/.well-known/gizzi`,
          }),
        )
        log.debug("loaded remote config from well-known", { url: key })
      }
    }

    const token = await Control.token()
    if (token) {
    }

    // Global user config overrides remote config.
    result = merge(result, await global())

    // Custom config path overrides global config.
    if (Flag.GIZZI_CONFIG) {
      result = merge(result, await loadFile(Flag.GIZZI_CONFIG))
      log.debug("loaded custom config", { path: Flag.GIZZI_CONFIG })
    }

    // Project config overrides global and remote config.
    if (!Flag.GIZZI_DISABLE_PROJECT_CONFIG) {
      for (const file of ["config.toml", "gizzi.jsonc", "gizzi.json"]) {
        const found = await Filesystem.findUp(file, Instance.directory, Instance.worktree)
        for (const resolved of found.toReversed()) {
          result = merge(result, await loadFile(resolved))
        }
      }
    }

    result.agent = result.agent || {}
    result.mode = result.mode || {}
    result.plugin = result.plugin || []

    const directories = [
      Global.Path.config,
      // Only scan project .gizzi/ directories when project discovery is enabled
      ...(!Flag.GIZZI_DISABLE_PROJECT_CONFIG
        ? await Array.fromAsync(
            Filesystem.up({
              targets: [".gizzi"],
              start: Instance.directory,
              stop: Instance.worktree,
            }),
          )
        : []),
      // Always scan ~/.gizzi/ (user home directory)
      ...(await Array.fromAsync(
        Filesystem.up({
          targets: [".gizzi"],
          start: Global.Path.home,
          stop: Global.Path.home,
        }),
      )),
    ]

    // .gizzi directory config overrides (project and global) config sources.
    if (Flag.GIZZI_CONFIG_DIR) {
      directories.push(Flag.GIZZI_CONFIG_DIR)
      log.debug("loading config from GIZZI_CONFIG_DIR", { path: Flag.GIZZI_CONFIG_DIR })
    }

    const deps = []

    for (const dir of unique(directories)) {
      if (dir.endsWith(".gizzi") || dir === Flag.GIZZI_CONFIG_DIR) {
        for (const file of ["config.toml", "gizzi.jsonc", "gizzi.json"]) {
          log.debug(`loading config from ${path.join(dir, file)}`)
          result = merge(result, await loadFile(path.join(dir, file)))
          // to satisfy the type checker
          result.agent ??= {}
          result.mode ??= {}
          result.plugin ??= []
        }
      }

      // Auto-installing plugin dependencies from Config.get() can run `bun install`
      // on the startup path (and even write package.json into config dirs). That is
      // an explicit opt-in only; by default the check is skipped entirely.
      if (process.env.GIZZI_AUTO_INSTALL_DEPS === "1") {
        deps.push(
          iife(async () => {
            const shouldInstall = await needsInstall(dir)
            if (shouldInstall) await installDependencies(dir)
          }),
        )
      }

      result.command = mergeDeep(result.command ?? {}, await loadCommand(dir))
      result.agent = mergeDeep(result.agent, await loadAgent(dir))
      result.agent = mergeDeep(result.agent, await loadMode(dir))
      result.plugin.push(...(await loadPlugin(dir)))
    }

    // Local settings (user-specific, gitignored) override project config
    if (!Flag.GIZZI_DISABLE_PROJECT_CONFIG) {
      for (const localFile of ["settings.local.jsonc", "settings.local.json"]) {
        const localDirs = await Array.fromAsync(
          Filesystem.up({
            targets: [".gizzi"],
            start: Instance.directory,
            stop: Instance.worktree,
          }),
        )
        for (const dir of localDirs) {
          const localPath = path.join(dir, localFile)
          if (existsSync(localPath)) {
            result = merge(result, await loadFile(localPath))
            log.debug("loaded local settings", { path: localPath })
          }
        }
      }
    }

    // Inline config content overrides all non-managed config sources.
    if (process.env.GIZZI_CONFIG_CONTENT) {
      result = merge(
        result,
        await load(process.env.GIZZI_CONFIG_CONTENT, {
          dir: Instance.directory,
          source: "GIZZI_CONFIG_CONTENT",
        }),
      )
      log.debug("loaded custom config from GIZZI_CONFIG_CONTENT")
    }

    // Load managed config files last (highest priority) - enterprise admin-controlled
    // Kept separate from directories array to avoid write operations when installing plugins
    // which would fail on system directories requiring elevated permissions
    // This way it only loads config file and not skills/plugins/commands
    if (existsSync(managedConfigDir)) {
      for (const file of ["gizzi.jsonc", "gizzi.json"]) {
        result = merge(result, await loadFile(path.join(managedConfigDir, file)))
      }
    }

    // Migrate deprecated mode field to agent field
    for (const [name, mode] of Object.entries(result.mode ?? {})) {
      result.agent = mergeDeep(result.agent ?? {}, {
        [name]: {
          ...mode,
          mode: "primary" as const,
        },
      })
    }

    if (Flag.GIZZI_PERMISSION) {
      result.permission = mergeDeep(result.permission ?? {}, JSON.parse(Flag.GIZZI_PERMISSION))
    }

    // Backwards compatibility: legacy top-level `tools` config
    if (result.tools) {
      const perms: Record<string, Config.PermissionAction> = {}
      for (const [tool, enabled] of Object.entries(result.tools)) {
        const action: Config.PermissionAction = enabled ? "allow" : "deny"
        if (tool === "write" || tool === "edit" || tool === "patch" || tool === "multiedit") {
          perms.edit = action
          continue
        }
        perms[tool] = action
      }
      result.permission = mergeDeep(perms, result.permission ?? {})
    }

    // Apply sandbox mode presets as defaults; explicit enabled/allow_network always win.
    if (result.sandbox?.mode) {
      const presets: Record<string, { enabled: boolean; allow_network: boolean }> = {
        "read-only": { enabled: true, allow_network: false },
        "workspace-write": { enabled: true, allow_network: false },
        "danger-full-access": { enabled: false, allow_network: true },
      }
      const preset = presets[result.sandbox.mode]
      if (preset) {
        result.sandbox = {
          enabled: preset.enabled,
          allow_network: preset.allow_network,
          ...result.sandbox,
        }
      }
    }

    // Apply approval_policy as defaults for the permission system; explicit `permission` rules always win.
    if (result.approval_policy) {
      const policy = result.approval_policy
      const modeDefaults: Record<string, Config.PermissionAction> = {
        untrusted: "ask",
        "on-request": "ask",
        "on-failure": "allow",
        never: "allow",
      }
      const defaultAction = policy.mode ? modeDefaults[policy.mode] : undefined
      if (defaultAction) {
        result.permission = mergeDeep({ "*": defaultAction }, result.permission ?? {})
      }
      if (policy.granular?.rules) {
        result.permission = mergeDeep(policy.granular.rules, result.permission ?? {})
      }
      if (policy.granular?.skill_approval !== undefined) {
        result.permission = mergeDeep(
          { skill: policy.granular.skill_approval ? "ask" : "allow" },
          result.permission ?? {},
        )
      }
      if (policy.granular?.web_search !== undefined) {
        result.permission = mergeDeep(
          { websearch: policy.granular.web_search === "live" ? "allow" : "deny" },
          result.permission ?? {},
        )
      }
      if (policy.granular?.sandbox_approval !== undefined) {
        result.permission = mergeDeep(
          { bash: policy.granular.sandbox_approval ? "ask" : "allow" },
          result.permission ?? {},
        )
      }
    }

    // Apply the active named permission profile as defaults; explicit `permission` rules always win.
    if (result.permission_profiles?.active_profile) {
      const activeProfile = result.permission_profiles.profiles?.[result.permission_profiles.active_profile]
      if (activeProfile?.rules) {
        result.permission = mergeDeep(activeProfile.rules, result.permission ?? {})
      }
    }

    if (!result.username) result.username = os.userInfo().username

    // Handle migration from autoshare to share field
    if (result.autoshare === true && !result.share) {
      result.share = "auto"
    }

    if (!result.keybinds) result.keybinds = Info.shape.keybinds.parse({})

    // Apply flag overrides for compaction settings
    if (Flag.GIZZI_DISABLE_AUTOCOMPACT) {
      result.compaction = { ...result.compaction, auto: false }
    }
    if (Flag.GIZZI_DISABLE_PRUNE) {
      result.compaction = { ...result.compaction, prune: false }
    }

    result.plugin = deduplicatePlugins(result.plugin ?? [])

    return {
      config: result,
      directories,
      deps,
    }
  })

  export async function waitForDependencies() {
    const deps = await state().then((x) => x.deps)
    await Promise.all(deps)
  }

  export async function installDependencies(dir: string) {
    const pkg = path.join(dir, "package.json")
    const targetVersion = pluginDependencyVersion()

    const json = await Filesystem.readJson<{ dependencies?: Record<string, string> }>(pkg).catch(() => ({
      dependencies: {},
    }))
    json.dependencies = {
      ...json.dependencies,
      "@allternit/plugin": targetVersion,
    }
    delete json.dependencies["@allternit/gizzi-sdk"]
    delete json.dependencies["@allternit/plugin"]
    await Filesystem.writeJson(pkg, json)
    await new Promise((resolve) => setTimeout(resolve, 3000))

    const gitignore = path.join(dir, ".gitignore")
    const hasGitIgnore = await Filesystem.exists(gitignore)
    if (!hasGitIgnore)
      await Filesystem.write(gitignore, ["node_modules", "package.json", "bun.lock", ".gitignore"].join("\n"))

    // Install any additional dependencies defined in the package.json
    // This allows local plugins and custom tools to use external packages
    await BunProc.run(
      [
        "install",
        // Workaround: Bun cache breaks with proxied registries (github.com/oven-sh/bun/issues/19936)
        ...(proxied() ? ["--no-cache"] : []),
      ],
      { cwd: dir },
    ).catch((err) => {
      log.warn("failed to install dependencies", { dir, error: err })
    })
  }

  async function isWritable(dir: string) {
    try {
      await fs.access(dir, constants.W_OK)
      return true
    } catch {
      return false
    }
  }

  async function needsInstall(dir: string) {
    // Some config dirs may be read-only.
    // Installing deps there will fail; skip installation in that case.
    const writable = await isWritable(dir)
    if (!writable) {
      log.debug("config dir is not writable, skipping dependency install", { dir })
      return false
    }

    const nodeModules = path.join(dir, "node_modules")
    if (!existsSync(nodeModules)) return true

    const pkg = path.join(dir, "package.json")
    const pkgExists = await Filesystem.exists(pkg)
    if (!pkgExists) return true

    const parsed = await Filesystem.readJson<{ dependencies?: Record<string, string> }>(pkg).catch(() => null)
    const dependencies = parsed?.dependencies ?? {}
    const depVersion = dependencies["@allternit/plugin"]
    const deprecatedSdkVersion = dependencies["@allternit/gizzi-sdk"]
    const legacyPluginVersion = dependencies["@allternit/plugin"]
    if (!depVersion) return true
    if (deprecatedSdkVersion || legacyPluginVersion) return true

    const targetVersion = pluginDependencyVersion()
    if (targetVersion === "latest") {
      // Skip npm registry check for workspace packages - they don't exist on npm
      // and will always return 404, causing unnecessary reinstalls
      if (depVersion === "*" || depVersion === "workspace:*" || depVersion.startsWith("file:")) {
        return false
      }
      const isOutdated = await PackageRegistry.isOutdated("@allternit/plugin", depVersion, dir)
      if (!isOutdated) return false
      log.info("Cached version is outdated, proceeding with install", {
        pkg: "@allternit/plugin",
        cachedVersion: depVersion,
      })
      return true
    }
    if (depVersion === targetVersion) return false
    return true
  }

  function pluginDependencyVersion() {
    const currentDir = process.cwd()
    const searchRoots = [currentDir]
    let cursor = currentDir

    for (let i = 0; i < 6; i++) {
      const parent = path.dirname(cursor)
      if (parent === cursor) break
      searchRoots.push(parent)
      cursor = parent
    }

    for (const root of searchRoots) {
      const candidates = [
        path.resolve(root, "packages/plugin"),
        path.resolve(root, "cmd/gizzi-code/packages/plugin"),
      ]

      for (const candidate of candidates) {
        if (existsSync(candidate)) {
          return `file:${candidate}`
        }
      }
    }

    return Installation.isLocal() ? "workspace:*" : Installation.VERSION
  }

  function rel(item: string, patterns: string[]) {
    for (const pattern of patterns) {
      const index = item.indexOf(pattern)
      if (index === -1) continue
      return item.slice(index + pattern.length)
    }
  }

  function trim(file: string) {
    const ext = path.extname(file)
    return ext.length ? file.slice(0, -ext.length) : file
  }

  async function loadCommand(dir: string) {
    const result: Record<string, Command> = {}
    for (const item of await Glob.scan("{command,commands}/**/*.md", {
      cwd: dir,
      absolute: true,
      dot: true,
      symlink: true,
    })) {
      const md = await ConfigMarkdown.parse(item).catch(async (err) => {
        const message = ConfigMarkdown.FrontmatterError.isInstance(err)
          ? (err.data as { message?: string } | undefined)?.message
          : `Failed to parse command ${item}`
        const { Session } = await import("@/runtime/session")
        Bus.publish(Session.Event.Error, { error: { name: "Unknown", message: message ?? "Unknown error", data: {} } })
        log.error("failed to load command", { command: item, err })
        return undefined
      })
      if (!md) continue

      const patterns = ["/.gizzi/command/", "/.gizzi/commands/", "/command/", "/commands/"]
      const file = rel(item, patterns) ?? path.basename(item)
      const name = trim(file)

      const config = {
        name,
        ...md.data,
        template: md.content.trim(),
      }
      const parsed = Command.safeParse(config)
      if (parsed.success) {
        result[config.name] = parsed.data
        continue
      }
      throw new InvalidError({ path: item, issues: parsed.error.issues }, { cause: parsed.error })
    }
    return result
  }

  async function loadAgent(dir: string) {
    const result: Record<string, Agent> = {}

    for (const item of await Glob.scan("{agent,agents}/**/*.md", {
      cwd: dir,
      absolute: true,
      dot: true,
      symlink: true,
    })) {
      const md = await ConfigMarkdown.parse(item).catch(async (err) => {
        const message = ConfigMarkdown.FrontmatterError.isInstance(err)
          ? (err.data as { message?: string } | undefined)?.message
          : `Failed to parse agent ${item}`
        const { Session } = await import("@/runtime/session")
        Bus.publish(Session.Event.Error, { error: { name: "Unknown", message: message ?? "Unknown error", data: {} } })
        log.error("failed to load agent", { agent: item, err })
        return undefined
      })
      if (!md) continue

      const patterns = ["/.gizzi/agent/", "/.gizzi/agents/", "/agent/", "/agents/"]
      const file = rel(item, patterns) ?? path.basename(item)
      const agentName = trim(file)

      const config = {
        name: agentName,
        ...md.data,
        prompt: md.content.trim(),
      }
      const parsed = Agent.safeParse(config)
      if (parsed.success) {
        result[config.name] = parsed.data
        continue
      }
      throw new InvalidError({ path: item, issues: parsed.error.issues }, { cause: parsed.error })
    }
    return result
  }

  async function loadMode(dir: string) {
    const result: Record<string, Agent> = {}
    for (const item of await Glob.scan("{mode,modes}/*.md", {
      cwd: dir,
      absolute: true,
      dot: true,
      symlink: true,
    })) {
      const md = await ConfigMarkdown.parse(item).catch(async (err) => {
        const message = ConfigMarkdown.FrontmatterError.isInstance(err)
          ? (err.data as { message?: string } | undefined)?.message
          : `Failed to parse mode ${item}`
        const { Session } = await import("@/runtime/session")
        Bus.publish(Session.Event.Error, { error: { name: "Unknown", message: message ?? "Unknown error", data: {} } })
        log.error("failed to load mode", { mode: item, err })
        return undefined
      })
      if (!md) continue

      const config = {
        name: path.basename(item, ".md"),
        ...md.data,
        prompt: md.content.trim(),
      }
      const parsed = Agent.safeParse(config)
      if (parsed.success) {
        result[config.name] = {
          ...parsed.data,
          mode: "primary" as const,
        }
        continue
      }
    }
    return result
  }

  async function loadPlugin(dir: string) {
    const plugins: string[] = []

    for (const item of await Glob.scan("{plugin,plugins}/*.{ts,js}", {
      cwd: dir,
      absolute: true,
      dot: true,
      symlink: true,
    })) {
      plugins.push(pathToFileURL(item).href)
    }
    return plugins
  }

  /**
   * Extracts a canonical plugin name from a plugin specifier.
   * - For file:// URLs: extracts filename without extension
   * - For npm packages: extracts package name without version
   *
   * @example
   * getPluginName("file:///path/to/plugin/foo.js") // "foo"
   * getPluginName("oh-my-gizzi@2.4.3") // "oh-my-gizzi"
   * getPluginName("@scope/pkg@1.0.0") // "@scope/pkg"
   */
  export function getPluginName(plugin: string): string {
    if (plugin.startsWith("file://")) {
      return path.parse(new URL(plugin).pathname).name
    }
    const lastAt = plugin.lastIndexOf("@")
    if (lastAt > 0) {
      return plugin.substring(0, lastAt)
    }
    return plugin
  }

  /**
   * Deduplicates plugins by name, with later entries (higher priority) winning.
   * Priority order (highest to lowest):
   * 1. Local plugin/ directory
   * 2. Local gizzi.json
   * 3. Global plugin/ directory
   * 4. Global gizzi.json
   *
   * Since plugins are added in low-to-high priority order,
   * we reverse, deduplicate (keeping first occurrence), then restore order.
   */
  export function deduplicatePlugins(plugins: string[]): string[] {
    // seenNames: canonical plugin names for duplicate detection
    // e.g., "oh-my-gizzi", "@scope/pkg"
    const seenNames = new Set<string>()

    // uniqueSpecifiers: full plugin specifiers to return
    // e.g., "oh-my-gizzi@2.4.3", "file:///path/to/plugin.js"
    const uniqueSpecifiers: string[] = []

    for (const specifier of plugins.toReversed()) {
      const name = getPluginName(specifier)
      if (!seenNames.has(name)) {
        seenNames.add(name)
        uniqueSpecifiers.push(specifier)
      }
    }

    return uniqueSpecifiers.toReversed()
  }

  export const McpLocal = z
    .object({
      type: z.literal("local").describe("Type of MCP server connection"),
      command: z.string().array().describe("Command and arguments to run the MCP server"),
      environment: z
        .record(z.string(), z.string())
        .optional()
        .describe("Environment variables to set when running the MCP server"),
      enabled: z.boolean().optional().describe("Enable or disable the MCP server on startup"),
      timeout: z
        .number()
        .int()
        .positive()
        .optional()
        .describe("Timeout in ms for MCP server requests. Defaults to 5000 (5 seconds) if not specified."),
    })
    .strict()
    

  export const McpOAuth = z
    .object({
      clientId: z
        .string()
        .optional()
        .describe("OAuth client ID. If not provided, dynamic client registration (RFC 7591) will be attempted."),
      clientSecret: z.string().optional().describe("OAuth client secret (if required by the authorization server)"),
      scope: z.string().optional().describe("OAuth scopes to request during authorization"),
    })
    .strict()
    
  export type McpOAuth = z.infer<typeof McpOAuth>

  export const McpRemote = z
    .object({
      type: z.literal("remote").describe("Type of MCP server connection"),
      url: z.string().describe("URL of the remote MCP server"),
      enabled: z.boolean().optional().describe("Enable or disable the MCP server on startup"),
      headers: z.record(z.string(), z.string()).optional().describe("Headers to send with the request"),
      oauth: z
        .union([McpOAuth, z.literal(false)])
        .optional()
        .describe(
          "OAuth authentication configuration for the MCP server. Set to false to disable OAuth auto-detection.",
        ),
      timeout: z
        .number()
        .int()
        .positive()
        .optional()
        .describe("Timeout in ms for MCP server requests. Defaults to 5000 (5 seconds) if not specified."),
    })
    .strict()
    

  export const Mcp = z.discriminatedUnion("type", [McpLocal, McpRemote])
  export type Mcp = z.infer<typeof Mcp>

  export const PermissionAction = z.enum(["ask", "allow", "deny"])
  export type PermissionAction = z.infer<typeof PermissionAction>

  export const PermissionObject = z.record(z.string(), PermissionAction)
  export type PermissionObject = z.infer<typeof PermissionObject>

  export const PermissionRule = z.union([PermissionAction, PermissionObject])
  export type PermissionRule = z.infer<typeof PermissionRule>

  // Capture original key order before zod reorders, then rebuild in original order
  const permissionPreprocess = (val: unknown) => {
    if (typeof val === "object" && val !== null && !Array.isArray(val)) {
      return { __originalKeys: Object.keys(val), ...val }
    }
    return val
  }

  const permissionTransform = (x: unknown): Record<string, PermissionRule> => {
    if (typeof x === "string") return { "*": x as PermissionAction }
    const obj = x as { __originalKeys?: string[] } & Record<string, unknown>
    const { __originalKeys, ...rest } = obj
    if (!__originalKeys) return rest as Record<string, PermissionRule>
    const result: Record<string, PermissionRule> = {}
    for (const key of __originalKeys) {
      if (key in rest) result[key] = rest[key] as PermissionRule
    }
    return result
  }

  export const Permission = z
    .preprocess(
      permissionPreprocess,
      z
        .object({
          __originalKeys: z.string().array().optional(),
          read: PermissionRule.optional(),
          edit: PermissionRule.optional(),
          glob: PermissionRule.optional(),
          grep: PermissionRule.optional(),
          list: PermissionRule.optional(),
          bash: PermissionRule.optional(),
          task: PermissionRule.optional(),
          external_directory: PermissionRule.optional(),
          todowrite: PermissionAction.optional(),
          todoread: PermissionAction.optional(),
          question: PermissionAction.optional(),
          webfetch: PermissionAction.optional(),
          websearch: PermissionAction.optional(),
          codesearch: PermissionAction.optional(),
          lsp: PermissionRule.optional(),
          doom_loop: PermissionAction.optional(),
          skill: PermissionRule.optional(),
        })
        .catchall(PermissionRule)
        .or(PermissionAction),
    )
    .transform(permissionTransform)

  export type Permission = z.infer<typeof Permission>

  export const ApprovalPolicyMode = z.enum(["untrusted", "on-failure", "on-request", "never", "granular"])
  export type ApprovalPolicyMode = z.infer<typeof ApprovalPolicyMode>

  export const WebSearchApproval = z.enum(["disabled", "live"])
  export type WebSearchApproval = z.infer<typeof WebSearchApproval>

  export const ApprovalPolicy = z
    .object({
      mode: ApprovalPolicyMode.optional().describe(
        "Approval policy preset. 'granular' defers to the 'granular' block for per-category rules.",
      ),
      granular: z
        .object({
          rules: Permission.optional().describe("Per-tool approval rules, same shape as the top-level 'permission' field"),
          sandbox_approval: z.boolean().optional().describe("Require approval before sandboxed command execution"),
          skill_approval: z.boolean().optional().describe("Require approval before running skills"),
          web_search: WebSearchApproval.optional().describe("Web search approval mode: 'disabled' or 'live'"),
        })
        .optional()
        .describe("Granular approval rules, used when mode is 'granular' (or as overrides alongside any mode)"),
    })
    .describe("Approval policy applied as defaults to the permission system on load")
  export type ApprovalPolicy = z.infer<typeof ApprovalPolicy>

  export const Command = z.object({
    template: z.string(),
    description: z.string().optional(),
    agent: z.string().optional(),
    model: ModelId.optional(),
    subtask: z.boolean().optional(),
  })
  export type Command = z.infer<typeof Command>

  export const Skills = z.object({
    paths: z.array(z.string()).optional().describe("Additional paths to skill folders"),
    urls: z
      .array(z.string())
      .optional()
      .describe("URLs to fetch skills from (e.g., https://example.com/.well-known/skills/)"),
  })
  export type Skills = z.infer<typeof Skills>

  export const Agent = z
    .object({
      model: ModelId.optional(),
      variant: z
        .string()
        .optional()
        .describe("Default model variant for this agent (applies only when using the agent's configured model)."),
      temperature: z.number().optional(),
      top_p: z.number().optional(),
      prompt: z.string().optional(),
      tools: z.record(z.string(), z.boolean()).optional().describe("@deprecated Use 'permission' field instead"),
      disable: z.boolean().optional(),
      description: z.string().optional().describe("Description of when to use the agent"),
      mode: z.enum(["subagent", "primary", "all"]).optional(),
      hidden: z
        .boolean()
        .optional()
        .describe("Hide this subagent from the @ autocomplete menu (default: false, only applies to mode: subagent)"),
      options: z.record(z.string(), z.any()).optional(),
      color: z
        .union([
          z.string().regex(/^#[0-9a-fA-F]{6}$/, "Invalid hex color format"),
          z.enum(["primary", "secondary", "accent", "success", "warning", "error", "info"]),
        ])
        .optional()
        .describe("Hex color code (e.g., #FF5733) or theme color (e.g., primary)"),
      steps: z
        .number()
        .int()
        .positive()
        .optional()
        .describe("Maximum number of agentic iterations before forcing text-only response"),
      summary_policy: z
        .object({
          min_chars: z.number().int().nonnegative(),
          continuation_prompt: z.string().min(1),
          retries: z.number().int().min(0).max(3),
        })
        .optional()
        .describe("Require a substantive subagent handoff, with bounded continuation turns when too brief."),
      maxSteps: z.number().int().positive().optional().describe("@deprecated Use 'steps' field instead."),
      permission: Permission.optional(),
      harness: z
        .object({
          mode: z.enum(["byok", "cloud", "local", "subprocess"]),
          byok: z
            .object({
              keys: z
                .object({
                  anthropic: z.string().optional(),
                  openai: z.string().optional(),
                  google: z.string().optional(),
                })
                .optional(),
              baseURLs: z
                .object({
                  anthropic: z.string().optional(),
                  openai: z.string().optional(),
                  google: z.string().optional(),
                })
                .optional(),
            })
            .optional(),
          cloud: z
            .object({
              baseURL: z.string(),
              accessToken: z.string(),
              refreshToken: z.string().optional(),
            })
            .optional(),
          local: z
            .object({
              baseURL: z.string(),
            })
            .optional(),
          subprocess: z
            .object({
              command: z.string(),
              cwd: z.string().optional(),
              env: z.record(z.string(), z.string()).optional(),
            })
            .optional(),
        })
        .optional()
        .describe("Per-agent AI routing harness (BYOK / cloud / local / subprocess)"),
    })
    .catchall(z.any())
    .transform((agent, ctx) => {
      const knownKeys = new Set([
        "name",
        "model",
        "variant",
        "prompt",
        "description",
        "temperature",
        "top_p",
        "mode",
        "hidden",
        "color",
        "steps",
        "maxSteps",
        "options",
        "permission",
        "disable",
        "tools",
        "harness",
      ])

      // Extract unknown properties into options
      const options: Record<string, unknown> = { ...agent.options }
      for (const [key, value] of Object.entries(agent)) {
        if (!knownKeys.has(key)) options[key] = value
      }

      // Convert legacy tools config to permissions
      const permission: Permission = {}
      for (const [tool, enabled] of Object.entries(agent.tools ?? {})) {
        const action = enabled ? "allow" : "deny"
        // write, edit, patch, multiedit all map to edit permission
        if (tool === "write" || tool === "edit" || tool === "patch" || tool === "multiedit") {
          permission.edit = action
        } else {
          permission[tool] = action
        }
      }
      Object.assign(permission, agent.permission)

      // Convert legacy maxSteps to steps
      const steps = agent.steps ?? agent.maxSteps

      return { ...agent, options, permission, steps } as typeof agent & {
        options?: Record<string, unknown>
        permission?: Permission
        steps?: number
      }
    })
    
  export type Agent = z.infer<typeof Agent>

  export const Keybinds = z
    .object({
      leader: z.string().optional().default("ctrl+x").describe("Leader key for keybind combinations"),
      app_exit: z.string().optional().default("ctrl+c,ctrl+d,<leader>q").describe("Exit the application"),
      editor_open: z.string().optional().default("<leader>e").describe("Open external editor"),
      theme_list: z.string().optional().default("<leader>t").describe("List available themes"),
      sidebar_toggle: z.string().optional().default("<leader>b").describe("Toggle sidebar"),
      scrollbar_toggle: z.string().optional().default("none").describe("Toggle session scrollbar"),
      username_toggle: z.string().optional().default("none").describe("Toggle username visibility"),
      status_view: z.string().optional().default("<leader>s").describe("View status"),
      session_export: z.string().optional().default("<leader>x").describe("Export session to editor"),
      session_new: z.string().optional().default("<leader>n").describe("Create a new session"),
      session_list: z.string().optional().default("<leader>l").describe("List all sessions"),
      session_timeline: z.string().optional().default("<leader>g").describe("Show session timeline"),
      session_fork: z.string().optional().default("none").describe("Fork session from message"),
      session_rename: z.string().optional().default("ctrl+r").describe("Rename session"),
      session_delete: z.string().optional().default("ctrl+d").describe("Delete session"),
      stash_delete: z.string().optional().default("ctrl+d").describe("Delete stash entry"),
      model_provider_list: z.string().optional().default("ctrl+a").describe("Open provider list from model dialog"),
      model_favorite_toggle: z.string().optional().default("ctrl+f").describe("Toggle model favorite status"),
      session_share: z.string().optional().default("none").describe("Share current session"),
      session_unshare: z.string().optional().default("none").describe("Unshare current session"),
      session_interrupt: z.string().optional().default("escape").describe("Interrupt current session"),
      session_compact: z.string().optional().default("<leader>c").describe("Compact the session"),
      messages_page_up: z.string().optional().default("pageup,ctrl+alt+b").describe("Scroll messages up by one page"),
      messages_page_down: z
        .string()
        .optional()
        .default("pagedown,ctrl+alt+f")
        .describe("Scroll messages down by one page"),
      messages_line_up: z.string().optional().default("ctrl+alt+y").describe("Scroll messages up by one line"),
      messages_line_down: z.string().optional().default("ctrl+alt+e").describe("Scroll messages down by one line"),
      messages_half_page_up: z.string().optional().default("ctrl+alt+u").describe("Scroll messages up by half page"),
      messages_half_page_down: z
        .string()
        .optional()
        .default("ctrl+alt+d")
        .describe("Scroll messages down by half page"),
      messages_first: z.string().optional().default("ctrl+g,home").describe("Navigate to first message"),
      messages_last: z.string().optional().default("ctrl+alt+g,end").describe("Navigate to last message"),
      messages_next: z.string().optional().default("none").describe("Navigate to next message"),
      messages_previous: z.string().optional().default("none").describe("Navigate to previous message"),
      messages_last_user: z.string().optional().default("none").describe("Navigate to last user message"),
      messages_copy: z.string().optional().default("<leader>y").describe("Copy message"),
      messages_undo: z.string().optional().default("<leader>u").describe("Undo message"),
      messages_redo: z.string().optional().default("<leader>r").describe("Redo message"),
      messages_toggle_conceal: z
        .string()
        .optional()
        .default("<leader>h")
        .describe("Toggle code block concealment in messages"),
      tool_details: z.string().optional().default("none").describe("Toggle tool details visibility"),
      model_list: z.string().optional().default("<leader>m").describe("List available models"),
      model_cycle_recent: z.string().optional().default("f2").describe("Next recently used model"),
      model_cycle_recent_reverse: z.string().optional().default("shift+f2").describe("Previous recently used model"),
      model_cycle_favorite: z.string().optional().default("none").describe("Next favorite model"),
      model_cycle_favorite_reverse: z.string().optional().default("none").describe("Previous favorite model"),
      command_list: z.string().optional().default("ctrl+p").describe("List available commands"),
      agent_list: z.string().optional().default("<leader>a").describe("List agents"),
      agent_cycle: z.string().optional().default("tab").describe("Next agent"),
      agent_cycle_reverse: z.string().optional().default("shift+tab").describe("Previous agent"),
      variant_cycle: z.string().optional().default("ctrl+t").describe("Cycle model variants"),
      input_clear: z.string().optional().default("ctrl+c").describe("Clear input field"),
      input_paste: z.string().optional().default("ctrl+v").describe("Paste from clipboard"),
      input_submit: z.string().optional().default("return").describe("Submit input"),
      input_newline: z
        .string()
        .optional()
        .default("shift+return,ctrl+return,alt+return,ctrl+j")
        .describe("Insert newline in input"),
      input_move_left: z.string().optional().default("left,ctrl+b").describe("Move cursor left in input"),
      input_move_right: z.string().optional().default("right,ctrl+f").describe("Move cursor right in input"),
      input_move_up: z.string().optional().default("up").describe("Move cursor up in input"),
      input_move_down: z.string().optional().default("down").describe("Move cursor down in input"),
      input_select_left: z.string().optional().default("shift+left").describe("Select left in input"),
      input_select_right: z.string().optional().default("shift+right").describe("Select right in input"),
      input_select_up: z.string().optional().default("shift+up").describe("Select up in input"),
      input_select_down: z.string().optional().default("shift+down").describe("Select down in input"),
      input_line_home: z.string().optional().default("ctrl+a").describe("Move to start of line in input"),
      input_line_end: z.string().optional().default("ctrl+e").describe("Move to end of line in input"),
      input_select_line_home: z
        .string()
        .optional()
        .default("ctrl+shift+a")
        .describe("Select to start of line in input"),
      input_select_line_end: z.string().optional().default("ctrl+shift+e").describe("Select to end of line in input"),
      input_visual_line_home: z.string().optional().default("alt+a").describe("Move to start of visual line in input"),
      input_visual_line_end: z.string().optional().default("alt+e").describe("Move to end of visual line in input"),
      input_select_visual_line_home: z
        .string()
        .optional()
        .default("alt+shift+a")
        .describe("Select to start of visual line in input"),
      input_select_visual_line_end: z
        .string()
        .optional()
        .default("alt+shift+e")
        .describe("Select to end of visual line in input"),
      input_buffer_home: z.string().optional().default("home").describe("Move to start of buffer in input"),
      input_buffer_end: z.string().optional().default("end").describe("Move to end of buffer in input"),
      input_select_buffer_home: z
        .string()
        .optional()
        .default("shift+home")
        .describe("Select to start of buffer in input"),
      input_select_buffer_end: z.string().optional().default("shift+end").describe("Select to end of buffer in input"),
      input_delete_line: z.string().optional().default("ctrl+shift+d").describe("Delete line in input"),
      input_delete_to_line_end: z.string().optional().default("ctrl+k").describe("Delete to end of line in input"),
      input_delete_to_line_start: z.string().optional().default("ctrl+u").describe("Delete to start of line in input"),
      input_backspace: z.string().optional().default("backspace,shift+backspace").describe("Backspace in input"),
      input_delete: z.string().optional().default("ctrl+d,delete,shift+delete").describe("Delete character in input"),
      input_undo: z.string().optional().default("ctrl+-,super+z").describe("Undo in input"),
      input_redo: z.string().optional().default("ctrl+.,super+shift+z").describe("Redo in input"),
      input_word_forward: z
        .string()
        .optional()
        .default("alt+f,alt+right,ctrl+right")
        .describe("Move word forward in input"),
      input_word_backward: z
        .string()
        .optional()
        .default("alt+b,alt+left,ctrl+left")
        .describe("Move word backward in input"),
      input_select_word_forward: z
        .string()
        .optional()
        .default("alt+shift+f,alt+shift+right")
        .describe("Select word forward in input"),
      input_select_word_backward: z
        .string()
        .optional()
        .default("alt+shift+b,alt+shift+left")
        .describe("Select word backward in input"),
      input_delete_word_forward: z
        .string()
        .optional()
        .default("alt+d,alt+delete,ctrl+delete")
        .describe("Delete word forward in input"),
      input_delete_word_backward: z
        .string()
        .optional()
        .default("ctrl+w,ctrl+backspace,alt+backspace")
        .describe("Delete word backward in input"),
      history_previous: z.string().optional().default("up").describe("Previous history item"),
      history_next: z.string().optional().default("down").describe("Next history item"),
      session_child_cycle: z.string().optional().default("<leader>right").describe("Next child session"),
      session_child_cycle_reverse: z.string().optional().default("<leader>left").describe("Previous child session"),
      session_parent: z.string().optional().default("<leader>up").describe("Go to parent session"),
      terminal_suspend: z.string().optional().default("ctrl+z").describe("Suspend terminal"),
      terminal_title_toggle: z.string().optional().default("none").describe("Toggle terminal title"),
      tips_toggle: z.string().optional().default("<leader>h").describe("Toggle tips on home screen"),
      display_thinking: z.string().optional().default("none").describe("Toggle thinking blocks visibility"),
      display_runtime_trace: z.string().optional().default("none").describe("Toggle runtime trace visibility"),
      display_receipts: z.string().optional().default("none").describe("Toggle receipt thread visibility"),
      display_cards: z.string().optional().default("none").describe("Toggle runtime preview cards visibility"),
      display_lane_history: z
        .string()
        .optional()
        .default("<leader>k")
        .describe("Toggle runtime lane history visibility"),
      runtime_focus_mode: z.string().optional().default("none").describe("Toggle runtime focus mode"),
    })
    .strict()
    

  export const TUI = z.object({
    scroll_speed: z.number().min(0.001).optional().describe("TUI scroll speed"),
    scroll_acceleration: z
      .object({
        enabled: z.boolean().describe("Enable scroll acceleration"),
      })
      .optional()
      .describe("Scroll acceleration settings"),
    diff_style: z
      .enum(["auto", "stacked"])
      .optional()
      .describe("Control diff rendering style: 'auto' adapts to terminal width, 'stacked' always shows single column"),
  })

  export const Server = z
    .object({
      port: z.number().int().positive().optional().describe("Port to listen on"),
      hostname: z.string().optional().describe("Hostname to listen on"),
      mdns: z.boolean().optional().describe("Enable mDNS service discovery"),
      mdnsDomain: z.string().optional().describe("Custom domain name for mDNS service (default: gizzi.local)"),
      cors: z.array(z.string()).optional().describe("Additional domains to allow for CORS"),
      tunnel: z.boolean().optional().describe("Expose the server through a cloudflared quick tunnel when serving"),
      tunnel_token: z
        .string()
        .optional()
        .describe("Cloudflare named-tunnel token; serves through `cloudflared tunnel run --token` instead of a quick tunnel"),
      tunnel_hostname: z
        .string()
        .optional()
        .describe("Public hostname mapped to the named tunnel (used for logs and instance registration)"),
      mesh: z.boolean().optional().describe("Join the Allternit mesh tailnet (Tailscale/Headscale) when serving"),
      mesh_auth_key: z
        .string()
        .optional()
        .describe(
          "Tailscale/Headscale preauth key for the mesh join (secret — prefer the GIZZI_MESH_AUTH_KEY env var over storing it in a config file)",
        ),
      mesh_control_url: z
        .string()
        .optional()
        .describe("Headscale coordination server URL (default https://mail.news.allternit.com:8444)"),
    })
    .strict()
    

  export const Layout = z.enum(["auto", "stretch"])
  export type Layout = z.infer<typeof Layout>

  export const Provider = ModelsDev.Provider.partial()
    .extend({
      whitelist: z.array(z.string()).optional(),
      blacklist: z.array(z.string()).optional(),
      // Auth mode for LLM config forwarding to the computer-use operator:
      //   "api_key"    — standard key-based auth (default for all API providers)
      //   "none"       — local model, no auth needed (Ollama, LM Studio, vLLM)
      //   "bearer"     — subscription / OAuth token (Authorization: Bearer)
      //   "subprocess" — CLI tool already authed in OS (claude -p, llm, aichat)
      auth_type: z.enum(["api_key", "none", "bearer", "subprocess"]).optional(),
      token: z.string().optional().describe("Bearer token for auth_type: bearer"),
      subprocess_cmd: z.string().optional().describe("CLI command for auth_type: subprocess, e.g. 'claude -p'"),
      runtime: z.enum(["sdk", "subprocess", "auto"]).optional().describe("Execution runtime for this provider's models"),
      models: z
        .record(
          z.string(),
          ModelsDev.Model.partial().extend({
            runtime: z.enum(["sdk", "subprocess", "auto"]).optional().describe("Execution runtime for this model"),
            variants: z
              .record(
                z.string(),
                z
                  .object({
                    disabled: z.boolean().optional().describe("Disable this variant for the model"),
                  })
                  .catchall(z.any()),
              )
              .optional()
              .describe("Variant-specific configuration"),
          }),
        )
        .optional(),
      options: z
        .object({
          apiKey: z.string().optional(),
          baseURL: z.string().optional(),
          enterpriseUrl: z.string().optional().describe("GitHub Enterprise URL for authentication"),
          setCacheKey: z.boolean().optional().describe("Enable promptCacheKey for this provider (default false)"),
          timeout: z
            .union([
              z
                .number()
                .int()
                .positive()
                .describe(
                  "Timeout in milliseconds for requests to this provider. Default is 300000 (5 minutes). Set to false to disable timeout.",
                ),
              z.literal(false).describe("Disable timeout for this provider entirely."),
            ])
            .optional()
            .describe(
              "Timeout in milliseconds for requests to this provider. Default is 300000 (5 minutes). Set to false to disable timeout.",
            ),
        })
        .catchall(z.any())
        .optional(),
    })
    .strict()
    
  export type Provider = z.infer<typeof Provider>

  export const Info = z
    .object({
      $schema: z.string().optional().describe("JSON schema reference for configuration validation"),
      theme: z.string().optional().describe("Theme name to use for the interface"),
      keybinds: Keybinds.optional().describe("Custom keybind configurations"),
      logLevel: Log.Level.optional().describe("Log level"),
      tui: TUI.optional().describe("TUI specific settings"),
      browser: z
        .object({
          preferred_adapter: z
            .enum(["extension-tab", "local-playwright", "browser-use", "stagehand", "playwright", "cdp", "desktop"])
            .optional()
            .describe("Default browser-tool adapter when a call names none (Settings → Cowork → Preferred browser)"),
        })
        .optional()
        .describe("Browser tool defaults"),
      server: Server.optional().describe("Server configuration for gizzi serve and web commands"),
      command: z
        .record(z.string(), Command)
        .optional()
        .describe("Command configuration, see https://gizzi.sh/docs/commands"),
      skills: Skills.optional().describe("Additional skill folder paths"),
      watcher: z
        .object({
          ignore: z.array(z.string()).optional(),
        })
        .optional(),
      plugin: z.string().array().optional(),
      snapshot: z.boolean().optional(),
      share: z
        .enum(["manual", "auto", "disabled"])
        .optional()
        .describe(
          "Control sharing behavior:'manual' allows manual sharing via commands, 'auto' enables automatic sharing, 'disabled' disables all sharing",
        ),
      autoshare: z
        .boolean()
        .optional()
        .describe("@deprecated Use 'share' field instead. Share newly created sessions automatically"),
      autoupdate: z
        .union([z.boolean(), z.literal("notify")])
        .optional()
        .describe(
          "Automatically update to the latest version. Set to true to auto-update, false to disable, or 'notify' to show update notifications",
        ),
      disabled_providers: z.array(z.string()).optional().describe("Disable providers that are loaded automatically"),
      enabled_providers: z
        .array(z.string())
        .optional()
        .describe("When set, ONLY these providers will be enabled. All other providers will be ignored"),
      model: ModelId.describe("Model to use in the format of provider/model, eg anthropic/claude-2").optional(),
      model_auto: z
        .boolean()
        .optional()
        .describe(
          "When true (default), gizzi picks the brain from a paid Allternit Cloud subscription or an installed CLI. Set false to pin a /model choice.",
        ),
      auth: z
        .object({
          active_profile: z.string().optional(),
          credential_store: z.enum(["file", "keyring", "auto"]).optional(),
          profiles: z
            .record(
              z.string(),
              z.object({
                provider: z.string(),
                api_key: z.string().optional(),
                api_key_env: z.string().optional(),
                base_url: z.string().optional(),
              }),
            )
            .optional(),
        })
        .optional()
        .describe("Named authentication profiles for CLI and provider selection"),
      permission_profiles: z
        .object({
          active_profile: z.string().optional(),
          profiles: z
            .record(
              z.string(),
              z.object({
                rules: z.record(z.string(), PermissionAction),
              }),
            )
            .optional(),
        })
        .optional()
        .describe("Named permission profiles managed via `gizzi config profile`"),
      sandbox: z
        .object({
          enabled: z.boolean().optional(),
          allow_network: z.boolean().optional(),
          allowed_domains: z.array(z.string()).optional(),
          mode: z
            .enum(["read-only", "workspace-write", "danger-full-access"])
            .optional()
            .describe(
              "Sandbox mode preset. Fills in 'enabled'/'allow_network' defaults when not explicitly set.",
            ),
        })
        .optional()
        .describe("Default sandbox preferences for non-interactive execution"),
      approval_policy: ApprovalPolicy.optional(),
      small_model: ModelId.describe(
        "Small model to use for tasks like title generation in the format of provider/model",
      ).optional(),
      routing: z
        .object({
          tiers: z.object({
            simple: ModelId.describe("Model for simple requests (greetings, short questions)"),
            standard: ModelId.describe("Model for standard requests (general tasks)"),
            complex: ModelId.describe("Model for complex requests (multi-step, technical)"),
            reasoning: ModelId.describe("Model for reasoning requests (formal logic, planning)"),
          }),
          fallbacks: z
            .array(ModelId)
            .optional()
            .describe(
              "Ordered cross-provider failover chain (format: provider/model). When the active model fails, the next untried model in this list takes over.",
            ),
        })
        .optional()
        .describe(
          "Smart model routing config. Set model: auto to enable. Maps complexity tiers to models (format: provider/model).",
        ),
      default_agent: z
        .string()
        .optional()
        .describe(
          "Default agent to use when none is specified. Must be a primary agent. Falls back to 'build' if not set or if the specified agent is invalid.",
        ),
      username: z
        .string()
        .optional()
        .describe("Custom username to display in conversations instead of system username"),
      mode: z
        .object({
          build: Agent.optional(),
          plan: Agent.optional(),
        })
        .catchall(Agent)
        .optional()
        .describe("@deprecated Use `agent` field instead."),
      agent: z
        .object({
          // primary
          plan: Agent.optional(),
          build: Agent.optional(),
          // subagent
          general: Agent.optional(),
          explore: Agent.optional(),
          // specialized
          title: Agent.optional(),
          summary: Agent.optional(),
          compaction: Agent.optional(),
        })
        .catchall(Agent)
        .optional()
        .describe("Agent configuration, see https://gizzi.sh/docs/agents"),
      provider: z
        .record(z.string(), Provider)
        .optional()
        .describe("Custom provider configurations and model overrides"),
      mcp: z
        .record(
          z.string(),
          z.union([
            Mcp,
            z
              .object({
                enabled: z.boolean(),
              })
              .strict(),
          ]),
        )
        .optional()
        .describe("MCP (Model Context Protocol) server configurations"),
      formatter: z
        .union([
          z.literal(false),
          z.record(
            z.string(),
            z.object({
              disabled: z.boolean().optional(),
              command: z.array(z.string()).optional(),
              environment: z.record(z.string(), z.string()).optional(),
              extensions: z.array(z.string()).optional(),
            }),
          ),
        ])
        .optional(),
      lsp: z
        .union([
          z.literal(false),
          z.record(
            z.string(),
            z.union([
              z.object({
                disabled: z.literal(true),
              }),
              z.object({
                command: z.array(z.string()),
                extensions: z.array(z.string()).optional(),
                disabled: z.boolean().optional(),
                env: z.record(z.string(), z.string()).optional(),
                initialization: z.record(z.string(), z.any()).optional(),
              }),
            ]),
          ),
        ])
        .optional()
        .refine(
          (data) => {
            if (!data) return true
            if (typeof data === "boolean") return true
            const serverIds = new Set(Object.values(LSPServer).map((s) => s.id))

            return Object.entries(data).every(([id, config]) => {
              if (config.disabled) return true
              if (serverIds.has(id)) return true
              return Boolean((config as { extensions?: string[] }).extensions)
            })
          },
          {
            error: "For custom LSP servers, 'extensions' array is required.",
          },
        ),
      instructions: z.array(z.string()).optional().describe("Additional instruction files or patterns to include"),
      layout: Layout.optional().describe("@deprecated Always uses stretch layout."),
      permission: Permission.optional(),
      tools: z.record(z.string(), z.boolean()).optional(),
      enterprise: z
        .object({
          url: z.string().optional().describe("Enterprise URL"),
        })
        .optional(),
      compaction: z
        .object({
          auto: z.boolean().optional().describe("Enable automatic compaction when context is full (default: true)"),
          prune: z.boolean().optional().describe("Enable pruning of old tool outputs (default: true)"),
          reserved: z
            .number()
            .int()
            .min(0)
            .optional()
            .describe("Token buffer for compaction. Leaves enough window to avoid overflow during compaction."),
        })
        .optional(),
      limits: z
        .object({
          land_at: z
            .number()
            .min(0.5)
            .max(1)
            .optional()
            .describe("Share of a provider window (5-hour, weekly…) at which a session pauses before the limit (default: 0.95)"),
          auto_resume: z.boolean().optional().describe("Resume paused sessions on their own when the limit resets (default: true)"),
          fallback: z
            .enum(["off", "suggest", "auto"])
            .optional()
            .describe(
              "When a session pauses at a limit: suggest the model with the most limit left (default), switch to it automatically, or do neither",
            ),
          fallback_models: z
            .array(z.string())
            .optional()
            .describe("Preferred models to continue on, in order (provider/model). Default: any connected model with limit left"),
        })
        .optional(),
      handoff: z
        .object({
          auto: z
            .boolean()
            .optional()
            .describe("Hand off to a fresh context window between turns once the window is past the threshold (default: true)"),
          threshold: z
            .number()
            .min(0.3)
            .max(0.95)
            .optional()
            .describe("Share of the usable window that triggers a handoff (default: 0.7)"),
        })
        .optional(),
      experimental: z
        .object({
          disable_paste_summary: z.boolean().optional(),
          batch_tool: z.boolean().optional().describe("Enable the batch tool"),
          openTelemetry: z
            .boolean()
            .optional()
            .describe("Enable OpenTelemetry spans for AI SDK calls (using the 'experimental_telemetry' flag)"),
          primary_tools: z
            .array(z.string())
            .optional()
            .describe("Tools that should only be available to primary agents."),
          dynamic_tool_selection: z
            .boolean()
            .optional()
            .describe("Defer MCP tool schemas until the agent loads them with select_tools."),
          continue_loop_on_deny: z.boolean().optional().describe("Continue the agent loop when a tool call is denied"),
          retry_max_attempts: z
            .number()
            .int()
            .min(0)
            .optional()
            .describe(
              "Maximum session-level retries for retryable provider errors. Default is 2. Set to 0 to disable retries.",
            ),
          retry_max_delay_ms: z
            .number()
            .int()
            .positive()
            .optional()
            .describe("Maximum delay in milliseconds between session-level retries. Default is 10000 (10 seconds)."),
          mcp_timeout: z
            .number()
            .int()
            .positive()
            .optional()
            .describe("Timeout in milliseconds for model context protocol (MCP) requests"),
        })
        .optional(),
      hooks: z
        .object({
          http: z
            .array(
              z.object({
                url: z.string(),
                events: z.array(z.string()),
                matchers: z.array(z.string()).optional().describe("Tool name glob patterns to filter events"),
              }),
            )
            .optional()
            .describe("HTTP webhook hooks"),
          command: z
            .array(
              z.object({
                command: z.string().describe("Shell command to execute"),
                events: z.array(z.string()),
                matchers: z.array(z.string()).optional().describe("Tool name glob patterns to filter events"),
                timeout: z.number().int().positive().optional().describe("Timeout in ms (default: 10000)"),
              }),
            )
            .optional()
            .describe("Shell command hooks"),
        })
        .optional()
        .describe("Hook configurations for tool and session lifecycle events"),
      verification: z
        .object({
          enabled: z.boolean().optional().describe("Enable semi-formal verification (default: true)"),
          mode: z.enum(["adaptive", "semi-formal", "empirical", "both"]).optional().describe("Default verification mode"),
          minConfidence: z.enum(["high", "medium", "low"]).optional().describe("Minimum confidence required"),
          autoVerify: z.boolean().optional().describe("Automatically verify after each turn (default: false)"),
          storeResults: z.boolean().optional().describe("Store verification results (default: true)"),
          model: z.object({
            providerID: z.string(),
            modelID: z.string(),
          }).optional().describe("Model to use for semi-formal reasoning"),
          visualCapture: z.object({
            enabled: z.boolean().optional().describe("Enable visual evidence capture (default: true)"),
            outputDir: z.string().optional().describe("Output directory for captures (default: ./.verification/visual)"),
            enabledTypes: z.array(
              z.enum(["ui-state", "visual-diff", "coverage-map", "performance-chart", "error-state", "console-output"])
            ).optional().describe("Types of visual artifacts to capture"),
            viewport: z.object({
              width: z.number().optional(),
              height: z.number().optional(),
            }).optional().describe("Viewport size for screenshots"),
            includeBase64: z.boolean().optional().describe("Include base64 image data in artifacts"),
          }).optional().describe("Visual evidence capture configuration for screenshots, coverage maps, etc."),
        })
        .optional()
        .describe("Semi-formal verification configuration based on Meta's Agentic Code Reasoning"),
    })
    .strict()


  export type Info = z.output<typeof Info>

  export const global = lazy(async () => {
    const legacyConfigDir = path.join(xdgConfig ?? path.join(os.homedir(), ".config"), "gizzi")
    // Build config through sequential merges to avoid TypeScript type depth limit
    let result: Info = {} as Info
    // Legacy GIZZI config stays as fallback to preserve plugin/provider/MCP defaults.
    result = mergeDeep(result, await loadFile(path.join(legacyConfigDir, "config.json")))
    result = mergeDeep(result, await loadFile(path.join(legacyConfigDir, "gizzi.json")))
    result = mergeDeep(result, await loadFile(path.join(legacyConfigDir, "gizzi.jsonc")))
    result = mergeDeep(result, await loadFile(path.join(legacyConfigDir, "config.toml")))
    result = mergeDeep(result, await loadFile(path.join(Global.Path.config, "config.json")))
    result = mergeDeep(result, await loadFile(path.join(Global.Path.config, "gizzi.json")))
    result = mergeDeep(result, await loadFile(path.join(Global.Path.config, "gizzi.jsonc")))
    result = mergeDeep(result, await loadFile(path.join(Global.Path.config, "config.toml")))

    const legacy = path.join(Global.Path.config, "config")
    if (existsSync(legacy)) {
      await import(pathToFileURL(legacy).href, {
        with: {
          type: "toml",
        },
      })
        .then(async (mod) => {
          const { provider, model, ...rest } = mod.default
          if (provider && model) result.model = `${provider}/${model}`
          result["$schema"] = "https://gizzi.sh/config.json"
          result = mergeDeep(result, rest)
          await Filesystem.writeJson(path.join(Global.Path.config, "config.json"), result)
          await fs.unlink(legacy)
        })
        .catch((err) => log.warn("legacy config migration failed", { error: err }))
    }

    return result
  })

  async function loadFile(filepath: string): Promise<Info> {
    log.info("loading", { path: filepath })
    let text = await Filesystem.readText(filepath).catch((err: any) => {
      if (err.code === "ENOENT") return
      throw new JsonError({ path: filepath }, { cause: err })
    })
    if (!text) return {}
    if (filepath.endsWith(".toml")) return loadToml(text, filepath)
    return load(text, { path: filepath })
  }

  function loadToml(text: string, filepath: string): Info {
    try {
      const parsed = Bun.TOML.parse(text) as Record<string, unknown>
      const { default_model, ...config } = parsed
      if (default_model !== undefined && config.model === undefined) config.model = default_model
      return Info.parse(config)
    } catch (error) {
      throw new InvalidError(
        { path: filepath, message: error instanceof Error ? error.message : String(error) },
        { cause: error },
      )
    }
  }

  async function load(text: string, options: { path: string } | { dir: string; source: string }) {
    const original = text
    const configDir = "path" in options ? path.dirname(options.path) : options.dir
    const source = "path" in options ? options.path : options.source
    const isFile = "path" in options

    text = text.replace(/\{env:([^}]+)\}/g, (_, varName) => {
      return process.env[varName] || ""
    })

    const fileMatches = text.match(/\{file:[^}]+\}/g)
    if (fileMatches) {
      const lines = text.split("\n")

      for (const match of fileMatches) {
        const lineIndex = lines.findIndex((line) => line.includes(match))
        if (lineIndex !== -1 && lines[lineIndex].trim().startsWith("//")) {
          continue
        }
        let filePath = match.replace(/^\{file:/, "").replace(/\}$/, "")
        if (filePath.startsWith("~/")) {
          filePath = path.join(os.homedir(), filePath.slice(2))
        }
        const resolvedPath = path.isAbsolute(filePath) ? filePath : path.resolve(configDir, filePath)
        const fileContent = (
          await Bun.file(resolvedPath)
            .text()
            .catch((error) => {
              const errMsg = `bad file reference: "${match}"`
              if (error.code === "ENOENT") {
                throw new InvalidError(
                  {
                    path: source,
                    message: errMsg + ` ${resolvedPath} does not exist`,
                  },
                  { cause: error },
                )
              }
              throw new InvalidError({ path: source, message: errMsg }, { cause: error })
            })
        ).trim()
        text = text.replace(match, () => JSON.stringify(fileContent).slice(1, -1))
      }
    }

    const errors: JsoncParseError[] = []
    const data = parseJsonc(text, errors, { allowTrailingComma: true })
    if (errors.length) {
      const lines = text.split("\n")
      const errorDetails = errors
        .map((e) => {
          const beforeOffset = text.substring(0, e.offset).split("\n")
          const line = beforeOffset.length
          const column = beforeOffset[beforeOffset.length - 1].length + 1
          const problemLine = lines[line - 1]

          const error = `${printParseErrorCode(e.error)} at line ${line}, column ${column}`
          if (!problemLine) return error

          return `${error}\n   Line ${line}: ${problemLine}\n${"".padStart(column + 9)}^`
        })
        .join("\n")

      throw new JsonError({
        path: source,
        message: `\n--- JSONC Input ---\n${text}\n--- Errors ---\n${errorDetails}\n--- End ---`,
      })
    }

    const parsed = Info.safeParse(data)
    if (parsed.success) {
      if (!parsed.data.$schema && isFile) {
        parsed.data.$schema = "https://docs.gizziio.com/config.json"
        const updated = original.replace(/^\s*\{/, '{\n  "$schema": "https://docs.gizziio.com/config.json",')
        await Bun.write(options.path, updated).catch((err) =>
          log.warn("failed to persist $schema into config file", { path: options.path, error: err }),
        )
      }
      const data = parsed.data
      if (data.plugin && isFile) {
        for (let i = 0; i < data.plugin.length; i++) {
          const plugin = data.plugin[i]
          try {
            data.plugin[i] = import.meta.resolve!(plugin, options.path)
          } catch (err) {
            log.debug("Failed to resolve plugin path", { plugin, error: err })
          }
        }
      }
      return data
    }

    throw new InvalidError({
      path: source,
      issues: parsed.error.issues,
    })
  }
  export const JsonError = NamedError.create(
    "ConfigJsonError",
    z.object({
      path: z.string(),
      message: z.string().optional(),
    }),
  )

  export const ConfigDirectoryTypoError = NamedError.create(
    "ConfigDirectoryTypoError",
    z.object({
      path: z.string(),
      dir: z.string(),
      suggestion: z.string(),
    }),
  )

  export const InvalidError = NamedError.create(
    "ConfigInvalidError",
    z.object({
      path: z.string(),
      issues: z.custom<z.core.$ZodIssue[]>().optional(),
      message: z.string().optional(),
    }),
  )

  export async function get() {
    return state().then((x) => x.config)
  }

  export async function getGlobal() {
    return global()
  }

  export async function update(config: Info) {
    const filepath = path.join(Instance.directory, "config.json")
    const existing = await loadFile(filepath)
    await Filesystem.writeJson(filepath, mergeDeep(existing, config))
    await Instance.dispose()
  }

  function globalConfigFile() {
    const candidates = ["gizzi.jsonc", "gizzi.json", "config.json"].map((file) =>
      path.join(Global.Path.config, file),
    )
    for (const file of candidates) {
      if (existsSync(file)) return file
    }
    return candidates[0]
  }

  function isRecord(value: unknown): value is Record<string, unknown> {
    return !!value && typeof value === "object" && !Array.isArray(value)
  }

  function patchJsonc(input: string, patch: unknown, path: string[] = []): string {
    if (!isRecord(patch)) {
      const edits = modify(input, path, patch, {
        formattingOptions: {
          insertSpaces: true,
          tabSize: 2,
        },
      })
      return applyEdits(input, edits)
    }

    return Object.entries(patch).reduce((result, [key, value]) => {
      if (value === undefined) return result
      return patchJsonc(result, value, [...path, key])
    }, input)
  }

  function parseConfig(text: string, filepath: string): Info {
    const errors: JsoncParseError[] = []
    const data = parseJsonc(text, errors, { allowTrailingComma: true })
    if (errors.length) {
      const lines = text.split("\n")
      const errorDetails = errors
        .map((e) => {
          const beforeOffset = text.substring(0, e.offset).split("\n")
          const line = beforeOffset.length
          const column = beforeOffset[beforeOffset.length - 1].length + 1
          const problemLine = lines[line - 1]

          const error = `${printParseErrorCode(e.error)} at line ${line}, column ${column}`
          if (!problemLine) return error

          return `${error}\n   Line ${line}: ${problemLine}\n${"".padStart(column + 9)}^`
        })
        .join("\n")

      throw new JsonError({
        path: filepath,
        message: `\n--- JSONC Input ---\n${text}\n--- Errors ---\n${errorDetails}\n--- End ---`,
      })
    }

    const parsed = Info.safeParse(data)
    if (parsed.success) return parsed.data

    throw new InvalidError({
      path: filepath,
      issues: parsed.error.issues,
    })
  }

  export async function updateGlobal(config: Info, opts?: { reload?: boolean }) {
    const filepath = globalConfigFile()
    const before = await Filesystem.readText(filepath).catch((err: any) => {
      if (err.code === "ENOENT") return "{}"
      throw new JsonError({ path: filepath }, { cause: err })
    })

    const next = await (async () => {
      if (!filepath.endsWith(".jsonc")) {
        const existing = parseConfig(before, filepath)
        const merged = mergeDeep(existing, config)
        await Filesystem.writeJson(filepath, merged)
        return merged
      }

      const updated = patchJsonc(before, config)
      const merged = parseConfig(updated, filepath)
      await Filesystem.write(filepath, updated)
      return merged
    })()

    global.reset()

    if (opts?.reload === false) return next

    void Instance.disposeAll()
      .catch(() => undefined)
      .finally(() => {
        GlobalBus.emit("event", {
          directory: "global",
          payload: {
            type: Event.Disposed.type,
            properties: {},
          },
        })
      })

    return next
  }

  export async function directories() {
    return state().then((x) => x.directories)
  }
}
