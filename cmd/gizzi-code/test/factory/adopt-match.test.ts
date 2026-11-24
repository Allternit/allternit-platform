import { describe, expect, test } from "bun:test"
import { errorSentence, matchSession, refuseMove, type LiveSession } from "@/cli/commands/factory/adopt-core"

const rows: LiveSession[] = [
  { address: "eoj/adopt-probe", name: "adopt-probe", harness: "claude", sessionId: "aaaa1111-2222" },
  { address: "eoj/adopt-other", name: "adopt-other", harness: "codex", sessionId: "bbbb3333-4444" },
  { address: "eoj/site", name: "site", harness: "claude", sessionId: "aaaa9999-0000" },
]

describe("matchSession", () => {
  test("exact id or address wins", () => {
    expect(matchSession(rows, "bbbb3333-4444")).toMatchObject({ kind: "one", session: { name: "adopt-other" } })
    expect(matchSession(rows, "EOJ/SITE")).toMatchObject({ kind: "one", session: { name: "site" } })
  })
  test("exact name beats a prefix of another", () => {
    expect(matchSession(rows, "site")).toMatchObject({ kind: "one", session: { address: "eoj/site" } })
  })
  test("unique prefix matches, shared prefix is ambiguous", () => {
    expect(matchSession(rows, "adopt-p")).toMatchObject({ kind: "one", session: { name: "adopt-probe" } })
    const m = matchSession(rows, "adopt")
    expect(m.kind).toBe("many")
    expect(matchSession(rows, "aaaa").kind).toBe("many")
  })
  test("nothing matches", () => {
    expect(matchSession(rows, "zzz").kind).toBe("none")
    expect(matchSession(rows, "  ").kind).toBe("none")
  })
})

describe("move rules", () => {
  test("refuses here, no_resume and open_elsewhere; allows now and busy", () => {
    expect(refuseMove({ ...rows[0], movable: "here" })).toContain("already in Allternit")
    expect(refuseMove({ ...rows[0], movable: "no_resume" })).toContain("--new")
    const many = refuseMove({
      ...rows[0],
      movable: "open_elsewhere",
      holders: [
        { pid: 11, app: "Terminal" },
        { pid: 22, app: "iTerm2" },
      ],
    })
    expect(many).toContain("Terminal pid 11")
    expect(many).toContain("iTerm2 pid 22")
    expect(refuseMove({ ...rows[0], movable: "now" })).toBeNull()
    expect(refuseMove({ ...rows[0], movable: "busy" })).toBeNull()
  })
  test("a 404 says the runtime is too old", () => {
    expect(errorSentence(404)).toBe("This Allternit runtime is too old for adopt; update Allternit Desktop.")
  })
})
