/**
 * Real (AST-verified) import-graph extraction for TS/TSX/JS/JSX, using the
 * TypeScript compiler API (already a direct dependency, zero new deps).
 *
 * Every produced edge is `type: "imports"` — this scans static import/
 * export/dynamic-import() relationships only, not a full cross-module call
 * graph (which would need type-checking, not just parsing, to resolve which
 * function a call expression actually reaches). That's an honest scope
 * limit, not an oversight: "calls"/"reads"/"writes"/"publishes"/"subscribes"
 * edges are left to future, more targeted heuristics rather than guessed
 * here.
 */
import path from "path"
import ts from "typescript"
import { Filesystem } from "@/shared/util/filesystem"
import type { FileEdge } from "./types"

const RESOLVE_EXTENSIONS = [".ts", ".tsx", ".js", ".jsx"]

interface TsConfigPaths {
  dir: string
  baseUrl: string
  paths: Record<string, string[]>
}

const tsconfigCache = new Map<string, TsConfigPaths | null>()

/** Walk up from `fromDir` (relative to `root`) to find the nearest tsconfig.json's `paths`/`baseUrl`. */
async function findTsConfigPaths(root: string, fromDir: string): Promise<TsConfigPaths | null> {
  let current = fromDir
  while (true) {
    // Keyed by absolute dir: one process can scan several roots (the live
    // repo map serves every project a server has open), and a bare relative
    // key would hand one repo's tsconfig paths to another.
    const key = path.join(root, current)
    if (tsconfigCache.has(key)) return tsconfigCache.get(key)!
    const candidate = path.join(root, current, "tsconfig.json")
    if (await Filesystem.exists(candidate)) {
      try {
        const raw = await Filesystem.readText(candidate)
        // tsconfig.json permits comments; strip them crudely before JSON.parse
        // rather than pulling in a JSONC parser for one field.
        const stripped = raw.replace(/\/\/.*$/gm, "").replace(/\/\*[\s\S]*?\*\//g, "")
        const parsed = JSON.parse(stripped) as { compilerOptions?: { baseUrl?: string; paths?: Record<string, string[]> } }
        const result: TsConfigPaths = {
          dir: current,
          baseUrl: parsed.compilerOptions?.baseUrl ?? ".",
          paths: parsed.compilerOptions?.paths ?? {},
        }
        tsconfigCache.set(key, result)
        return result
      } catch {
        tsconfigCache.set(key, null)
        return null
      }
    }
    if (current === "" || current === ".") {
      tsconfigCache.set(key, null)
      return null
    }
    current = path.dirname(current)
    if (current === ".") current = ""
  }
}

async function tryResolve(root: string, candidatePath: string): Promise<string | null> {
  const abs = path.join(root, candidatePath)
  if (await Filesystem.exists(abs)) {
    if (await Filesystem.isDir(abs)) {
      for (const ext of RESOLVE_EXTENSIONS) {
        const indexed = path.join(candidatePath, `index${ext}`)
        if (await Filesystem.exists(path.join(root, indexed))) return indexed
      }
      return null
    }
    return candidatePath
  }
  for (const ext of RESOLVE_EXTENSIONS) {
    if (await Filesystem.exists(abs + ext)) return candidatePath + ext
  }
  return null
}

/** Resolve an import specifier to a repo-relative file path, or null if external/unresolvable. */
async function resolveSpecifier(
  root: string,
  fromFile: string,
  specifier: string,
  internalPackages: Map<string, string>,
): Promise<string | null> {
  const fromDir = path.dirname(fromFile)

  if (specifier.startsWith(".")) {
    return tryResolve(root, path.normalize(path.join(fromDir, specifier)))
  }

  const tsconfig = await findTsConfigPaths(root, fromDir)
  if (tsconfig) {
    for (const [pattern, targets] of Object.entries(tsconfig.paths)) {
      const prefix = pattern.replace(/\*$/, "")
      if (pattern.includes("*") ? specifier.startsWith(prefix) : specifier === pattern) {
        const rest = pattern.includes("*") ? specifier.slice(prefix.length) : ""
        for (const target of targets) {
          const resolved = target.replace(/\*$/, rest)
          const candidate = path.normalize(path.join(tsconfig.dir, tsconfig.baseUrl, resolved))
          const found = await tryResolve(root, candidate)
          if (found) return found
        }
      }
    }
  }

  // Bare specifier — only resolve if it names a package that lives inside
  // this repo (real internal cross-package import); external npm packages
  // are deliberately not turned into edges (edges are intra-repo structure).
  for (const [pkgName, pkgDir] of internalPackages) {
    if (specifier === pkgName || specifier.startsWith(pkgName + "/")) {
      const sub = specifier === pkgName ? "" : specifier.slice(pkgName.length + 1)
      const candidate = sub ? path.join(pkgDir, "src", sub) : path.join(pkgDir, "src", "index")
      return tryResolve(root, candidate)
    }
  }

  return null
}

function collectSpecifiers(sourceFile: ts.SourceFile): Array<{ specifier: string; line: number }> {
  const results: Array<{ specifier: string; line: number }> = []

  function lineOf(node: ts.Node): number {
    return sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile)).line + 1
  }

  function visit(node: ts.Node) {
    if (
      (ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) &&
      node.moduleSpecifier &&
      ts.isStringLiteral(node.moduleSpecifier)
    ) {
      // Line of the specifier string itself, not the statement's `import`
      // keyword — a multi-line `import { a, b, c } from "..."` puts the
      // specifier several lines below the statement start, and evidence
      // must cite where the text actually is (validate.ts checks the
      // symbol literally appears on the cited line).
      results.push({ specifier: node.moduleSpecifier.text, line: lineOf(node.moduleSpecifier) })
    } else if (
      ts.isCallExpression(node) &&
      node.expression.kind === ts.SyntaxKind.ImportKeyword &&
      node.arguments[0] &&
      ts.isStringLiteral(node.arguments[0])
    ) {
      results.push({ specifier: node.arguments[0].text, line: lineOf(node.arguments[0]) })
    }
    ts.forEachChild(node, visit)
  }

  visit(sourceFile)
  return results
}

