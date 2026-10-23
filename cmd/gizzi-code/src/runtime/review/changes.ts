import { $ } from "bun"
import { createTwoFilesPatch } from "diff"

/** What the reviewer looks at. The first source that has changes wins:
 * files this session edited (its snapshot diff), else uncommitted git work. */
export namespace ReviewChanges {
  export const MAX_FILES = 30
  export const MAX_DIFF_CHARS = 60_000

  export type Status = "added" | "modified" | "deleted"
  export interface File {
    path: string
    status: Status
  }
  export interface Changes {
    source: "session" | "git"
    files: File[]
    /** Unified diff, capped at MAX_DIFF_CHARS. */
    diff: string
    /** More files or diff existed than were included. */
    truncated: boolean
  }

  /** Shape of a session snapshot diff entry (Snapshot.FileDiff). */
  export interface SessionDiff {
    file: string
    before: string
    after: string
    status?: Status
  }

  export interface Deps {
    sessionDiff?: (sessionID: string) => Promise<SessionDiff[]>
  }

  async function defaultSessionDiff(sessionID: string): Promise<SessionDiff[]> {
    // Lazy: keeps this module importable without booting the session runtime.
    const { SessionSummary } = await import("@/runtime/session/summary")
    return (await SessionSummary.diff({ sessionID: sessionID as any })) as SessionDiff[]
  }

  function cap(text: string): { text: string; truncated: boolean } {
    if (text.length <= MAX_DIFF_CHARS) return { text, truncated: false }
    return { text: text.slice(0, MAX_DIFF_CHARS) + "\n… (diff truncated; read the files directly)", truncated: true }
  }

  export async function collect(input: { sessionID?: string; cwd: string }, deps: Deps = {}): Promise<Changes | null> {
    if (input.sessionID) {
      const diffs = await (deps.sessionDiff ?? defaultSessionDiff)(input.sessionID).catch(() => [] as SessionDiff[])
      const changed = diffs.filter((d) => d.status === "deleted" || d.before !== d.after)
      if (changed.length > 0) {
        const files = changed.slice(0, MAX_FILES).map((d) => ({
          path: d.file,
          status: d.status ?? (d.before === "" ? "added" : d.after === "" ? "deleted" : "modified"),
        })) satisfies File[]
        const patch = changed
          .slice(0, MAX_FILES)
          .map((d) => createTwoFilesPatch(`a/${d.file}`, `b/${d.file}`, d.before, d.after, "", "", { context: 3 }))
          .join("\n")
        const capped = cap(patch)
        return { source: "session", files, diff: capped.text, truncated: capped.truncated || changed.length > MAX_FILES }
      }
    }
    return collectGit(input.cwd)
  }

  /** Uncommitted changes: `git status --porcelain` covers tracked edits,
   * staged adds, deletions and untracked files, and works before the first commit. */
  export async function collectGit(cwd: string): Promise<Changes | null> {
    const inside = await $`git rev-parse --is-inside-work-tree`.cwd(cwd).quiet().nothrow()
    if (inside.exitCode !== 0) return null
    const status = await $`git -c core.quotepath=false status --porcelain=v1 -z --untracked-files=all`
      .cwd(cwd)
      .quiet()
      .nothrow()
      .text()
    const entries = status.split("\0")
    const all: File[] = []
    for (let i = 0; i < entries.length; i++) {
      const entry = entries[i]
      if (!entry || entry.length < 4) continue
      const code = entry.slice(0, 2)
      const file = entry.slice(3)
      // Renames/copies carry the original path as the next NUL field.
      if (code[0] === "R" || code[0] === "C") i++
      const statusOf: Status = code.includes("D") ? "deleted" : code === "??" || code.includes("A") ? "added" : "modified"
      all.push({ path: file, status: statusOf })
    }
    if (all.length === 0) return null
    const files = all.slice(0, MAX_FILES)
    const hasHead = (await $`git rev-parse --verify HEAD`.cwd(cwd).quiet().nothrow()).exitCode === 0
    const tracked = hasHead
      ? await $`git -c core.quotepath=false diff --no-ext-diff HEAD -- ${files.filter((f) => f.status !== "added").map((f) => f.path)}`
          .cwd(cwd)
          .quiet()
          .nothrow()
          .text()
      : ""
    const untracked: string[] = []
    for (const file of files.filter((f) => f.status === "added")) {
      const text = await Bun.file(`${cwd}/${file.path}`).text().catch(() => "")
      untracked.push(createTwoFilesPatch("/dev/null", `b/${file.path}`, "", text, "", "", { context: 3 }))
    }
    const capped = cap([tracked, ...untracked].filter(Boolean).join("\n"))
    return { source: "git", files, diff: capped.text, truncated: capped.truncated || all.length > MAX_FILES }
  }
}
