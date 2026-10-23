import z from "zod/v4"
import * as path from "path"
import ts from "typescript"
import { pathToFileURL } from "url"
import { Tool } from "@/runtime/tools/builtins/tool"
import { LSP } from "@/runtime/integrations/lsp"
import DESCRIPTION from "@/runtime/tools/builtins/symbol-edit.txt"
import { Filesystem } from "@/shared/util/filesystem"
import { Instance } from "@/runtime/context/project/instance"
import { assertExternalDirectory } from "@/runtime/tools/builtins/external-directory"
import { WorkspaceEdit } from "@/runtime/tools/builtins/workspace-edit"

export const SYMBOL_KINDS = [
  "function",
  "class",
  "method",
  "property",
  "constructor",
  "interface",
  "type",
  "enum",
  "variable",
  "namespace",
] as const
export type SymbolKindName = (typeof SYMBOL_KINDS)[number] | "other"

export type SymbolMatch = {
  /** Qualified name, e.g. `Foo.bar` for a method `bar` of class `Foo`. */
  name: string
  kind: SymbolKindName
  /** Offset of the declaration itself (modifiers/decorators/keyword), without leading comments. */
  start: number
  /** Offset of the attached leading comment block, or `start` when there is none. */
  commentStart: number
  end: number
  /** 1-based first and last line of the declaration. */
  line: number
  endLine: number
}

const TS_EXTENSIONS: Record<string, ts.ScriptKind> = {
  ".ts": ts.ScriptKind.TS,
  ".mts": ts.ScriptKind.TS,
  ".cts": ts.ScriptKind.TS,
  ".tsx": ts.ScriptKind.TSX,
  ".js": ts.ScriptKind.JS,
  ".mjs": ts.ScriptKind.JS,
  ".cjs": ts.ScriptKind.JS,
  ".jsx": ts.ScriptKind.JSX,
}

export function isTsLike(file: string) {
  return path.extname(file).toLowerCase() in TS_EXTENSIONS
}

function lineOf(source: string, offset: number) {
  let line = 1
  for (let i = 0; i < offset && i < source.length; i++) if (source.charCodeAt(i) === 10) line++
  return line
}

/** Start of the comment block directly attached (no blank line) above `start`. */
function attachedCommentStart(source: string, fullStart: number, start: number) {
  const ranges = ts.getLeadingCommentRanges(source, fullStart) ?? []
  let result = start
  for (let i = ranges.length - 1; i >= 0; i--) {
    const gap = source.slice(ranges[i].end, result)
    if (/\n[ \t]*\r?\n/.test(gap)) break
    result = ranges[i].pos
  }
  return result
}

function nameText(name: ts.Node | undefined, sf: ts.SourceFile): string | undefined {
  if (!name) return undefined
  if (ts.isIdentifier(name) || ts.isPrivateIdentifier(name) || ts.isStringLiteral(name) || ts.isNumericLiteral(name)) {
    return name.text
  }
  return name.getText(sf)
}

/**
 * Every named declaration in a TS/JS file, found with the TypeScript compiler
 * API (parse only, no type-check): functions, classes and their members,
 * const/let/var (arrow functions count as functions), interfaces, type
 * aliases, enums and namespaces. Exported for tests.
 */
