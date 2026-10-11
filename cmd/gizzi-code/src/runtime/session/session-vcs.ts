import z from "zod/v4"
import path from "path"
import { readFile, stat } from "fs/promises"

/**
 * What a code session's branch changes and the pull request it belongs to,
 * for the surfaces' diff pane and PR chip (Claude Code parity: `main →
 * branch`, per-file diffs, CI checks, auto-merge).
 *
 * Plain git and the GitHub CLI (`gh`) run in the session's directory: no
 * Instance state, so the routes can serve any session the daemon knows.
 */
export namespace SessionVcs {
  export const FileChange = z.object({
    file: z.string(),
    status: z.enum(["added", "deleted", "modified"]),
    additions: z.number(),
    deletions: z.number(),
    binary: z.boolean(),
    /** Full text at the merge base and in the working tree; omitted (null) for binary or oversized files. */
    before: z.string().nullable(),
    after: z.string().nullable(),
  })
  export type FileChange = z.infer<typeof FileChange>

  export const Changes = z.object({
    branch: z.string().nullable(),
    base: z.string().nullable(),
    mergeBase: z.string().nullable(),
    additions: z.number(),
    deletions: z.number(),
    files: FileChange.array(),
  })
  export type Changes = z.infer<typeof Changes>

  export const CheckState = z.enum(["queued", "in_progress", "passed", "failed", "skipped", "cancelled"])
  export type CheckState = z.infer<typeof CheckState>

  export const Check = z.object({
    name: z.string(),
    workflow: z.string().nullable(),
    state: CheckState,
    url: z.string().nullable(),
  })
  export type Check = z.infer<typeof Check>

  export const PullRequest = z.object({
    number: z.number(),
    url: z.string(),
    title: z.string(),
    repo: z.string(),
    branch: z.string(),
    base: z.string(),
    state: z.enum(["open", "closed", "merged"]),
    draft: z.boolean(),
    additions: z.number(),
    deletions: z.number(),
    mergeState: z.string().nullable(),
    checks: Check.array(),
    counts: z.record(CheckState, z.number()),
    autoMerge: z.object({ enabled: z.boolean(), available: z.boolean(), reason: z.string().nullable() }),
  })
  export type PullRequest = z.infer<typeof PullRequest>

  export const PrResult = z.object({ pr: PullRequest.nullable(), reason: z.string().nullable() })
  export type PrResult = z.infer<typeof PrResult>

  /** Files larger than this are listed with counts only. */
  const MAX_CONTENT_BYTES = 512_000
  const MAX_FILES_WITH_CONTENT = 300

  type Run = { code: number; stdout: string; stderr: string }

  async function run(cmd: string[], cwd: string): Promise<Run> {
    try {
      const proc = Bun.spawn(cmd, { cwd, stdout: "pipe", stderr: "pipe", stdin: "ignore", env: process.env })
      const [stdout, stderr, code] = await Promise.all([
        new Response(proc.stdout).text(),
        new Response(proc.stderr).text(),
        proc.exited,
      ])
      return { code, stdout, stderr }
    } catch (error) {
      return { code: -1, stdout: "", stderr: error instanceof Error ? error.message : String(error) }
    }
  }

  const git = (cwd: string, ...args: string[]) => run(["git", ...args], cwd)
  const gh = (cwd: string, ...args: string[]) => run([process.env.GIZZI_GH_BIN || "gh", ...args], cwd)

  /** The repo's default branch, the base a feature branch is compared against. */
  export async function defaultBase(cwd: string): Promise<string | null> {
    const head = await git(cwd, "symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD")
    if (head.code === 0 && head.stdout.trim()) return head.stdout.trim()
    for (const candidate of ["origin/main", "origin/master", "main", "master"]) {
      if ((await git(cwd, "rev-parse", "--verify", "--quiet", `${candidate}^{commit}`)).code === 0) return candidate
    }
    return null
  }

  async function readWorking(cwd: string, file: string): Promise<string | null> {
    const full = path.join(cwd, file)
    const info = await stat(full).catch(() => undefined)
    if (!info?.isFile() || info.size > MAX_CONTENT_BYTES) return null
    return readFile(full, "utf8").catch(() => null)
  }

  async function readAt(cwd: string, rev: string, file: string): Promise<string | null> {
    const size = await git(cwd, "cat-file", "-s", `${rev}:${file}`)
    if (size.code !== 0 || Number(size.stdout.trim()) > MAX_CONTENT_BYTES) return null
    const show = await git(cwd, "show", `${rev}:${file}`)
    return show.code === 0 ? show.stdout : null
  }

