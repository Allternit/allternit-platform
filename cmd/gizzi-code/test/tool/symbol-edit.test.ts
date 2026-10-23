// @ts-nocheck
import { describe, test, expect } from "bun:test"
import {
  findTsSymbols,
  resolveSymbol,
  replaceSymbol,
  symbolsFromLsp,
} from "../../src/runtime/tools/builtins/symbol-edit"

const SOURCE = `import x from "y"

/** Adds numbers. */
export function add(a: number, b: number) {
  return a + b
}

export class Session {
  start() {
    return 1
  }

  /** Stops it. */
  stop = () => 2
}

export class Other {
  start() {
    return 3
  }
}

export const double = (n: number) => n * 2

export interface Shape {
  size: number
}

export type Id = string

export namespace Config {
  export function load() {
    return {}
  }
}
`

describe("symbol_edit TypeScript lookup", () => {
  const symbols = findTsSymbols(SOURCE, "file.ts")

  test("finds functions, classes, members, arrow consts, interfaces, types and namespace members", () => {
    const names = symbols.map((s) => `${s.kind}:${s.name}`)
    for (const expected of [
      "function:add",
      "class:Session",
      "method:Session.start",
      "method:Session.stop",
      "class:Other",
      "method:Other.start",
      "function:double",
      "interface:Shape",
      "type:Id",
      "namespace:Config",
      "function:Config.load",
    ]) {
      expect(names).toContain(expected)
    }
  })

  test("resolves a dotted Class.method", () => {
    const match = resolveSymbol(symbols, "Session.start")
    expect(SOURCE.slice(match.start, match.end)).toBe("start() {\n    return 1\n  }")
    expect(match.line).toBe(9)
  })

  test("reports an ambiguous plain name with candidates and line numbers", () => {
    expect(() => resolveSymbol(symbols, "start")).toThrow(/ambiguous[\s\S]*Session\.start \(method, line 9\)[\s\S]*Other\.start \(method, line 18\)/)
  })

  test("kind filter narrows a name", () => {
    expect(resolveSymbol(symbols, "load", "function").name).toBe("Config.load")
  })

  test("reports a missing name with the symbols in the file", () => {
    expect(() => resolveSymbol(symbols, "nope")).toThrow(/"nope" was not found[\s\S]*add \(function, line 4\)/)
  })

  test("replaces from the declaration keyword and keeps the doc comment", () => {
    const match = resolveSymbol(symbols, "add")
    const out = replaceSymbol(SOURCE, match, "export function add(a: number, b: number, c = 0) {\n  return a + b + c\n}\n")
    expect(out).toContain("/** Adds numbers. */\nexport function add(a: number, b: number, c = 0) {\n  return a + b + c\n}\n\nexport class Session")
  })

  test("replaces the attached doc comment too when newCode starts with one", () => {
    const match = resolveSymbol(symbols, "add")
    const out = replaceSymbol(SOURCE, match, "/** Sums. */\nexport function add() {\n  return 0\n}")
    expect(out).toContain('import x from "y"\n\n/** Sums. */\nexport function add() {\n  return 0\n}\n')
    expect(out).not.toContain("Adds numbers")
  })

  test("a const arrow function replaces the whole statement", () => {
    const match = resolveSymbol(symbols, "double")
    expect(SOURCE.slice(match.start, match.end)).toBe("export const double = (n: number) => n * 2")
  })
})

describe("symbol_edit LSP lookup", () => {
  test("walks hierarchical documentSymbol children for dotted names", () => {
    const source = "class Foo:\n    # does bar\n    def bar(self):\n        return 1\n"
    const symbols = symbolsFromLsp(source, [
      {
        name: "Foo",
        kind: 5,
        range: { start: { line: 0, character: 0 }, end: { line: 3, character: 16 } },
        selectionRange: { start: { line: 0, character: 6 }, end: { line: 0, character: 9 } },
        children: [
          {
            name: "bar",
            kind: 6,
            range: { start: { line: 2, character: 4 }, end: { line: 3, character: 16 } },
            selectionRange: { start: { line: 2, character: 8 }, end: { line: 2, character: 11 } },
          },
        ],
      },
    ])
    const match = resolveSymbol(symbols, "Foo.bar")
    expect(match.kind).toBe("method")
    expect(replaceSymbol(source, match, "def bar(self):\n        return 2")).toBe(
      "class Foo:\n    # does bar\n    def bar(self):\n        return 2\n",
    )
  })
})
