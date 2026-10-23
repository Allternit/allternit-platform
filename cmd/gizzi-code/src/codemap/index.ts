/**
 * Deterministic codemap generation entrypoint. See
 * docs/LENS_ARCHITECTURE_REFERENCE.md-style plan doc for the full design —
 * summary: scan the repo, discover <=20 primary modules, extract real
 * (TS compiler API) or best-effort (Rust regex) cross-module edges,
 * mechanically derive flows, fingerprint each module, validate everything
 * against real source before writing, then emit docs/codemap/{codemap.json,
 * codemap.html,codemap.lock}. No LLM calls anywhere in this pipeline.
 *
 * Never throws on a normal "nothing to generate" or "validation failed"
 * outcome — callers (gizzi init's step 5, the standalone `gizzi codemap
 * generate` command) both treat a non-ok result as a warning, not a hard
 * failure; codemap generation must never block `gizzi init` itself.
 */
import path from "path"
import { Filesystem } from "@/shared/util/filesystem"
import { Git } from "@/shared/util/git"
import { Ripgrep } from "@/shared/file/ripgrep"
import { assembleCodemapJson, buildCodemapNodes } from "./assemble"
import { discoverModules, deriveEntrypoints, buildPackageNameMap, buildCrateNameMap } from "./discover"
import { rollUpToNodeEdges } from "./edges"
import { buildLock, diffStaleModules } from "./fingerprint"
import { detectFlows } from "./flows"
import { generateCodemapHtml } from "./render-html"
import { scanRustFile } from "./scan-rust"
import { scanTypeScriptFile } from "./scan-ts"
import { validateCodemap } from "./validate"
import type { FileEdge, GenerateCodemapResult } from "./types"

const TS_EXTENSIONS = new Set([".ts", ".tsx", ".js", ".jsx"])
const EXCLUDED_DIRS = ["node_modules", "dist", "build", "out", "target", ".git", ".next", ".turbo", "coverage"]
const SCAN_CONCURRENCY = 16

export async function mapWithConcurrency<T, R>(items: T[], limit: number, fn: (item: T) => Promise<R>): Promise<R[]> {
  const results: R[] = new Array(items.length)
  let cursor = 0
  async function worker() {
    while (true) {
      const i = cursor++
      if (i >= items.length) return
      results[i] = await fn(items[i]!)
    }
  }
  await Promise.all(Array.from({ length: Math.min(limit, items.length) || 1 }, worker))
  return results
}

export async function generateCodemap(dir: string): Promise<GenerateCodemapResult> {
  const root = dir

  const gitCheck = await Git.exec(["rev-parse", "--is-inside-work-tree"], { cwd: root })
  if ((await gitCheck.text()).trim() !== "true") {
    return { ok: false, reason: "not a git repository — codemap generation needs git for commit/fingerprint tracking", staleModules: [] }
  }

  const modules = await discoverModules(root)
  if (modules.length === 0) {
    return { ok: false, reason: "no modules discovered (empty repo?)", staleModules: [] }
  }

  const entrypointsMap = await deriveEntrypoints(root, modules)
  const [internalPackages, internalCrates] = await Promise.all([
    buildPackageNameMap(root, modules),
    buildCrateNameMap(root, modules),
  ])

  const files: string[] = []
  for await (const f of Ripgrep.files({ cwd: root })) files.push(f)

  const fileEdgeLists = await mapWithConcurrency(files, SCAN_CONCURRENCY, async (relFile): Promise<FileEdge[]> => {
    const ext = path.extname(relFile)
    if (TS_EXTENSIONS.has(ext)) return scanTypeScriptFile(root, relFile, internalPackages)
    if (ext === ".rs") return scanRustFile(root, relFile, internalCrates)
    return []
  })
  const fileEdges = fileEdgeLists.flat()

  const edges = rollUpToNodeEdges(fileEdges, modules)
  const nodes = buildCodemapNodes(modules, entrypointsMap)
  const flows = detectFlows(nodes, edges)

  const commitResult = await Git.exec(["rev-parse", "HEAD"], { cwd: root })
  const commit = (await commitResult.text()).trim()
  const generatedAt = new Date().toISOString()
  const scope = modules.map((m) => m.path)

  const json = assembleCodemapJson(nodes, edges, flows, scope, commit, generatedAt)
  const lock = await buildLock(root, modules, scope, EXCLUDED_DIRS, generatedAt)
  const staleModules = await diffStaleModules(root, lock)
  const repoName = path.basename(root)
  const html = generateCodemapHtml(json, repoName)

  const issues = await validateCodemap(root, json, html, lock, modules)
  if (issues.length > 0) {
    const summary = issues.slice(0, 3).join("; ") + (issues.length > 3 ? ` (+${issues.length - 3} more)` : "")
    return { ok: false, reason: `validation failed: ${summary}`, staleModules: [] }
  }

  const codemapDir = path.join(root, "docs", "codemap")
  await Filesystem.mkdir(codemapDir, { recursive: true })
  await Filesystem.writeJson(path.join(codemapDir, "codemap.json"), json)
  await Filesystem.write(path.join(codemapDir, "codemap.html"), html)
  await Filesystem.writeJson(path.join(codemapDir, "codemap.lock"), lock)

  return { ok: true, staleModules, codemap: json }
}
