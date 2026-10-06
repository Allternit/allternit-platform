import path from "path"
import os from "os"
import { Global } from "@/runtime/context/global"
import { Filesystem } from "@/shared/util/filesystem"
import { Config } from "@/runtime/context/config/config"
import { Instance } from "@/runtime/context/project/instance"
import { Flag } from "@/runtime/context/flag/flag"
import { Log } from "@/shared/util/log"
import { Glob } from "@/shared/util/glob"
import type { MessageV2 } from "@/runtime/session/message-v2"
import { parseFrontmatter } from "@/runtime/memory/memory-service"
import { ANTI_PATTERNS_FILENAME, LOCAL_INSTRUCTION_FILENAMES, pickWinner, ROOT_INSTRUCTION_FILENAMES } from "@/shared/utils/agentFileResolver"

const log = Log.create({ service: "instruction" })

// GIZZI.md is canonical; CLAUDE.md/AGENTS.md/CONTEXT.md are read for compat
// (crossing over from gizzi-code, or the cross-tool AGENTS.md convention).
// Order matters: this is precedence, not a set of independently-loaded files —
// see agentFileResolver.ts, the shared source of truth for this order across
// both this (headless/server) pipeline and the interactive TUI pipeline.
const FILES = ROOT_INSTRUCTION_FILENAMES

const MAX_MEMORY_LINES = 200

/**
 * Workspace memory directories (project-local `.gizzi`/`.openclaw`
 * L1-COGNITIVE notes). The user's long-term memory is the Memory Drive,
 * injected separately by SystemPrompt (src/runtime/session/system.ts) with
 * its checkout path; drive topic files are read on demand with normal tools,
 * so they are deliberately NOT auto-loaded here. The old per-project memdir
 * and pre-convergence global store are import sources only
 * (`gizzi memory import`).
 */
function workspaceMemoryDirs(): string[] {
  return [
    path.join(Instance.directory, ".gizzi", "L1-COGNITIVE", "memory"),
    path.join(Instance.directory, ".openclaw", "L1-COGNITIVE", "memory"),
  ]
}

/** Workspace memory files to auto-load into context */
function workspaceMemoryFiles(): string[] {
  return workspaceMemoryDirs().map((dir) => path.join(dir, "MEMORY.md"))
}

/** Discover all topic memory files (*.md) in all workspace memory dirs */
async function discoverTopicMemoryFiles(): Promise<string[]> {
  const results: string[] = []
  const seen = new Set<string>()
  for (const dir of workspaceMemoryDirs()) {
    if (!(await Filesystem.exists(dir))) continue
    const files = await Glob.scan("*.md", { cwd: dir, absolute: true, include: "file" }).catch(() => [])
    for (const f of files) {
      const resolved = path.resolve(f)
      // Skip MEMORY.md itself (already loaded separately)
      if (path.basename(resolved) === "MEMORY.md") continue
      // Deduplicate by basename across directories
      if (seen.has(path.basename(resolved))) continue
      seen.add(path.basename(resolved))
      results.push(resolved)
    }
  }
  return results
}

/** Score a topic file's relevance to a session title using frontmatter description */
async function scoreTopicRelevance(filepath: string, queryTerms: string[]): Promise<number> {
  if (queryTerms.length === 0) return 1 // no filter — include all
  const content = await Filesystem.readText(filepath).catch(() => "")
  if (!content) return 0
  const { fm } = parseFrontmatter(content)
  // If no frontmatter, include by default (legacy files)
  if (!fm.description) return 1
  const haystack = [fm.name ?? "", fm.description, fm.type ?? ""].join(" ").toLowerCase()
  return queryTerms.filter((t) => haystack.includes(t)).length
}

/**
 * Filter topic files by relevance to the current session title.
 * Files with frontmatter are scored; files without frontmatter pass through.
 */
