import { describe, expect, test } from "bun:test"
import {
  applyOperations,
  collectEntries,
  CONFLICT_KEY,
  conflictEntries,
  conflictSiblingId,
  entryVersion,
  mergeOperations,
  renderEntry,
  type DriveFiles,
  type Entry,
} from "../../../src/runtime/memory/drive/format"

function entry(id: string, text: string, source = "gizzi:session/ses_a"): Entry {
  return { id, text, source, added: "2026-11-17", metadata: {} }
}

function tree(...entries: Entry[]): DriveFiles {
  return applyOperations({}, entries.map((e) => ({ UpsertEntry: { path: "notes.md", entry: e } })))
}

/** Replay our work (base → ours) onto their newer head. */
function replay(base: DriveFiles, ours: DriveFiles, theirs: DriveFiles, conflicts?: Map<string, string>): DriveFiles {
  return applyOperations(theirs, mergeOperations(base, ours, theirs, conflicts))
}

const texts = (files: DriveFiles) => [...collectEntries(files).values()].map((e) => e.entry.text).sort()

describe("three-way replay merge (wave D)", () => {
  test("edit/edit: their edit stays, ours is kept as a conflict_of sibling", () => {
    const base = tree(entry("x", "Lives in Saint Paul"))
    const ours = tree(entry("x", "Lives in Minneapolis"))
    const theirs = tree(entry("x", "Lives in Duluth", "/?session=web1"))
    const out = replay(base, ours, theirs)
    const byId = collectEntries(out)
    expect(byId.get("x")!.entry.text).toBe("Lives in Duluth")
    const siblings = conflictEntries(out)
    expect(siblings.length).toBe(1)
    expect(siblings[0]!.entry.text).toBe("Lives in Minneapolis")
    expect(siblings[0]!.entry.metadata[CONFLICT_KEY]).toBe("x")
    expect(siblings[0]!.entry.id).toBe(conflictSiblingId("x", renderEntry(entry("x", "Lives in Minneapolis"))))
  })

  test("we edit, they delete: the edit beats the delete", () => {
    const base = tree(entry("x", "Old text"), entry("keep", "Other fact"))
    const ours = tree(entry("x", "New text"), entry("keep", "Other fact"))
    const theirs = tree(entry("keep", "Other fact"))
    const out = replay(base, ours, theirs)
    expect(texts(out)).toEqual(["New text", "Other fact"])
    expect(conflictEntries(out)).toEqual([])
  })

  test("we delete, they edit: their edit stays", () => {
    const base = tree(entry("x", "Old text"))
    const ours = tree()
    const theirs = tree(entry("x", "Their newer text"))
    const out = replay(base, ours, theirs)
    expect(texts(out)).toEqual(["Their newer text"])
  })

  test("identical edits from two devices: one entry, theirs untouched, no conflict", () => {
    const base = tree(entry("x", "Old text"))
    const ours = tree(entry("x", "Same new text", "gizzi:session/ses_a"))
    const theirs = tree(entry("x", "Same new text", "/?session=web1"))
    const out = replay(base, ours, theirs)
    expect(out["notes.md"]).toBe(theirs["notes.md"])
    expect(conflictEntries(out)).toEqual([])
  })

  test("only we changed it: our edit applies; an unchanged delete applies", () => {
    const base = tree(entry("x", "Old text"), entry("gone", "Delete me"))
    const ours = tree(entry("x", "Our edit"))
    const theirs = tree(entry("x", "Old text"), entry("gone", "Delete me"), entry("new", "Their new fact"))
    const out = replay(base, ours, theirs)
    expect(texts(out)).toEqual(["Our edit", "Their new fact"])
  })

  test("later local commits on a conflicted entry update the same sibling", () => {
    const conflicts = new Map<string, string>()
    const base = tree(entry("x", "v0"))
    const step1 = tree(entry("x", "ours v1"))
    const theirs = tree(entry("x", "theirs v1"))
    const after1 = replay(base, step1, theirs, conflicts)
    const step2 = tree(entry("x", "ours v2"))
    const after2 = replay(step1, step2, after1, conflicts)
    expect(collectEntries(after2).get("x")!.entry.text).toBe("theirs v1")
    const siblings = conflictEntries(after2)
    expect(siblings.map((s) => s.entry.text)).toEqual(["ours v2"])
  })

  test("versions: a short hash of the stored line that changes with any edit", () => {
    const a = renderEntry(entry("x", "Text"))
    const b = renderEntry(entry("x", "Text!"))
    expect(entryVersion(a)).toMatch(/^[0-9a-f]{12}$/)
    expect(entryVersion(a)).toBe(entryVersion(a))
    expect(entryVersion(a)).not.toBe(entryVersion(b))
  })
})