  function countLines(text: string): number {
    if (!text) return 0
    return text.split("\n").length - (text.endsWith("\n") ? 1 : 0)
  }

  /**
   * The branch against its base: committed work since the merge base plus
   * uncommitted and untracked files, like the diff a PR would show once pushed.
   */
  export async function changes(cwd: string, opts: { base?: string } = {}): Promise<Changes> {
    if ((await git(cwd, "rev-parse", "--is-inside-work-tree")).code !== 0) {
      return { branch: null, base: null, mergeBase: null, additions: 0, deletions: 0, files: [] }
    }
    const branchRun = await git(cwd, "rev-parse", "--abbrev-ref", "HEAD")
    const branch = branchRun.code === 0 ? branchRun.stdout.trim() : null
    const base = opts.base || (await defaultBase(cwd))
    let mergeBase: string | null = null
    if (base) {
      const mb = await git(cwd, "merge-base", base, "HEAD")
      if (mb.code === 0) mergeBase = mb.stdout.trim()
    }
    const from = mergeBase ?? ((await git(cwd, "rev-parse", "--verify", "--quiet", "HEAD")).code === 0 ? "HEAD" : null)

    const entries = new Map<string, { status: FileChange["status"]; additions: number; deletions: number; binary: boolean }>()
    if (from) {
      const nameStatus = await git(cwd, "diff", "--no-renames", "--name-status", "-z", from)
      const tokens = nameStatus.stdout.split("\0").filter(Boolean)
      for (let i = 0; i + 1 < tokens.length; i += 2) {
        const code = tokens[i][0]
        entries.set(tokens[i + 1], {
          status: code === "A" ? "added" : code === "D" ? "deleted" : "modified",
          additions: 0,
          deletions: 0,
          binary: false,
        })
      }
      const numstat = await git(cwd, "diff", "--no-renames", "--numstat", "-z", from)
      for (const record of numstat.stdout.split("\0").filter(Boolean)) {
        const [adds, dels, ...rest] = record.split("\t")
        const file = rest.join("\t")
        const entry = entries.get(file)
        if (!entry) continue
        if (adds === "-") entry.binary = true
        else {
          entry.additions = Number(adds) || 0
          entry.deletions = Number(dels) || 0
        }
      }
    }
    const untracked = await git(cwd, "ls-files", "--others", "--exclude-standard", "-z")
    for (const file of untracked.stdout.split("\0").filter(Boolean)) {
      if (entries.has(file)) continue
      entries.set(file, { status: "added", additions: 0, deletions: 0, binary: false })
    }

    const files: FileChange[] = []
    let withContent = 0
    for (const [file, entry] of [...entries].sort(([a], [b]) => a.localeCompare(b))) {
      let before: string | null = null
      let after: string | null = null
      if (!entry.binary && withContent < MAX_FILES_WITH_CONTENT) {
        withContent += 1
        before = entry.status === "added" || !from ? "" : await readAt(cwd, from, file)
        after = entry.status === "deleted" ? "" : await readWorking(cwd, file)
        // NUL bytes mean binary content git did not flag (e.g. untracked).
        if ((before ?? "").includes("\0") || (after ?? "").includes("\0")) {
          entry.binary = true
          before = after = null
        }
      }
      if (entry.status === "added" && entry.additions === 0 && after) entry.additions = countLines(after)
      files.push({ file, ...entry, before, after })
    }
    return {
      branch,
      base,
      mergeBase,
      additions: files.reduce((sum, f) => sum + f.additions, 0),
      deletions: files.reduce((sum, f) => sum + f.deletions, 0),
      files,
    }
  }

  // ── Pull request + CI (GitHub CLI) ─────────────────────────────────────────

  const PR_FIELDS = [
    "number",
    "url",
    "title",
    "state",
    "isDraft",
    "headRefName",
    "baseRefName",
    "additions",
    "deletions",
    "mergeStateStatus",
    "statusCheckRollup",
    "autoMergeRequest",
  ].join(",")

  export function checkState(item: Record<string, unknown>): CheckState {
    const upper = (v: unknown) => (typeof v === "string" ? v.toUpperCase() : "")
    if (item.__typename === "StatusContext" || ("state" in item && !("status" in item))) {
      const state = upper(item.state)
      if (state === "SUCCESS") return "passed"
      if (state === "FAILURE" || state === "ERROR") return "failed"
      return state === "EXPECTED" || state === "PENDING" ? "in_progress" : "queued"
    }
    const status = upper(item.status)
    if (status !== "COMPLETED") return status === "IN_PROGRESS" ? "in_progress" : "queued"
    switch (upper(item.conclusion)) {
      case "SUCCESS":
      case "NEUTRAL":
        return "passed"
      case "SKIPPED":
        return "skipped"
      case "CANCELLED":
      case "STALE":
        return "cancelled"
      default:
        return "failed"
    }
  }

