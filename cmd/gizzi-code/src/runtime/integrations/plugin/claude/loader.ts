/**
 * Loader for the native .claude-plugin/ format.
 *
 * Discovers and loads plugins from:
 *   - ~/.gizzi/plugins/                              (user-installed plugins, canonical)
 *   - ~/.gizzi/plugins/marketplaces/* / *            (marketplace clones, canonical)
 *   - ~/.gizzi/plugins/cache/* / * / *               (versioned cache, canonical)
 *   - ~/.claude/plugins/marketplaces/* / *           (legacy read-only fallback)
 *   - ~/.claude/plugins/cache/* / * / *              (legacy read-only fallback)
 *   - src/runtime/plugins/builtin/                   (built-in plugins shipped with gizzi)
 *
 * The ~/.claude locations are upstream-inherited and read-only: they keep
 * existing installs discoverable but nothing writes there. Run
 * `gizzi plugin migrate` to copy legacy content into ~/.gizzi/plugins.
 */
import fs from "fs/promises"
import path from "path"
import os from "os"
import { ConfigMarkdown } from "@/runtime/context/config/markdown"
import { Log } from "@/shared/util/log"
import type {
  ClaudePlugin,
  ClaudePluginCommand,
  ClaudePluginManifest,
  ClaudePluginSkill,
  ClaudeHooksConfig,
  ClaudeMcpServer,
} from "./types"

const PLUGIN_META_DIR = ".claude-plugin"
const MANIFEST_FILE = "plugin.json"

let pluginCache: ClaudePlugin[] | null = null
let builtinCache: ClaudePlugin[] | null = null

export function invalidate(): void {
  pluginCache = null
  builtinCache = null
}

function homeDir(): string {
  return process.env.GIZZI_TEST_HOME || os.homedir()
}

async function exists(p: string): Promise<boolean> {
  return fs.access(p).then(() => true).catch(() => false)
}

async function listDir(p: string): Promise<string[]> {
  if (!(await exists(p))) return []
  return fs.readdir(p)
}

async function safeReadJson<T>(p: string): Promise<T | null> {
  if (!(await exists(p))) return null
  try {
    const text = await fs.readFile(p, "utf8")
    return JSON.parse(text) as T
  } catch {
    return null
  }
}

async function safeReadText(p: string): Promise<string | null> {
  if (!(await exists(p))) return null
  try {
    return await fs.readFile(p, "utf8")
  } catch {
    return null
  }
}

function expandClaudePluginRoot(value: unknown, root: string): unknown {
  if (typeof value === "string") {
    return value.replace(/\$\{GIZZI_PLUGIN_ROOT\}/g, root)
  }
  if (Array.isArray(value)) {
    return value.map((v) => expandClaudePluginRoot(v, root))
  }
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {}
    for (const [k, v] of Object.entries(value)) {
      out[k] = expandClaudePluginRoot(v, root)
    }
    return out
  }
  return value
}

async function discoverGizziPluginRoots(): Promise<string[]> {
  const base = path.join(homeDir(), ".gizzi", "plugins")
  const roots: string[] = []
  for (const name of await listDir(base)) {
    const root = path.join(base, name)
    const meta = path.join(root, PLUGIN_META_DIR, MANIFEST_FILE)
    if (await exists(meta)) roots.push(root)
  }
  return roots
}

async function discoverMarketplacePluginRoots(): Promise<string[]> {
  // Canonical gizzi location first, upstream-inherited ~/.claude location as
  // read-only legacy fallback so pre-migration installs stay discoverable.
  const bases = [
    path.join(homeDir(), ".gizzi", "plugins", "marketplaces"),
    path.join(homeDir(), ".claude", "plugins", "marketplaces"),
  ]
  const roots: string[] = []
  for (const base of bases) {
    for (const marketplace of await listDir(base)) {
      const pluginsDir = path.join(base, marketplace, "plugins")
      for (const name of await listDir(pluginsDir)) {
        const root = path.join(pluginsDir, name)
        const meta = path.join(root, PLUGIN_META_DIR, MANIFEST_FILE)
        if (await exists(meta)) roots.push(root)
      }
    }
  }
  return roots
}

async function discoverCachePluginRoots(): Promise<string[]> {
  // Canonical gizzi location first, ~/.claude as read-only legacy fallback.
  const bases = [
    path.join(homeDir(), ".gizzi", "plugins", "cache"),
    path.join(homeDir(), ".claude", "plugins", "cache"),
  ]
  const roots: string[] = []
  for (const base of bases) {
    for (const marketplace of await listDir(base)) {
      const marketplaceDir = path.join(base, marketplace)
      for (const name of await listDir(marketplaceDir)) {
        const pluginDir = path.join(marketplaceDir, name)
        for (const version of await listDir(pluginDir)) {
          const root = path.join(pluginDir, version)
          const meta = path.join(root, PLUGIN_META_DIR, MANIFEST_FILE)
          if (await exists(meta)) roots.push(root)
        }
      }
    }
  }
  return roots
}

export async function discoverPluginRoots(): Promise<string[]> {
  const [gizzi, marketplaces, cache] = await Promise.all([
    discoverGizziPluginRoots(),
    discoverMarketplacePluginRoots(),
    discoverCachePluginRoots(),
  ])
  return [...gizzi, ...marketplaces, ...cache]
}

