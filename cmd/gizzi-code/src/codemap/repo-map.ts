/**
 * Live per-file repo map, put into every Code turn's system prompt.
 *
 * Built on the codemap's own scanners (scan-ts.ts with the TypeScript
 * compiler API, scan-rust.ts) but kept per FILE instead of rolled up to
 * <=20 modules, and kept current while a session edits. Same rules as the
 * rest of codemap/: no LLM, deterministic, never fabricate — every symbol
 * comes from a parse (TS/JS, `textual: false`) or a top-level regex
 * (Rust and other languages, `textual: true`), every import edge resolves
 * to a real file in the repo or is dropped.
 *
 * Ranking (`score`):
 *   - in-degree: how many indexed files import this one;
 *   - co-change: how often the file changed in the same commit as a file
 *     this session touched (`git log --name-only -n 300`);
 *   - ×3 (plus a floor) for files this session edited, read or mentioned.
 * Ties go to the more recently changed file, then the path.
 *
 * Cached at `<root>/.gizzi/repo-map.json` with each file's mtime + size, so
 * reopening a project only rescans files that changed. A write that would
 * drop more than 80% of the cached entries is refused (a broken listing
 * should not wipe a good map). Nothing in here throws into a session: every
 * public entry point logs and returns an empty result on failure.
 */
import fs from "fs/promises"
import os from "os"
import path from "path"
import ignore, { type Ignore } from "ignore"
import { Filesystem } from "@/shared/util/filesystem"
import { Git } from "@/shared/util/git"
import { Log } from "@/shared/util/log"
import { Token } from "@/shared/util/token"
import { Ripgrep } from "@/shared/file/ripgrep"
import { discoverModules, buildPackageNameMap, buildCrateNameMap } from "./discover"
import { mapWithConcurrency } from "./index"
import { scanRustFile } from "./scan-rust"
import { scanTypeScriptFileDetailed } from "./scan-ts"

export namespace RepoMap {
  const log = Log.create({ service: "repo-map" })

  export const MAX_FILES = 10_000
  export const DEFAULT_BUDGET_TOKENS = 1_500
  const MAX_SYMBOLS_SHOWN = 6
  const MAX_SYMBOLS_STORED = 24
  const HISTORY_COMMITS = 300
  /** Commits touching more files than this (mass renames, formatting runs) say nothing about coupling. */
  const MAX_COCHANGE_COMMIT_FILES = 50
  const SESSION_BOOST = 3
  /** Below this many cached entries the drop guard stays out of the way (tiny repos legitimately churn). */
  const DROP_GUARD_MIN_ENTRIES = 10
  const DROP_GUARD_RATIO = 0.8
  const DEBOUNCE_MS = 500
  const SCAN_CONCURRENCY = 16
  const MAX_SCAN_BYTES = 500_000
  const CACHE_VERSION = 1

  const TS_EXTENSIONS = new Set([".ts", ".tsx", ".js", ".jsx", ".mts", ".cts", ".mjs", ".cjs"])
  const EXCLUDED_DIRS = new Set(["node_modules", "dist", "build", "out", "target", ".next", "vendor", "coverage"])
  const LOCKFILES = new Set([
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    "Cargo.lock",
    "poetry.lock",
    "Pipfile.lock",
    "Gemfile.lock",
    "composer.lock",
    "go.sum",
    "flake.lock",
    "uv.lock",
  ])
  /** Assets and binaries: no symbols, no imports, only noise in a code map. */
  const SKIPPED_EXTENSIONS = new Set([
    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".ico", ".icns", ".bmp", ".avif", ".heic",
    ".mp3", ".mp4", ".mov", ".wav", ".webm", ".ogg", ".m4a",
    ".woff", ".woff2", ".ttf", ".otf", ".eot",
    ".pdf", ".zip", ".gz", ".tgz", ".tar", ".7z", ".dmg", ".exe", ".dll", ".so", ".dylib", ".a", ".o", ".wasm", ".bin",
    ".map", ".snap", ".db", ".sqlite", ".lock",
  ])