  export function parseChecks(rollup: unknown): Check[] {
    if (!Array.isArray(rollup)) return []
    return rollup
      .filter((item): item is Record<string, unknown> => !!item && typeof item === "object")
      .map((item) => ({
        name: String(item.name ?? item.context ?? "check"),
        workflow: typeof item.workflowName === "string" && item.workflowName ? item.workflowName : null,
        state: checkState(item),
        url: (typeof item.detailsUrl === "string" && item.detailsUrl) || (typeof item.targetUrl === "string" && item.targetUrl) || null,
      }))
  }

  export function repoFromUrl(url: string): string {
    const match = /github\.com\/([^/]+\/[^/]+)\/pull\//.exec(url)
    return match ? match[1] : ""
  }

  function ghReason(result: Run): string {
    const text = `${result.stderr}\n${result.stdout}`
    if (result.code === -1 || /not found|ENOENT|No such file/i.test(result.stderr)) return "GitHub CLI (gh) is not installed."
    if (/auth login|not logged|authentication/i.test(text)) return "GitHub CLI is not signed in (run `gh auth login`)."
    return text.trim().split("\n")[0] || "GitHub CLI failed."
  }

  const autoMergeAllowed = new Map<string, { at: number; allowed: boolean | null }>()

  async function repoAllowsAutoMerge(cwd: string, repo: string): Promise<boolean | null> {
    const cached = autoMergeAllowed.get(repo)
    if (cached && Date.now() - cached.at < 5 * 60_000) return cached.allowed
    const result = await gh(cwd, "repo", "view", repo, "--json", "autoMergeAllowed")
    let allowed: boolean | null = null
    if (result.code === 0) {
      try {
        allowed = !!JSON.parse(result.stdout).autoMergeAllowed
      } catch {}
    }
    autoMergeAllowed.set(repo, { at: Date.now(), allowed })
    return allowed
  }

  /** The open (or just merged/closed) pull request for the session's current branch. */
  export async function pullRequest(cwd: string): Promise<PrResult> {
    const view = await gh(cwd, "pr", "view", "--json", PR_FIELDS)
    if (view.code !== 0) {
      if (/no pull requests found|no open pull requests/i.test(`${view.stderr}${view.stdout}`)) return { pr: null, reason: null }
      return { pr: null, reason: ghReason(view) }
    }
    let raw: Record<string, any>
    try {
      raw = JSON.parse(view.stdout)
    } catch {
      return { pr: null, reason: "GitHub CLI returned unreadable output." }
    }
    const checks = parseChecks(raw.statusCheckRollup)
    const counts = Object.fromEntries(CheckState.options.map((state) => [state, 0])) as Record<CheckState, number>
    for (const check of checks) counts[check.state] += 1
    const repo = repoFromUrl(String(raw.url ?? ""))
    const state = String(raw.state ?? "OPEN").toLowerCase()
    const open = state === "open"
    const allowed = open && repo ? await repoAllowsAutoMerge(cwd, repo) : null
    return {
      reason: null,
      pr: {
        number: Number(raw.number),
        url: String(raw.url ?? ""),
        title: String(raw.title ?? ""),
        repo,
        branch: String(raw.headRefName ?? ""),
        base: String(raw.baseRefName ?? ""),
        state: state === "merged" ? "merged" : state === "closed" ? "closed" : "open",
        draft: !!raw.isDraft,
        additions: Number(raw.additions) || 0,
        deletions: Number(raw.deletions) || 0,
        mergeState: typeof raw.mergeStateStatus === "string" ? raw.mergeStateStatus : null,
        checks,
        counts,
        autoMerge: {
          enabled: !!raw.autoMergeRequest,
          available: open && allowed === true,
          reason: !open
            ? "The pull request is not open."
            : allowed === false
              ? "Auto-merge isn't enabled for this repository. Use GitHub directly."
              : allowed === null
                ? "Couldn't check whether this repository allows auto-merge."
                : null,
        },
      },
    }
  }

  export async function setAutoMerge(cwd: string, number: number, enabled: boolean): Promise<{ ok: boolean; reason: string | null }> {
    const args = enabled ? ["pr", "merge", String(number), "--auto", "--merge"] : ["pr", "merge", String(number), "--disable-auto"]
    const result = await gh(cwd, ...args)
    return result.code === 0 ? { ok: true, reason: null } : { ok: false, reason: ghReason(result) }
  }
}