function hasExportModifier(node: ts.Node): boolean {
  const modifiers = ts.canHaveModifiers(node) ? ts.getModifiers(node) : undefined
  return modifiers?.some((m) => m.kind === ts.SyntaxKind.ExportKeyword) ?? false
}

/**
 * Top-level exported names, in source order, read from the AST (never
 * guessed): `export function/class/const/let/interface/type/enum/namespace`,
 * `export { a, b as c }` (the exported name), and `export default` (as
 * "default", or the declaration's own name when it has one).
 */
export function collectExports(sourceFile: ts.SourceFile): string[] {
  const names: string[] = []
  const add = (name: string | undefined) => {
    if (name && !names.includes(name)) names.push(name)
  }
  for (const statement of sourceFile.statements) {
    if (ts.isExportDeclaration(statement)) {
      if (statement.exportClause && ts.isNamedExports(statement.exportClause)) {
        for (const element of statement.exportClause.elements) add(element.name.text)
      }
      continue
    }
    if (ts.isExportAssignment(statement)) {
      add("default")
      continue
    }
    if (!hasExportModifier(statement)) continue
    if (ts.isVariableStatement(statement)) {
      for (const decl of statement.declarationList.declarations) {
        if (ts.isIdentifier(decl.name)) add(decl.name.text)
      }
    } else if (
      ts.isFunctionDeclaration(statement) ||
      ts.isClassDeclaration(statement) ||
      ts.isInterfaceDeclaration(statement) ||
      ts.isTypeAliasDeclaration(statement) ||
      ts.isEnumDeclaration(statement) ||
      ts.isModuleDeclaration(statement)
    ) {
      const name = statement.name && ts.isIdentifier(statement.name) ? statement.name.text : undefined
      add(name ?? "default")
    }
  }
  return names
}

export async function scanTypeScriptFile(
  root: string,
  relFile: string,
  internalPackages: Map<string, string>,
): Promise<FileEdge[]> {
  return (await scanTypeScriptFileDetailed(root, relFile, internalPackages)).edges
}

/** One parse, both outputs: the file's import edges and its exported names (repo-map.ts). */
export async function scanTypeScriptFileDetailed(
  root: string,
  relFile: string,
  internalPackages: Map<string, string>,
): Promise<{ edges: FileEdge[]; exports: string[] }> {
  const abs = path.join(root, relFile)
  const size = await Filesystem.size(abs)
  if (size > 500_000) return { edges: [], exports: [] } // skip very large generated/bundled files

  let content: string
  try {
    content = await Filesystem.readText(abs)
  } catch {
    return { edges: [], exports: [] }
  }

  const sourceFile = ts.createSourceFile(relFile, content, ts.ScriptTarget.Latest, true)
  const specifiers = collectSpecifiers(sourceFile)

  const edges: FileEdge[] = []
  for (const { specifier, line } of specifiers) {
    const resolved = await resolveSpecifier(root, relFile, specifier, internalPackages)
    if (!resolved || resolved === relFile) continue
    edges.push({
      fromFile: relFile,
      toFile: resolved,
      type: "imports",
      evidence: { file: relFile, line, symbol: specifier, confidence: "verified" },
    })
  }
  return { edges, exports: collectExports(sourceFile) }
}