async function loadManifest(root: string): Promise<ClaudePluginManifest | null> {
  const raw = await safeReadJson<Record<string, unknown>>(path.join(root, PLUGIN_META_DIR, MANIFEST_FILE))
  if (!raw) return null
  return raw as ClaudePluginManifest
}

const log = Log.create({ service: "claude-plugin" })

// One malformed file must not take down every plugin: skip it and warn.
function parseFrontmatter(text: string, source: string) {
  try {
    return ConfigMarkdown.parseText(text)
  } catch (err) {
    log.warn("skipping file with invalid frontmatter", { source, error: String(err) })
    return null
  }
}

async function loadCommands(root: string, pluginName: string): Promise<ClaudePluginCommand[]> {
  const commandsDir = path.join(root, "commands")
  if (!(await exists(commandsDir))) return []
  const files = (await fs.readdir(commandsDir)).filter((f) => f.endsWith(".md"))
  const commands: ClaudePluginCommand[] = []
  for (const file of files) {
    const source = path.join(commandsDir, file)
    const text = await fs.readFile(source, "utf8")
    const parsed = parseFrontmatter(text, source)
    if (!parsed) continue
    const data = parsed.data as Record<string, unknown>
    let allowedTools: string[] | undefined
    if (data["allowed-tools"] !== undefined) {
      if (typeof data["allowed-tools"] === "string") {
        allowedTools = data["allowed-tools"].split(/[,\s]+/).filter(Boolean)
      } else if (Array.isArray(data["allowed-tools"])) {
        allowedTools = data["allowed-tools"].map(String)
      }
    }
    commands.push({
      name: file.replace(/\.md$/, ""),
      description: data.description as string,
      allowedTools,
      argumentHint: data["argument-hint"] as string,
      model: data.model as string,
      template: parsed.content,
      pluginName,
      source,
    })
  }
  return commands
}

async function loadSkills(root: string, pluginName: string): Promise<ClaudePluginSkill[]> {
  const skillsDir = path.join(root, "skills")
  if (!(await exists(skillsDir))) return []
  const skills: ClaudePluginSkill[] = []
  for (const skillName of await listDir(skillsDir)) {
    const skillDir = path.join(skillsDir, skillName)
    const skillFile = path.join(skillDir, "SKILL.md")
    if (!(await exists(skillFile))) continue
    const text = await fs.readFile(skillFile, "utf8")
    const parsed = parseFrontmatter(text, skillFile)
    if (!parsed) continue
    const data = parsed.data as Record<string, unknown>
    if (!data.name || !data.description) continue
    skills.push({
      name: String(data.name),
      description: String(data.description),
      version: data.version ? String(data.version) : undefined,
      content: parsed.content,
      pluginName,
      location: skillFile,
    })
  }
  return skills
}

async function loadHooksConfig(root: string): Promise<ClaudeHooksConfig | null> {
  const hooksFile = path.join(root, "hooks", "hooks.json")
  const raw = await safeReadJson<ClaudeHooksConfig>(hooksFile)
  if (!raw) return null
  return raw
}

async function loadMcpServers(root: string): Promise<Record<string, ClaudeMcpServer>> {
  const mcpFile = path.join(root, ".mcp.json")
  const raw = await safeReadJson<Record<string, ClaudeMcpServer>>(mcpFile)
  if (!raw) return {}
  return expandClaudePluginRoot(raw, root) as Record<string, ClaudeMcpServer>
}

export async function loadPlugin(root: string): Promise<ClaudePlugin | null> {
  const manifest = await loadManifest(root)
  if (!manifest) return null
  const [commands, skills, hooksConfig, mcpServers] = await Promise.all([
    loadCommands(root, manifest.name),
    loadSkills(root, manifest.name),
    loadHooksConfig(root),
    loadMcpServers(root),
  ])
  return {
    root,
    manifest,
    commands,
    skills,
    hooksConfig,
    mcpServers,
  }
}

export async function all(includeCache = false): Promise<ClaudePlugin[]> {
  if (pluginCache) return pluginCache
  const roots = includeCache
    ? await discoverPluginRoots()
    : await discoverGizziPluginRoots()
  const plugins = (await Promise.all(roots.map(loadPlugin))).filter(Boolean) as ClaudePlugin[]
  pluginCache = plugins
  return plugins
}

function builtinDir(): string {
  return path.resolve(import.meta.dir, "..", "..", "..", "plugins", "builtin")
}

async function discoverBuiltinPluginRoots(): Promise<string[]> {
  const base = builtinDir()
  const roots: string[] = []
  for (const name of await listDir(base)) {
    const root = path.join(base, name)
    const meta = path.join(root, PLUGIN_META_DIR, MANIFEST_FILE)
    if (await exists(meta)) roots.push(root)
  }
  return roots
}

export async function allBuiltin(includeCache = false): Promise<ClaudePlugin[]> {
  if (builtinCache) return builtinCache
  const roots = await discoverBuiltinPluginRoots()
  const plugins = (await Promise.all(roots.map(loadPlugin))).filter(Boolean) as ClaudePlugin[]
  builtinCache = plugins
  return plugins
}
