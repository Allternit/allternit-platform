import { beforeEach, describe, expect, test } from "bun:test"
import { FileEditors } from "../../src/runtime/session/file-editors"

const MIN = 60_000

describe("FileEditors", () => {
  beforeEach(() => FileEditors.reset())

  test("a lone session has edited files and no other editors", () => {
    expect(FileEditors.record("/p/a.ts", "ses_a", 1_000)).toEqual([])
    FileEditors.record("/p/b.ts", "ses_a", 2_000)
    const state = FileEditors.forSession("ses_a", 3_000)
    expect(state.edited).toEqual(["/p/b.ts", "/p/a.ts"])
    expect(state.files.every((f) => f.otherSessions.length === 0)).toBe(true)
  })

  test("a second session on the same file contends with the first", () => {
    FileEditors.record("/p/a.ts", "ses_a", 1_000)
    expect(FileEditors.record("/p/a.ts", "ses_b", 2_000)).toEqual(["ses_a"])
    const a = FileEditors.forSession("ses_a", 3_000)
    expect(a.files).toEqual([{ path: "/p/a.ts", otherSessions: [{ sessionID: "ses_b", lastEditAt: 2_000 }] }])
  })

  test("edits older than 10 minutes no longer contend, older than 30 are dropped", () => {
    FileEditors.record("/p/a.ts", "ses_a", 0)
    expect(FileEditors.record("/p/a.ts", "ses_b", 11 * MIN)).toEqual([])
    expect(FileEditors.forSession("ses_a", 11 * MIN).edited).toEqual(["/p/a.ts"])
    expect(FileEditors.forSession("ses_a", 31 * MIN).edited).toEqual([])
  })

  test("forget drops a deleted session everywhere", () => {
    FileEditors.record("/p/a.ts", "ses_a", 1_000)
    FileEditors.record("/p/a.ts", "ses_b", 2_000)
    FileEditors.forget("ses_b")
    expect(FileEditors.forSession("ses_b", 3_000).edited).toEqual([])
    expect(FileEditors.forSession("ses_a", 3_000).files[0].otherSessions).toEqual([])
  })
})
