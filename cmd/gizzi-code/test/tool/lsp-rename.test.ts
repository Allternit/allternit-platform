// @ts-nocheck
import { describe, test, expect, mock } from "bun:test"
import path from "path"
import fs from "fs/promises"
import { pathToFileURL } from "url"
import { tmpdir } from "../fixture/fixture"

// Mock the LSP layer (same mock.module + spread-the-real-module pattern as
// test/session/wrap-up.test.ts). Each test sets `lspState` to what the
// language server should report.
const real = await import("../../src/runtime/integrations/lsp")
const lspState: { hasClients: boolean; rename: any[]; touched: string[] } = { hasClients: false, rename: [], touched: [] }
mock.module("../../src/runtime/integrations/lsp", () => ({
  LSP: {
    ...real.LSP,
    hasClients: async () => lspState.hasClients,
    touchFile: async (file: string) => {
      lspState.touched.push(file)
    },
    diagnostics: async () => ({}),
    rename: async () => lspState.rename,
    documentSymbol: async () => [],
  },
}))

const { LspTool } = await import("../../src/runtime/tools/builtins/lsp")
const { SymbolEditTool } = await import("../../src/runtime/tools/builtins/symbol-edit")
const { Instance } = await import("../../src/runtime/context/project/instance")

function makeCtx() {
  const asks: any[] = []
  return {
    asks,
    ctx: {
      sessionID: "test-lsp-rename-session",
      messageID: "",
      callID: "",
      agent: "build",
      abort: AbortSignal.any([]),
      messages: [],
      metadata: () => {},
      ask: async (input: any) => {
        asks.push(input)
      },
    },
  }
}

const range = (line: number, start: number, end: number) => ({
  start: { line, character: start },
  end: { line, character: end },
})

describe("tool.lsp rename", () => {
  test("returns a clear message (no throw) when no language server handles the file", async () => {
    await using tmp = await tmpdir({ git: true })
    const file = path.join(tmp.path, "notes.xyz")
    await fs.writeFile(file, "foo\n")
    lspState.hasClients = false

    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const tool = await LspTool.init()
        const { ctx } = makeCtx()
        const result = await tool.execute(
          { operation: "rename", filePath: file, line: 1, character: 1, newName: "bar" },
          ctx,
        )
        expect(result.output).toContain("No language server is available")
        expect(result.output).toContain("edit tool")
        expect(await fs.readFile(file, "utf-8")).toBe("foo\n")
      },
    })
  })

  test("applies a multi-file WorkspaceEdit through the edit permission and reports counts", async () => {
    await using tmp = await tmpdir({ git: true })
    const a = path.join(tmp.path, "a.ts")
    const b = path.join(tmp.path, "b.ts")
    await fs.writeFile(a, "export function foo() {}\nfoo()\n")
    await fs.writeFile(b, 'import { foo } from "./a"\nfoo()\n')
    lspState.hasClients = true
    lspState.touched = []
    lspState.rename = [
      {
        serverID: "typescript",
        placeholder: "foo",
        edit: {
          changes: {
            [pathToFileURL(a).href]: [
              { range: range(1, 0, 3), newText: "bar" },
              { range: range(0, 16, 19), newText: "bar" },
            ],
          },
        },
      },
    ]
    lspState.rename[0].edit.changes[pathToFileURL(b).href] = [
      { range: range(0, 9, 12), newText: "bar" },
      { range: range(1, 0, 3), newText: "bar" },
    ]

    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const tool = await LspTool.init()
        const { ctx, asks } = makeCtx()
        const result = await tool.execute(
          { operation: "rename", filePath: a, line: 1, character: 17, newName: "bar" },
          ctx,
        )
        expect(await fs.readFile(a, "utf-8")).toBe("export function bar() {}\nbar()\n")
        expect(await fs.readFile(b, "utf-8")).toBe('import { bar } from "./a"\nbar()\n')
        expect(result.output).toContain("Renamed `foo` to `bar`: 4 edits in 2 files")
        expect(result.output).toContain("a.ts (2 edits)")
        expect(result.output).toContain("b.ts (2 edits)")
        const editAsk = asks.find((x) => x.permission === "edit")
        expect(editAsk.patterns.sort()).toEqual(["a.ts", "b.ts"])
        expect(lspState.touched).toEqual(expect.arrayContaining([a, b]))
      },
    })
  })

  test("applies documentChanges and lists skipped file operations", async () => {
    await using tmp = await tmpdir({ git: true })
    const a = path.join(tmp.path, "a.ts")
    await fs.writeFile(a, "const foo = 1\nfoo\n")
    lspState.hasClients = true
    lspState.rename = [
      {
        serverID: "typescript",
        edit: {
          documentChanges: [
            {
              textDocument: { uri: pathToFileURL(a).href, version: 1 },
              edits: [
                { range: range(0, 6, 9), newText: "baz" },
                { range: range(1, 0, 3), newText: "baz" },
              ],
            },
            { kind: "rename", oldUri: pathToFileURL(a).href, newUri: pathToFileURL(path.join(tmp.path, "baz.ts")).href },
          ],
        },
      },
    ]

    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const tool = await LspTool.init()
        const { ctx } = makeCtx()
        const result = await tool.execute(
          { operation: "rename", filePath: a, line: 1, character: 7, newName: "baz" },
          ctx,
        )
        expect(await fs.readFile(a, "utf-8")).toBe("const baz = 1\nbaz\n")
        expect(result.output).toContain("Renamed `foo` to `baz`: 2 edits in 1 file")
        expect(result.output).toContain("Not applied")
      },
    })
  })
})

describe("tool.symbol_edit without a language server", () => {
  test("returns a clear message for non-TS files", async () => {
    await using tmp = await tmpdir({ git: true })
    const file = path.join(tmp.path, "main.py")
    await fs.writeFile(file, "def foo():\n    return 1\n")
    lspState.hasClients = false

    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const tool = await SymbolEditTool.init()
        const { ctx } = makeCtx()
        const result = await tool.execute({ filePath: file, symbol: "foo", newCode: "def foo():\n    return 2" }, ctx)
        expect(result.output).toContain("No language server is available")
        expect(await fs.readFile(file, "utf-8")).toBe("def foo():\n    return 1\n")
      },
    })
  })
})