async function relevantTopicFiles(files: string[], sessionTitle?: string): Promise<string[]> {
  if (!sessionTitle) return files
  const terms = sessionTitle
    .toLowerCase()
    .split(/[\s\-_/\\.,;:!?()[\]{}'"]+/)
    .filter((t) => t.length > 2)
  if (terms.length === 0) return files

  const scored = await Promise.all(
    files.map(async (f) => ({ f, score: await scoreTopicRelevance(f, terms) })),
  )
  return scored.filter((s) => s.score > 0).map((s) => s.f)
}

async function loadMemoryFile(filepath: string): Promise<string> {
  const content = await Filesystem.readText(filepath).catch(() => "")
  if (!content) return ""
  const lines = content.split("\n")
  const truncated = lines.slice(0, MAX_MEMORY_LINES).join("\n")
  const suffix = lines.length > MAX_MEMORY_LINES
    ? `\n\n<!-- Truncated at ${MAX_MEMORY_LINES} lines. ${lines.length - MAX_MEMORY_LINES} lines omitted. -->`
    : ""
  return "Auto-memory from: " + filepath + "\n" + truncated + suffix
}

function globalFiles() {
  const files = []
  // User-global memory, matching the TUI's getMemoryPath('User') precedence
  // (GIZZI.md > CLAUDE.md within the same config home; GIZZI_CONFIG_DIR ??
  // ~/.gizzi). The headless pipeline's AGENTS.md convention is preserved as a
  // fallback after the GIZZI/CLAUDE names in each directory.
  const userConfigDir = Flag.GIZZI_CONFIG_DIR ?? path.join(Global.Path.home, ".gizzi")
  files.push(path.join(userConfigDir, "GIZZI.md"))
  files.push(path.join(userConfigDir, "CLAUDE.md"))
  files.push(path.join(userConfigDir, "AGENTS.md"))
  files.push(path.join(Global.Path.config, "GIZZI.md"))
  files.push(path.join(Global.Path.config, "AGENTS.md"))
  if (!Flag.GIZZI_DISABLE_LEGACY_PROMPT) {
    files.push(path.join(Global.Path.home, ".claude", "CLAUDE.md"))
  }
  return files
}

async function resolveRelative(instruction: string): Promise<string[]> {
  if (!Flag.GIZZI_DISABLE_PROJECT_CONFIG) {
    return Filesystem.globUp(instruction, Instance.directory, Instance.worktree).catch(() => [])
  }
  if (!Flag.GIZZI_CONFIG_DIR) {
    log.warn(
      `Skipping relative instruction "${instruction}" - no GIZZI_CONFIG_DIR set while project config is disabled`,
    )
    return []
  }
  return Filesystem.globUp(instruction, Flag.GIZZI_CONFIG_DIR, Flag.GIZZI_CONFIG_DIR).catch(() => [])
}

export namespace InstructionPrompt {
  const state = Instance.state(() => {
    return {
      claims: new Map<string, Set<string>>(),
    }
  })

  function isClaimed(messageID: string, filepath: string) {
    const claimed = state().claims.get(messageID)
    if (!claimed) return false
    return claimed.has(filepath)
  }

  function claim(messageID: string, filepath: string) {
    const current = state()
    let claimed = current.claims.get(messageID)
    if (!claimed) {
      claimed = new Set()
      current.claims.set(messageID, claimed)
    }
    claimed.add(filepath)
  }

  export function clear(messageID: string) {
    state().claims.delete(messageID)
  }

  export async function systemPaths(sessionTitle?: string) {
    const config = await Config.get()
    const paths = new Set<string>()

    if (!Flag.GIZZI_DISABLE_PROJECT_CONFIG) {
      // Per-directory-level precedence (not a single global winner for the
      // whole walk): each directory from cwd up to the worktree root picks
      // its own highest-precedence file independently, so e.g. a subdirectory
      // with only AGENTS.md and a parent directory with only CLAUDE.md are
      // BOTH honored, instead of one filename choice suppressing the other
      // directory's file entirely.
      let current = Instance.directory
      while (true) {
        const existing = new Set<string>()
        for (const file of FILES) {
          if (await Filesystem.exists(path.join(current, file))) existing.add(file)
        }
        const winner = pickWinner(existing, FILES)
        if (winner) paths.add(path.resolve(path.join(current, winner)))

        // Local (gitignored, personal) instructions: GIZZI.local.md >
        // CLAUDE.local.md at the same directory level, loaded alongside the
        // root marker like the TUI does.
        const existingLocal = new Set<string>()
        for (const file of LOCAL_INSTRUCTION_FILENAMES) {
          if (await Filesystem.exists(path.join(current, file))) existingLocal.add(file)
        }
        const localWinner = pickWinner(existingLocal, LOCAL_INSTRUCTION_FILENAMES)
        if (localWinner) paths.add(path.resolve(path.join(current, localWinner)))

        // ANTI_PATTERNS.md is a companion to the root marker, not a
        // precedence alternative — load it alongside, same directory level.
        const antiPatternsPath = path.join(current, ANTI_PATTERNS_FILENAME)
        if (await Filesystem.exists(antiPatternsPath)) paths.add(path.resolve(antiPatternsPath))

        if (Instance.worktree === current) break
        const parent = path.dirname(current)
        if (parent === current) break
        current = parent
      }
    }

    for (const file of globalFiles()) {
      if (await Filesystem.exists(file)) {
        paths.add(path.resolve(file))
        break
      }
    }

    // Auto-load workspace memory files (MEMORY.md from all layers)
    for (const file of workspaceMemoryFiles()) {
      if (await Filesystem.exists(file)) {
        paths.add(path.resolve(file))
      }
    }

    // Auto-load topic-specific memory files (*.md in memory dir, excluding MEMORY.md)
    // Filter by session title relevance when frontmatter is present
    const allTopicFiles = await discoverTopicMemoryFiles()
    const topicFiles = await relevantTopicFiles(allTopicFiles, sessionTitle)
    for (const file of topicFiles) {
      paths.add(file)
    }

    if (config.instructions) {
      for (let instruction of config.instructions) {
        if (instruction.startsWith("https://") || instruction.startsWith("http://")) continue
        if (instruction.startsWith("~/")) {
          instruction = path.join(os.homedir(), instruction.slice(2))
        }
        const matches = path.isAbsolute(instruction)
          ? await Glob.scan(path.basename(instruction), {
              cwd: path.dirname(instruction),
              absolute: true,
              include: "file",
            }).catch(() => [])
          : await resolveRelative(instruction)
        matches.forEach((p) => {
          paths.add(path.resolve(p))
        })
      }
    }

    return paths
  }

  export async function system(sessionTitle?: string) {
    const config = await Config.get()
    const paths = await systemPaths(sessionTitle)

    const allTopicFiles = await discoverTopicMemoryFiles()
    const memoryPaths = new Set([
      ...workspaceMemoryFiles().map((f) => path.resolve(f)),
      ...allTopicFiles,
    ])
    const files = Array.from(paths).map(async (p) => {
      if (memoryPaths.has(p)) return loadMemoryFile(p)
      const content = await Filesystem.readText(p).catch(() => "")
      return content ? "Instructions from: " + p + "\n" + content : ""
    })

    const urls: string[] = []
    if (config.instructions) {
      for (const instruction of config.instructions) {
        if (instruction.startsWith("https://") || instruction.startsWith("http://")) {
          urls.push(instruction)
        }
      }
    }
    const fetches = urls.map((url) =>
      fetch(url, { signal: AbortSignal.timeout(5000) })
        .then((res) => (res.ok ? res.text() : ""))
        .catch(() => "")
        .then((x) => (x ? "Instructions from: " + url + "\n" + x : "")),
    )

    return Promise.all([...files, ...fetches]).then((result) => result.filter(Boolean))
  }

  export function loaded(messages: MessageV2.WithParts[]) {
    const paths = new Set<string>()
    for (const msg of messages) {
      for (const part of msg.parts) {
        if (part.type === "tool" && part.tool === "read" && part.state.status === "completed") {
          if (part.state.time.compacted) continue
          const loaded = part.state.metadata?.loaded
          if (!loaded || !Array.isArray(loaded)) continue
          for (const p of loaded) {
            if (typeof p === "string") paths.add(p)
          }
        }
      }
    }
    return paths
  }

  export async function find(dir: string) {
    for (const file of FILES) {
      const filepath = path.resolve(path.join(dir, file))
      if (await Filesystem.exists(filepath)) return filepath
    }
  }

  export async function resolve(messages: MessageV2.WithParts[], filepath: string, messageID: string) {
    const system = await systemPaths()
    const already = loaded(messages)
    const results: { filepath: string; content: string }[] = []

    const target = path.resolve(filepath)
    let current = path.dirname(target)
    const root = path.resolve(Instance.directory)

    while (current.startsWith(root) && current !== root) {
      const found = await find(current)

      if (found && found !== target && !system.has(found) && !already.has(found) && !isClaimed(messageID, found)) {
        claim(messageID, found)
        const content = await Filesystem.readText(found).catch(() => undefined)
        if (content) {
          results.push({ filepath: found, content: "Instructions from: " + found + "\n" + content })
        }
      }
      current = path.dirname(current)
    }

    return results
  }
}