export function findTsSymbols(source: string, fileName: string): SymbolMatch[] {
  const kind = TS_EXTENSIONS[path.extname(fileName).toLowerCase()] ?? ts.ScriptKind.TS
  const sf = ts.createSourceFile(fileName, source, ts.ScriptTarget.Latest, true, kind)
  const out: SymbolMatch[] = []

  const push = (name: string, symbolKind: SymbolKindName, node: ts.Node) => {
    const start = node.getStart(sf)
    const end = node.getEnd()
    out.push({
      name,
      kind: symbolKind,
      start,
      commentStart: attachedCommentStart(source, node.getFullStart(), start),
      end,
      line: lineOf(source, start),
      endLine: lineOf(source, end),
    })
  }

  const isFunctionLike = (node: ts.Node | undefined) =>
    !!node && (ts.isArrowFunction(node) || ts.isFunctionExpression(node))

  const visitClassMembers = (cls: ts.ClassLikeDeclaration, prefix: string) => {
    for (const member of cls.members) {
      if (ts.isConstructorDeclaration(member)) {
        push(`${prefix}.constructor`, "constructor", member)
        continue
      }
      const name = nameText(member.name, sf)
      if (!name) continue
      if (ts.isMethodDeclaration(member) || ts.isGetAccessor(member) || ts.isSetAccessor(member)) {
        push(`${prefix}.${name}`, "method", member)
      } else if (ts.isPropertyDeclaration(member)) {
        push(`${prefix}.${name}`, isFunctionLike(member.initializer) ? "method" : "property", member)
      }
    }
  }

  const visit = (statements: readonly ts.Statement[], prefix: string) => {
    const qualify = (name: string) => (prefix ? `${prefix}.${name}` : name)
    for (const stmt of statements) {
      if (ts.isFunctionDeclaration(stmt) && stmt.name) {
        push(qualify(stmt.name.text), "function", stmt)
      } else if (ts.isClassDeclaration(stmt) && stmt.name) {
        const name = qualify(stmt.name.text)
        push(name, "class", stmt)
        visitClassMembers(stmt, name)
      } else if (ts.isInterfaceDeclaration(stmt)) {
        push(qualify(stmt.name.text), "interface", stmt)
      } else if (ts.isTypeAliasDeclaration(stmt)) {
        push(qualify(stmt.name.text), "type", stmt)
      } else if (ts.isEnumDeclaration(stmt)) {
        push(qualify(stmt.name.text), "enum", stmt)
      } else if (ts.isModuleDeclaration(stmt)) {
        let name = qualify(nameText(stmt.name, sf) ?? "")
        push(name, "namespace", stmt)
        let body = stmt.body
        while (body && ts.isModuleDeclaration(body)) {
          name = `${name}.${nameText(body.name, sf)}`
          body = body.body
        }
        if (body && ts.isModuleBlock(body)) visit(body.statements, name)
      } else if (ts.isVariableStatement(stmt)) {
        const declarations = stmt.declarationList.declarations
        for (const decl of declarations) {
          if (!ts.isIdentifier(decl.name)) continue
          const name = qualify(decl.name.text)
          // One declaration: replace the whole statement (keyword included).
          const node = declarations.length === 1 ? stmt : decl
          if (decl.initializer && ts.isClassExpression(decl.initializer)) {
            push(name, "class", node)
            visitClassMembers(decl.initializer, name)
          } else {
            push(name, isFunctionLike(decl.initializer) ? "function" : "variable", node)
          }
        }
      }
    }
  }

  visit(sf.statements, "")
  return out
}

const LSP_KIND: Record<number, SymbolKindName> = {
  2: "namespace",
  3: "namespace",
  4: "namespace",
  5: "class",
  6: "method",
  7: "property",
  8: "property",
  9: "constructor",
  10: "enum",
  11: "interface",
  12: "function",
  13: "variable",
  14: "variable",
  23: "class",
  26: "type",
}

