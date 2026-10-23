// @ts-nocheck
import { describe, test, expect } from "bun:test"
import { pathToFileURL } from "url"
import { WorkspaceEdit } from "../../src/runtime/tools/builtins/workspace-edit"

const range = (sl: number, sc: number, el: number, ec: number) => ({
  start: { line: sl, character: sc },
  end: { line: el, character: ec },
})

describe("WorkspaceEdit.applyTextEdits", () => {
  test("applies edits bottom-up regardless of the order the server sent them", () => {
    const content = "const foo = 1\nconsole.log(foo)\nexport { foo }\n"
    const edits = [
      { range: range(0, 6, 0, 9), newText: "renamedFoo" },
      { range: range(2, 9, 2, 12), newText: "renamedFoo" },
      { range: range(1, 12, 1, 15), newText: "renamedFoo" },
    ]
    expect(WorkspaceEdit.applyTextEdits(content, edits)).toBe(
      "const renamedFoo = 1\nconsole.log(renamedFoo)\nexport { renamedFoo }\n",
    )
  })

  test("several edits on one line keep their offsets", () => {
    const content = "a + a + a"
    const edits = [0, 4, 8].map((c) => ({ range: range(0, c, 0, c + 1), newText: "bb" }))
    expect(WorkspaceEdit.applyTextEdits(content, edits)).toBe("bb + bb + bb")
  })

  test("inserts at the same position keep the server's order", () => {
    const edits = [
      { range: range(0, 0, 0, 0), newText: "A" },
      { range: range(0, 0, 0, 0), newText: "B" },
    ]
    expect(WorkspaceEdit.applyTextEdits("x", edits)).toBe("ABx")
  })

  test("handles CRLF line endings", () => {
    const content = "let x = 1\r\nreturn x\r\n"
    const edits = [
      { range: range(0, 4, 0, 5), newText: "y" },
      { range: range(1, 7, 1, 8), newText: "y" },
    ]
    expect(WorkspaceEdit.applyTextEdits(content, edits)).toBe("let y = 1\r\nreturn y\r\n")
  })

  test("refuses overlapping edits", () => {
    const edits = [
      { range: range(0, 0, 0, 5), newText: "a" },
      { range: range(0, 3, 0, 8), newText: "b" },
    ]
    expect(() => WorkspaceEdit.applyTextEdits("0123456789", edits)).toThrow(/Overlapping/)
  })
})

describe("WorkspaceEdit.collect", () => {
  const a = "/project/a.ts"
  const b = "/project/b.ts"
  const uri = (p: string) => pathToFileURL(p).href

  test("reads the `changes` shape across files", () => {
    const collected = WorkspaceEdit.collect({
      changes: {
        [uri(a)]: [{ range: range(0, 0, 0, 1), newText: "x" }],
        [uri(b)]: [
          { range: range(1, 0, 1, 1), newText: "y" },
          { range: range(2, 0, 2, 1), newText: "z" },
        ],
      },
    })
    expect([...collected.files.keys()].sort()).toEqual([a, b])
    expect(collected.files.get(b)).toHaveLength(2)
    expect(collected.skipped).toEqual([])
  })

  test("reads `documentChanges`, prefers it over `changes`, and reports resource operations", () => {
    const collected = WorkspaceEdit.collect({
      changes: { [uri("/project/ignored.ts")]: [{ range: range(0, 0, 0, 1), newText: "q" }] },
      documentChanges: [
        { textDocument: { uri: uri(a), version: 3 }, edits: [{ range: range(0, 0, 0, 1), newText: "x" }] },
        { textDocument: { uri: uri(a), version: 3 }, edits: [{ range: range(4, 0, 4, 1), newText: "w", annotationId: "n" }] },
        { kind: "rename", oldUri: uri(b), newUri: uri("/project/c.ts") },
        { kind: "create", uri: uri("/project/d.ts") },
      ],
    })
    expect([...collected.files.keys()]).toEqual([a])
    expect(collected.files.get(a)).toHaveLength(2)
    expect(collected.skipped).toEqual([`rename ${b} -> /project/c.ts`, "create /project/d.ts"])
  })
})