  /** Top-level declarations per language, line-anchored. Textual, not parsed. */
  const TEXTUAL_PATTERNS: Record<string, RegExp> = {
    ".rs": /^pub(?:\([^)]*\))?\s+(?:async\s+|unsafe\s+|const\s+)*(?:fn|struct|enum|trait|type|const|static|mod|union)\s+([A-Za-z_]\w*)/gm,
    ".py": /^(?:async\s+)?(?:def|class)\s+([A-Za-z_]\w*)/gm,
    ".go": /^(?:func(?:\s+\([^)]*\))?|type)\s+([A-Z]\w*)/gm,
    ".rb": /^\s{0,2}(?:def|class|module)\s+(?:self\.)?([A-Za-z_]\w*[?!]?)/gm,
    ".java": /^public\s+(?:(?:abstract|final|static|sealed)\s+)*(?:class|interface|enum|record)\s+([A-Za-z_]\w*)/gm,
    ".kt": /^(?:(?:public|internal|data|sealed|abstract|open|enum)\s+)*(?:class|interface|object|fun)\s+([A-Za-z_]\w*)/gm,
    ".swift": /^(?:public\s+|open\s+)?(?:final\s+)?(?:class|struct|enum|protocol|func|actor)\s+([A-Za-z_]\w*)/gm,
    ".php": /^(?:abstract\s+|final\s+)?(?:class|interface|trait|function)\s+([A-Za-z_]\w*)/gm,
    ".ex": /^\s*(?:defmodule|def)\s+([A-Za-z_][\w.]*)/gm,
    ".sh": /^(?:function\s+)?([A-Za-z_][\w-]*)\s*\(\)\s*\{/gm,
  }

  export interface FileEntry {
    mtime: number
    size: number
    /** Exported / top-level names, source order. */
    symbols: string[]
    /** True when `symbols` came from a regex, not a parser. */
    textual: boolean
    /** Repo-relative files this one imports (resolved, intra-repo only). */
    imports: string[]
  }

  interface CacheFile {
    version: number
    generatedAt: string
    files: Record<string, FileEntry>
    /** `git log --name-only` commits, newest first, repo-relative paths. */
    history: string[][]
  }

  interface RootState {
    root: string
    files: Map<string, FileEntry>
    history: string[][]
    /** Path → index of the newest commit that touched it (smaller = more recent). */
    recency: Map<string, number>
    internalPackages?: Map<string, string>
    internalCrates?: Map<string, string>
    /** The one scan or update running for this root; the next one chains after it. */
    queue: Promise<unknown>
    ready?: Promise<boolean>
    pending: Set<string>
    timer?: ReturnType<typeof setTimeout>
  }

  const roots = new Map<string, RootState>()
  /** Files each session wrote (absolute paths) — the `[NEW]` marks. */
  const editedBySession = new Map<string, Set<string>>()
  /** Per-session prompt text, rebuilt once per user turn so the cached system prefix stays put across steps. */
  const promptCache = new Map<string, { turn: string; text: string }>()
  const MAX_SESSIONS = 256

  // ---------------------------------------------------------------- listing

  function isExcluded(rel: string): boolean {
    const segments = rel.split("/")
    const base = segments[segments.length - 1]!
    for (let i = 0; i < segments.length - 1; i++) {
      const segment = segments[i]!
      if (segment.startsWith(".") || EXCLUDED_DIRS.has(segment)) return true
    }
    if (base.startsWith(".")) return true
    if (LOCKFILES.has(base)) return true
    if (/\.min\.[a-z0-9]+$/i.test(base)) return true
    if (SKIPPED_EXTENSIONS.has(path.extname(base).toLowerCase())) return true
    return false
  }

  async function loadIgnore(root: string): Promise<Ignore | undefined> {
    const file = path.join(root, ".gizziignore")
    if (!(await Filesystem.exists(file))) return undefined
    try {
      return ignore().add(await Filesystem.readText(file))
    } catch (error) {
      log.warn("unreadable .gizziignore", { root, error: String(error) })
      return undefined
    }
  }

  async function isGitRepo(root: string): Promise<boolean> {
    try {
      const result = await Git.exec(["rev-parse", "--is-inside-work-tree"], { cwd: root })
      return result.exitCode === 0 && (await result.text()).trim() === "true"
    } catch {
      return false
    }
  }

  /** Every indexable file, repo-relative with forward slashes, filters applied. */
  export async function listFiles(root: string): Promise<string[]> {
    let raw: string[] = []
    if (await isGitRepo(root)) {
      const result = await Git.exec(["ls-files", "-z", "--cached", "--others", "--exclude-standard"], { cwd: root })
      if (result.exitCode === 0) raw = (await result.text()).split("\0").filter(Boolean)
    }
    if (raw.length === 0) {
      for await (const file of Ripgrep.files({ cwd: root })) raw.push(file)
    }
    const ig = await loadIgnore(root)
    const seen = new Set<string>()
    const out: string[] = []
    for (const file of raw) {
      const rel = file.split(path.sep).join("/")
      if (seen.has(rel) || isExcluded(rel)) continue
      if (ig?.ignores(rel)) continue
      seen.add(rel)
      out.push(rel)
    }
    return out
  }

  async function readHistory(root: string): Promise<string[][]> {
    if (!(await isGitRepo(root))) return []
    try {
      // %x1e marks each commit; the names that follow are that commit's files.
      const result = await Git.exec(
        ["log", "--name-only", `--format=%x1e`, "-n", String(HISTORY_COMMITS), "--no-renames"],
        { cwd: root },
      )
      if (result.exitCode !== 0) return []
      return (await result.text())
        .split("\x1e")
        .map((chunk) =>
          chunk
            .split("\n")
            .map((line) => line.trim())
            .filter(Boolean),
        )
        .filter((files) => files.length > 0)
    } catch {
      return []
    }
  }

  function recencyOf(history: string[][]): Map<string, number> {
    const recency = new Map<string, number>()
    history.forEach((files, index) => {
      for (const file of files) if (!recency.has(file)) recency.set(file, index)
    })
    return recency
  }

  /** Over the cap: keep the most recently changed files (git log order), then by path. */
  function capFiles(files: string[], recency: Map<string, number>): string[] {
    if (files.length <= MAX_FILES) return files
    const rank = (file: string) => recency.get(file) ?? Number.MAX_SAFE_INTEGER
    return [...files].sort((a, b) => rank(a) - rank(b) || a.localeCompare(b)).slice(0, MAX_FILES)
  }

  // ---------------------------------------------------------------- scanning

  async function internalMaps(state: RootState) {
    if (state.internalPackages && state.internalCrates) return
    try {
      const modules = await discoverModules(state.root)
      const [packages, crates] = await Promise.all([
        buildPackageNameMap(state.root, modules),
        buildCrateNameMap(state.root, modules),
      ])
      state.internalPackages = packages
      state.internalCrates = crates
    } catch (error) {
      // Relative imports still resolve without these; only bare internal
      // package specifiers go unresolved.
      log.warn("module discovery failed; bare internal imports stay unresolved", { root: state.root, error: String(error) })
      state.internalPackages = new Map()
      state.internalCrates = new Map()
    }
  }

  function textualSymbols(content: string, ext: string): string[] {
    const pattern = TEXTUAL_PATTERNS[ext]
    if (!pattern) return []
    const regex = new RegExp(pattern.source, pattern.flags)
    const names: string[] = []
    let match: RegExpExecArray | null
    while ((match = regex.exec(content)) !== null) {
      const name = match[1]
      if (name && !names.includes(name)) names.push(name)
      if (names.length >= MAX_SYMBOLS_STORED) break
    }
    return names
  }

  /** A crate edge points at the crate directory; count it against the crate's root file when it is indexed. */
  function crateRootFile(crateDir: string, known: (rel: string) => boolean): string | undefined {
    for (const candidate of ["src/lib.rs", "src/main.rs", "lib.rs"]) {
      const rel = path.posix.join(crateDir, candidate)
      if (known(rel)) return rel
    }
    return undefined
  }

  async function scanFile(state: RootState, rel: string, stat: { mtime: number; size: number }, known: (rel: string) => boolean): Promise<FileEntry> {
    const ext = path.extname(rel).toLowerCase()
    const entry: FileEntry = { mtime: stat.mtime, size: stat.size, symbols: [], textual: false, imports: [] }
    if (stat.size > MAX_SCAN_BYTES) return entry
    try {
      if (TS_EXTENSIONS.has(ext)) {
        const scanned = await scanTypeScriptFileDetailed(state.root, rel, state.internalPackages ?? new Map())
        entry.symbols = scanned.exports.slice(0, MAX_SYMBOLS_STORED)
        entry.imports = [...new Set(scanned.edges.map((edge) => edge.toFile.split(path.sep).join("/")))]
        return entry
      }
      if (ext === ".rs") {
        const edges = await scanRustFile(state.root, rel, state.internalCrates ?? new Map())
        const imports = new Set<string>()
        for (const edge of edges) {
          const target = crateRootFile(edge.toFile.split(path.sep).join("/"), known)
          if (target && target !== rel) imports.add(target)
        }
        entry.imports = [...imports]
      }
      if (TEXTUAL_PATTERNS[ext]) {
        entry.symbols = textualSymbols(await Filesystem.readText(path.join(state.root, rel)), ext)
        entry.textual = true
      }
    } catch (error) {
      log.warn("scan failed", { file: rel, error: String(error) })
    }
    return entry
  }

  async function statFile(abs: string): Promise<{ mtime: number; size: number } | undefined> {
    try {
      const stat = await fs.stat(abs)
      if (!stat.isFile()) return undefined
      return { mtime: Math.floor(stat.mtimeMs), size: stat.size }
    } catch {
      return undefined
    }
  }

  // ---------------------------------------------------------------- cache

  export function cachePath(root: string): string {
    return path.join(root, ".gizzi", "repo-map.json")
  }

  async function readCache(root: string): Promise<CacheFile | undefined> {
    const file = cachePath(root)
    if (!(await Filesystem.exists(file))) return undefined
    try {
      const parsed = (await Filesystem.readJson(file)) as CacheFile
      if (parsed?.version !== CACHE_VERSION || typeof parsed.files !== "object") return undefined
      return parsed
    } catch {
      return undefined
    }
  }

  /** False when the drop guard refused the write (the old cache and in-memory map stay). */
  function passesDropGuard(root: string, before: number, after: number): boolean {
    if (before < DROP_GUARD_MIN_ENTRIES) return true
    if (after >= before * (1 - DROP_GUARD_RATIO)) return true
    log.warn("refusing to replace repo map: update would drop more than 80% of entries", { root, before, after })
    return false
  }

  async function writeCache(state: RootState) {
    const data: CacheFile = {
      version: CACHE_VERSION,
      generatedAt: new Date().toISOString(),
      files: Object.fromEntries([...state.files].sort(([a], [b]) => a.localeCompare(b))),
      history: state.history,
    }
    try {
      await Filesystem.mkdir(path.dirname(cachePath(state.root)), { recursive: true })
      await Filesystem.writeJson(cachePath(state.root), data)
    } catch (error) {
      log.warn("could not write repo map cache", { root: state.root, error: String(error) })
    }
  }

  // ---------------------------------------------------------------- state

  function stateFor(root: string): RootState {
    const key = path.resolve(root)
    let state = roots.get(key)
    if (!state) {
      state = { root: key, files: new Map(), history: [], recency: new Map(), queue: Promise.resolve(), pending: new Set() }
      roots.set(key, state)
    }
    return state
  }

  /** Run `fn` after whatever this root is already doing: one scan per project at a time. */
  function serialize<T>(state: RootState, fn: () => Promise<T>): Promise<T> {
    const next = state.queue.then(fn, fn)
    state.queue = next.catch(() => undefined)
    return next
  }

  async function fullScan(state: RootState): Promise<boolean> {
    const started = Date.now()
    const [listed, history, cache] = await Promise.all([listFiles(state.root), readHistory(state.root), readCache(state.root)])
    const recency = recencyOf(history)
    const files = capFiles(listed, recency)
    const fileSet = new Set(files)
    const known = (rel: string) => fileSet.has(rel)
    const previous = new Map<string, FileEntry>(cache ? Object.entries(cache.files) : state.files)
    let rescanned = 0
    const needsMaps = files.some((rel) => {
      const ext = path.extname(rel).toLowerCase()
      return TS_EXTENSIONS.has(ext) || ext === ".rs"
    })
    if (needsMaps) await internalMaps(state)

    const entries = await mapWithConcurrency(files, SCAN_CONCURRENCY, async (rel): Promise<[string, FileEntry] | undefined> => {
      const stat = await statFile(path.join(state.root, rel))
      if (!stat) return undefined
      const cached = previous.get(rel)
      if (cached && cached.mtime === stat.mtime && cached.size === stat.size) return [rel, cached]
      rescanned++
      return [rel, await scanFile(state, rel, stat, known)]
    })
    const next = new Map(entries.filter((entry): entry is [string, FileEntry] => !!entry))

    if (!passesDropGuard(state.root, previous.size, next.size)) {
      // Keep the last good map in memory too, not just on disk.
      if (state.files.size === 0 && cache) state.files = new Map(Object.entries(cache.files))
      if (state.history.length === 0 && cache) {
        state.history = cache.history ?? []
        state.recency = recencyOf(state.history)
      }
      return state.files.size > 0
    }
    state.files = next
    state.history = history
    state.recency = recency
    if (rescanned > 0 || !cache || cache.files === undefined || Object.keys(cache.files).length !== next.size) {
      await writeCache(state)
    }
    log.info("repo map ready", { root: state.root, files: next.size, rescanned, ms: Date.now() - started })
    return true
  }

  /** Scan (or reload from cache) once per root; later calls share the same result. */
  export function load(root: string): Promise<boolean> {
    const state = stateFor(root)
    if (!state.ready) {
      state.ready = serialize(state, () => fullScan(state)).catch((error) => {
        log.warn("repo map scan failed", { root: state.root, error: String(error) })
        state.ready = undefined // let a later turn retry
        return false
      })
    }
    return state.ready
  }

  function relTo(root: string, file: string): string | undefined {
    const abs = path.isAbsolute(file) ? file : path.join(root, file)
    const rel = path.relative(root, abs)
    if (!rel || rel.startsWith("..") || path.isAbsolute(rel)) return undefined
    return rel.split(path.sep).join("/")
  }

  /**
   * Rescan the given files now (absolute, or relative to `root`). Without a
   * `root`, each path goes to the loaded project that contains it. Deleted
   * files drop out; the drop guard still applies to the write.
   */
  export async function update(changedPaths: string[], root?: string): Promise<void> {
    const targets = root ? [stateFor(root)] : [...roots.values()]
    await Promise.all(
      targets.map((state) =>
        serialize(state, async () => {
          // Excluded paths never enter the map — including our own cache file
          // under .gizzi/, whose write would otherwise echo back through the
          // file watcher as another update.
          const ig = await loadIgnore(state.root)
          const rels = [
            ...new Set(changedPaths.map((file) => relTo(state.root, file)).filter((rel): rel is string => !!rel)),
          ].filter((rel) => !isExcluded(rel) && !ig?.ignores(rel))
          if (rels.length === 0) return
          const next = new Map(state.files)
          const known = (rel: string) => next.has(rel)
          if (rels.some((rel) => TS_EXTENSIONS.has(path.extname(rel).toLowerCase()) || rel.endsWith(".rs"))) {
            await internalMaps(state)
          }
          let changed = false
          for (const rel of rels) {
            const stat = await statFile(path.join(state.root, rel))
            const current = next.get(rel)
            if (!stat) {
              changed = next.delete(rel) || changed
              continue
            }
            if (current && current.mtime === stat.mtime && current.size === stat.size) continue
            if (!current && next.size >= MAX_FILES) continue
            next.set(rel, await scanFile(state, rel, stat, known))
            changed = true
          }
          if (!changed) return
          if (!passesDropGuard(state.root, state.files.size, next.size)) return
          state.files = next
          // A file this session wrote is the newest change there is.
          for (const rel of rels) state.recency.set(rel, -1)
          await writeCache(state)
        }).catch((error) => log.warn("repo map update failed", { root: state.root, error: String(error) })),
      ),
    )
  }

  /** Debounced `update` for tool writes: never awaited by the tool, batches bursts of edits. */
  export function queue(changedPaths: string[]) {
    for (const state of roots.values()) {
      const rels = changedPaths.filter((file) => relTo(state.root, file))
      if (rels.length === 0) continue
      for (const file of rels) state.pending.add(file)
      if (state.timer) clearTimeout(state.timer)
      state.timer = setTimeout(() => {
        state.timer = undefined
        const batch = [...state.pending]
        state.pending.clear()
        void update(batch, state.root)
      }, DEBOUNCE_MS)
      state.timer.unref?.()
    }
  }

  /** Record that a session's tool wrote `file` (absolute): boosts it and marks it `[NEW]`. */
  export function noteEdited(sessionID: string, file: string) {
    let set = editedBySession.get(sessionID)
    if (!set) {
      set = new Set()
      editedBySession.set(sessionID, set)
      while (editedBySession.size > MAX_SESSIONS) editedBySession.delete(editedBySession.keys().next().value!)
    }
    set.add(path.resolve(file))
  }

  // ---------------------------------------------------------------- ranking + slice

  export interface RankedFile {
    path: string
    score: number
    inDegree: number
    symbols: string[]
    edited: boolean
  }

  export interface SliceInput {
    root: string
    budgetTokens?: number
    /** Files the session read or mentioned (absolute or root-relative). */
    boost?: string[]
    /** Session whose edits get `[NEW]` and the boost. */
    sessionID?: string
    /** Ignore the budget and list every indexed file. */
    all?: boolean
  }

  export function rank(input: { root: string; boost?: string[]; sessionID?: string }): RankedFile[] {
    const state = roots.get(path.resolve(input.root))
    if (!state || state.files.size === 0) return []
    const edited = new Set<string>()
    for (const file of editedBySession.get(input.sessionID ?? "") ?? []) {
      const rel = relTo(state.root, file)
      if (rel) edited.add(rel)
    }
    const boosted = new Set(edited)
    for (const file of input.boost ?? []) {
      const rel = relTo(state.root, file)
      if (rel) boosted.add(rel)
    }

    const inDegree = new Map<string, number>()
    for (const [file, entry] of state.files) {
      for (const target of entry.imports) {
        if (target === file || !state.files.has(target)) continue
        inDegree.set(target, (inDegree.get(target) ?? 0) + 1)
      }
    }

    // Co-change: every commit that touched a boosted file lends +1 to the
    // other files in it (oversized commits skipped — they carry no signal).
    const cochange = new Map<string, number>()
    if (boosted.size > 0) {
      for (const commit of state.history) {
        if (commit.length > MAX_COCHANGE_COMMIT_FILES) continue
        if (!commit.some((file) => boosted.has(file))) continue
        for (const file of commit) {
          if (!boosted.has(file)) cochange.set(file, (cochange.get(file) ?? 0) + 1)
        }
      }
    }

    const ranked: RankedFile[] = []
    for (const [file, entry] of state.files) {
      const degree = inDegree.get(file) ?? 0
      const raw = degree + (cochange.get(file) ?? 0)
      // The floor keeps a boosted leaf file (nothing imports it) above the
      // unboosted ones it would otherwise tie with at zero.
      const score = boosted.has(file) ? raw * SESSION_BOOST + SESSION_BOOST : raw
      ranked.push({ path: file, score, inDegree: degree, symbols: entry.symbols, edited: edited.has(file) })
    }
    const recency = (file: string) => state.recency.get(file) ?? Number.MAX_SAFE_INTEGER
    ranked.sort((a, b) => b.score - a.score || recency(a.path) - recency(b.path) || a.path.localeCompare(b.path))
    return ranked
  }

  function line(file: RankedFile): string {
    const parts = [file.path]
    if (file.inDegree > 0) parts.push(`(→${file.inDegree})`)
    for (const symbol of file.symbols.slice(0, MAX_SYMBOLS_SHOWN)) parts.push(`+${symbol}`)
    if (file.edited) parts.push("[NEW]")
    return parts.join(" ")
  }

  /**
   * The compact, ranked text block for the prompt. Empty string when there
   * is no map for `root` (not loaded, empty, or failed) — never a partial
   * or invented one.
   */
  export function slice(input: SliceInput): string {
    try {
      const state = roots.get(path.resolve(input.root))
      if (!state || state.files.size === 0) return ""
      const ranked = rank(input)
      const budget = input.budgetTokens ?? DEFAULT_BUDGET_TOKENS
      const legend = "(→N) = imported by N files · +Name = exported symbol · [NEW] = changed this session"
      const lines: string[] = []
      const render = (shown: string[]) =>
        [`<repo_map files="${state.files.size}" shown="${shown.length}">`, legend, ...shown, "</repo_map>"].join("\n")
      for (const file of ranked) {
        const text = line(file)
        // Measured on the whole block (header and footer included), so the
        // budget holds exactly for the estimator the context panel uses.
        if (!input.all && Token.estimate(render([...lines, text])) > budget) break
        lines.push(text)
      }
      if (lines.length === 0) return ""
      return render(lines)
    } catch (error) {
      log.warn("repo map slice failed", { root: input.root, error: String(error) })
      return ""
    }
  }

  // ---------------------------------------------------------------- prompt

  export function disabledByEnv(): boolean {
    const value = process.env["GIZZI_NO_REPO_MAP"]
    return value === "1" || value?.toLowerCase() === "true"
  }

  const PROJECT_MARKERS = [
    ".git",
    ".gizzi",
    "package.json",
    "Cargo.toml",
    "pyproject.toml",
    "go.mod",
    "Gemfile",
    "pom.xml",
    "build.gradle",
    "composer.json",
    "deno.json",
  ]

  /** A real project folder: not the home or filesystem root, and carrying a VCS dir or a manifest. */
  export async function isProjectDir(dir: string | undefined): Promise<boolean> {
    if (!dir) return false
    const resolved = path.resolve(dir)
    if (resolved === path.resolve(os.homedir()) || resolved === path.parse(resolved).root) return false
    for (const marker of PROJECT_MARKERS) {
      if (await Filesystem.exists(path.join(resolved, marker))) return true
    }
    return false
  }

  /** How long a turn waits for a first scan before going without the map this once. */
  const FIRST_SCAN_WAIT_MS = 1_500

  /**
   * The repo map block for a session's turn. Rebuilt once per user turn
   * (`turn` = the user message id) so every step of one turn sends the same
   * system prefix. A first scan that takes longer than FIRST_SCAN_WAIT_MS
   * keeps running in the background; that turn simply has no map.
   */
  export async function prompt(input: {
    root: string | undefined
    sessionID: string
    turn: string
    boost?: string[]
    enabled?: boolean
    budgetTokens?: number
  }): Promise<string> {
    try {
      if (input.enabled === false || disabledByEnv()) return ""
      if (!(await isProjectDir(input.root))) return ""
      const root = path.resolve(input.root!)
      const cacheKey = `${input.sessionID}\0${root}`
      const cached = promptCache.get(cacheKey)
      if (cached?.turn === input.turn) return cached.text
      const ready = await Promise.race([
        load(root),
        new Promise<false>((resolve) => setTimeout(() => resolve(false), FIRST_SCAN_WAIT_MS).unref?.()),
      ])
      if (!ready) return ""
      const text = slice({ root, sessionID: input.sessionID, boost: input.boost, budgetTokens: input.budgetTokens })
      promptCache.set(cacheKey, { turn: input.turn, text })
      while (promptCache.size > MAX_SESSIONS) promptCache.delete(promptCache.keys().next().value!)
      return text
    } catch (error) {
      log.warn("repo map prompt failed", { sessionID: input.sessionID, error: String(error) })
      return ""
    }
  }

  /** Test hook: forget every root, session and cached prompt. */
  export function reset() {
    for (const state of roots.values()) if (state.timer) clearTimeout(state.timer)
    roots.clear()
    editedBySession.clear()
    promptCache.clear()
  }
}