const COMMENT_LINE = /^\s*(\/\/|\/\*|\*|#|--|;)/

/** Flatten LSP documentSymbol results (hierarchical or SymbolInformation) into matches. Exported for tests. */
export function symbolsFromLsp(source: string, items: any[]): SymbolMatch[] {
  const out: SymbolMatch[] = []
  const lines = source.split("\n")
  const toMatch = (name: string, kind: number, range: LSP.Range) => {
    const start = WorkspaceEdit.offsetAt(source, range.start)
    const end = WorkspaceEdit.offsetAt(source, range.end)
    // Comment lines directly above the declaration count as its leading comment.
    let commentLine = range.start.line
    while (commentLine > 0 && COMMENT_LINE.test(lines[commentLine - 1] ?? "")) commentLine--
    const commentStart =
      commentLine < range.start.line ? WorkspaceEdit.offsetAt(source, { line: commentLine, character: 0 }) : start
    out.push({
      name,
      kind: LSP_KIND[kind] ?? "other",
      start,
      commentStart,
      end,
      line: range.start.line + 1,
      endLine: range.end.line + 1,
    })
  }
  const walk = (symbols: any[], prefix: string) => {
    for (const symbol of symbols ?? []) {
      if (!symbol?.name) continue
      if (symbol.range) {
        const name = prefix ? `${prefix}.${symbol.name}` : symbol.name
        toMatch(name, symbol.kind, symbol.range)
        walk(symbol.children, name)
      } else if (symbol.location?.range) {
        const name = symbol.containerName ? `${symbol.containerName}.${symbol.name}` : symbol.name
        toMatch(name, symbol.kind, symbol.location.range)
      }
    }
  }
  walk(items, "")
  return out
}

/**
 * Pick the one declaration `symbol` names. A full qualified-name match wins;
 * otherwise a plain name matches any member with that last segment
 * (`bar` → `Foo.bar`). Throws a clear error listing candidates when nothing or
 * more than one declaration matches. Exported for tests.
 */
export function resolveSymbol(all: SymbolMatch[], symbol: string, kind?: SymbolKindName): SymbolMatch {
  const pool = kind ? all.filter((item) => item.kind === kind) : all
  const exact = pool.filter((item) => item.name === symbol)
  const matches = exact.length ? exact : pool.filter((item) => item.name.endsWith(`.${symbol}`))
  const describe = (item: SymbolMatch) => `- ${item.name} (${item.kind}, line ${item.line})`

  if (matches.length === 1) return matches[0]
  if (matches.length > 1) {
    throw new Error(
      `Symbol "${symbol}" is ambiguous; ${matches.length} declarations match. Use a qualified name (e.g. Class.method) or pass kind:\n${matches.map(describe).join("\n")}`,
    )
  }
  const available = all.slice(0, 40).map(describe).join("\n")
  throw new Error(
    `Symbol "${symbol}"${kind ? ` of kind ${kind}` : ""} was not found in this file.` +
      (available ? ` Symbols in the file:\n${available}${all.length > 40 ? `\n... and ${all.length - 40} more` : ""}` : " The file has no named declarations."),
  )
}

/** Replace a matched declaration's range with `newCode`. Exported for tests. */
export function replaceSymbol(source: string, match: SymbolMatch, newCode: string): string {
  const code = newCode.replace(/^[ \t]+/, "").replace(/\s+$/, "")
  const startsWithComment = /^(\/\/|\/\*|#)/.test(code)
  const start = startsWithComment ? match.commentStart : match.start
  return source.slice(0, start) + code + source.slice(match.end)
}

const NO_SERVER_MESSAGE =
  "No language server is available for this file type and the built-in fallback only understands TypeScript/JavaScript, so the symbol cannot be located. Use the edit tool to replace the code instead."

export const SymbolEditTool = Tool.define("symbol_edit", {
  description: DESCRIPTION,
  parameters: z.object({
    filePath: z.string().describe("The absolute path to the file containing the symbol"),
    symbol: z
      .string()
      .describe("The symbol name, optionally qualified with dots (e.g. `parseConfig`, `Session.start`, `Ns.helper`)"),
    newCode: z
      .string()
      .describe(
        "The full replacement source for the symbol, signature included. Start it with its doc comment to replace the existing doc comment too.",
      ),
    kind: z
      .enum(SYMBOL_KINDS)
      .optional()
      .describe("Only match symbols of this kind (use it when a name is ambiguous)"),
  }),
  async execute(params, ctx) {
    if (!params.symbol?.trim()) throw new Error("symbol is required")
    const filePath = path.isAbsolute(params.filePath) ? params.filePath : path.join(Instance.workdir, params.filePath)
    await assertExternalDirectory(ctx, filePath)

    const stats = Filesystem.stat(filePath)
    if (!stats) throw new Error(`File ${filePath} not found`)
    if (stats.isDirectory()) throw new Error(`Path is a directory, not a file: ${filePath}`)

    const relative = path.relative(Instance.worktree, filePath)
    const before = await Filesystem.readText(filePath)
    const symbol = params.symbol.trim()

    let symbols: SymbolMatch[] = isTsLike(filePath) ? findTsSymbols(before, filePath) : []
    let source: "typescript" | "lsp" = "typescript"
    const tsHit = symbols.some(
      (item) => (!params.kind || item.kind === params.kind) && (item.name === symbol || item.name.endsWith(`.${symbol}`)),
    )
    if (!tsHit) {
      if (await LSP.hasClients(filePath)) {
        await LSP.touchFile(filePath, true)
        const fromLsp = symbolsFromLsp(before, await LSP.documentSymbol(pathToFileURL(filePath).href))
        if (fromLsp.length) {
          symbols = fromLsp
          source = "lsp"
        }
      } else if (!isTsLike(filePath)) {
        return {
          title: relative,
          metadata: { diagnostics: {}, diff: "" } as Record<string, any>,
          output: NO_SERVER_MESSAGE,
        }
      }
    }

    const match = resolveSymbol(symbols, symbol, params.kind)
    const after = replaceSymbol(before, match, params.newCode)
    if (after === before) {
      return {
        title: relative,
        metadata: { diagnostics: {}, diff: "" } as Record<string, any>,
        output: `No changes: the new code for ${match.name} is identical to the current code.`,
      }
    }

    const applied = await WorkspaceEdit.apply(ctx, [{ filePath, before, after, edits: 1 }], { assertRead: true })
    const filediff = applied.filediff[0]
    ctx.metadata({ metadata: { diff: applied.diff, filediff, diagnostics: {} } })

    return {
      title: relative,
      metadata: {
        diagnostics: applied.diagnostics,
        diff: applied.diff,
        filediff,
        symbol: match.name,
        source,
      } as Record<string, any>,
      output: `Replaced ${match.kind} ${match.name} (lines ${match.line}-${match.endLine}) in ${relative}.${applied.diagnosticsOutput}`,
    }
  },
})
