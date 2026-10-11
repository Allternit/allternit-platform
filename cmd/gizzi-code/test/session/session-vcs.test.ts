import { describe, expect, test } from "bun:test"
import { chmod, mkdtemp, rm, writeFile } from "fs/promises"
import { tmpdir } from "os"
import path from "path"
import { SessionVcs } from "../../src/runtime/session/session-vcs"

async function sh(cwd: string, ...cmd: string[]) {
  const proc = Bun.spawn(cmd, { cwd, stdout: "pipe", stderr: "pipe" })
  if ((await proc.exited) !== 0) throw new Error(`${cmd.join(" ")}: ${await new Response(proc.stderr).text()}`)
}

async function repo() {
  const dir = await mkdtemp(path.join(tmpdir(), "session-vcs-"))
  await sh(dir, "git", "init", "-q", "-b", "main")
  await sh(dir, "git", "config", "user.email", "t@example.com")
  await sh(dir, "git", "config", "user.name", "T")
  await writeFile(path.join(dir, "a.txt"), "one\ntwo\nthree\n")
  await writeFile(path.join(dir, "gone.txt"), "bye\n")
  await sh(dir, "git", "add", ".")
  await sh(dir, "git", "commit", "-q", "-m", "base")
  await sh(dir, "git", "checkout", "-q", "-b", "feature")
  await writeFile(path.join(dir, "a.txt"), "one\nTWO\nthree\nfour\n")
  await sh(dir, "git", "rm", "-q", "gone.txt")
  await sh(dir, "git", "commit", "-q", "-am", "work")
  // Uncommitted and untracked work counts too.
  await writeFile(path.join(dir, "new.ts"), "export const x = 1\nexport const y = 2\n")
  return dir
}

describe("SessionVcs.changes", () => {
  test("diffs the branch against its base, including uncommitted and untracked files", async () => {
    const dir = await repo()
    try {
      const changes = await SessionVcs.changes(dir)
      expect(changes.branch).toBe("feature")
      expect(changes.base).toBe("main")
      expect(changes.mergeBase).toMatch(/^[0-9a-f]{40}$/)
      expect(changes.files.map((f) => [f.file, f.status, f.additions, f.deletions])).toEqual([
        ["a.txt", "modified", 2, 1],
        ["gone.txt", "deleted", 0, 1],
        ["new.ts", "added", 2, 0],
      ])
      const a = changes.files[0]
      expect(a.before).toBe("one\ntwo\nthree\n")
      expect(a.after).toBe("one\nTWO\nthree\nfour\n")
      expect(changes.files[1]).toMatchObject({ before: "bye\n", after: "" })
      expect(changes).toMatchObject({ additions: 4, deletions: 2 })
    } finally {
      await rm(dir, { recursive: true, force: true })
    }
  })

  test("is empty outside a git repo", async () => {
    const dir = await mkdtemp(path.join(tmpdir(), "session-vcs-nogit-"))
    try {
      expect(await SessionVcs.changes(dir)).toEqual({ branch: null, base: null, mergeBase: null, additions: 0, deletions: 0, files: [] })
    } finally {
      await rm(dir, { recursive: true, force: true })
    }
  })
})

describe("SessionVcs pull request", () => {
  test("maps check runs and status contexts to states", () => {
    const checks = SessionVcs.parseChecks([
      { __typename: "CheckRun", name: "test", status: "COMPLETED", conclusion: "SUCCESS", detailsUrl: "u1", workflowName: "CI" },
      { __typename: "CheckRun", name: "lint", status: "IN_PROGRESS", conclusion: "" },
      { __typename: "CheckRun", name: "e2e", status: "QUEUED" },
      { __typename: "CheckRun", name: "build", status: "COMPLETED", conclusion: "FAILURE" },
      { __typename: "CheckRun", name: "docs", status: "COMPLETED", conclusion: "SKIPPED" },
      { __typename: "StatusContext", context: "deploy", state: "PENDING", targetUrl: "u2" },
      { __typename: "StatusContext", context: "cla", state: "SUCCESS" },
    ])
    expect(checks.map((c) => [c.name, c.state])).toEqual([
      ["test", "passed"],
      ["lint", "in_progress"],
      ["e2e", "queued"],
      ["build", "failed"],
      ["docs", "skipped"],
      ["deploy", "in_progress"],
      ["cla", "passed"],
    ])
    expect(checks[0]).toMatchObject({ workflow: "CI", url: "u1" })
    expect(SessionVcs.repoFromUrl("https://github.com/Allternit/allternit-ai/pull/526")).toBe("Allternit/allternit-ai")
  })

  test("reads the branch's PR through gh, with counts and auto-merge availability", async () => {
    const dir = await mkdtemp(path.join(tmpdir(), "session-vcs-gh-"))
    const fake = path.join(dir, "gh")
    await writeFile(
      fake,
      `#!/bin/sh
case "$1 $2" in
  "pr view") echo '{"number":7,"url":"https://github.com/o/r/pull/7","title":"T","state":"OPEN","isDraft":false,"headRefName":"feature","baseRefName":"main","additions":5,"deletions":1,"mergeStateStatus":"BLOCKED","autoMergeRequest":null,"statusCheckRollup":[{"__typename":"CheckRun","name":"a","status":"IN_PROGRESS"},{"__typename":"CheckRun","name":"b","status":"COMPLETED","conclusion":"SUCCESS"}]}' ;;
  "repo view") echo '{"autoMergeAllowed":false}' ;;
  "pr merge") echo "auto-merge is not allowed" >&2; exit 1 ;;
esac
`,
    )
    await chmod(fake, 0o755)
    process.env.GIZZI_GH_BIN = fake
    try {
      const { pr, reason } = await SessionVcs.pullRequest(dir)
      expect(reason).toBeNull()
      expect(pr).toMatchObject({
        number: 7,
        repo: "o/r",
        branch: "feature",
        base: "main",
        state: "open",
        additions: 5,
        deletions: 1,
        autoMerge: { enabled: false, available: false },
      })
      expect(pr?.counts).toMatchObject({ in_progress: 1, passed: 1, failed: 0 })
      expect(pr?.autoMerge.reason).toMatch(/isn't enabled/)
      expect(await SessionVcs.setAutoMerge(dir, 7, true)).toEqual({ ok: false, reason: "auto-merge is not allowed" })
    } finally {
      delete process.env.GIZZI_GH_BIN
      await rm(dir, { recursive: true, force: true })
    }
  })

  test("no PR for the branch is not an error; a missing gh is", async () => {
    const dir = await mkdtemp(path.join(tmpdir(), "session-vcs-nopr-"))
    const fake = path.join(dir, "gh")
    await writeFile(fake, `#!/bin/sh\necho 'no pull requests found for branch "feature"' >&2\nexit 1\n`)
    await chmod(fake, 0o755)
    try {
      process.env.GIZZI_GH_BIN = fake
      expect(await SessionVcs.pullRequest(dir)).toEqual({ pr: null, reason: null })
      process.env.GIZZI_GH_BIN = path.join(dir, "missing-gh")
      expect((await SessionVcs.pullRequest(dir)).reason).toBe("GitHub CLI (gh) is not installed.")
    } finally {
      delete process.env.GIZZI_GH_BIN
      await rm(dir, { recursive: true, force: true })
    }
  })
})
