/**
 * pr_comments: lists and replies to pull request comments through the gh CLI.
 * Shapes in runtime/primitives/gap-types.ts. Comment text is written by other
 * people, so the model sees it fenced as untrusted data. Every reply asks first.
 */
import { existsSync } from "fs"
import { Tool } from "@/runtime/tools/builtins/tool"
import {
  PR_COMMENTS_TOOL_ID,
  PrCommentsInput,
  type PrComment,
  type PrCommentsMetadata,
} from "@/runtime/primitives/gap-types"
import { fenceUntrusted } from "@/runtime/tools/guard/untrusted"
import { Instance } from "@/runtime/context/project/instance"
import DESCRIPTION from "@/runtime/tools/builtins/pr-comments.txt"

export const available: boolean = true

const GH_CANDIDATES = ["gh", "/opt/homebrew/bin/gh", "/usr/local/bin/gh", "/usr/bin/gh"]

function ghPath(): string | null {
  for (const candidate of GH_CANDIDATES) {
    const found = candidate.includes("/") ? (existsSync(candidate) ? candidate : null) : Bun.which(candidate)
    if (found) return found
  }
  return null
}

async function gh(args: string[]): Promise<{ ok: boolean; out: string }> {
  const bin = ghPath()
  if (!bin) return { ok: false, out: "The gh CLI is not installed." }
  try {
    const proc = Bun.spawn([bin, ...args], { cwd: Instance.directory, stdout: "pipe", stderr: "pipe" })
    const [out, err] = await Promise.all([new Response(proc.stdout).text(), new Response(proc.stderr).text()])
    const code = await proc.exited
    return code === 0 ? { ok: true, out } : { ok: false, out: (err || out).trim().slice(0, 500) }
  } catch (e) {
    return { ok: false, out: e instanceof Error ? e.message : String(e) }
  }
}

function fail(action: "list" | "reply", message: string) {
  return {
    title: `pr_comments ${action} failed`,
    output: message,
    metadata: { ok: false, action } satisfies PrCommentsMetadata,
  }
}

async function resolvePr(pr: string | number | undefined) {
  const args = ["pr", "view", ...(pr === undefined ? [] : [String(pr)]), "--json", "number,url"]
  const res = await gh(args)
  if (!res.ok) return { error: res.out || "Could not find the pull request." }
  try {
    const json = JSON.parse(res.out)
    const m = /github\.com\/([^/]+)\/([^/]+)\/pull\//.exec(String(json.url))
    if (!m) return { error: "Could not read the repository from the pull request URL." }
    return { number: Number(json.number), url: String(json.url), owner: m[1], repo: m[2] }
  } catch {
    return { error: "Could not read the pull request." }
  }
}

async function resolvedIds(owner: string, repo: string, number: number): Promise<Set<string>> {
  const query = `query($o:String!,$r:String!,$n:Int!){repository(owner:$o,name:$r){pullRequest(number:$n){reviewThreads(first:100){nodes{isResolved comments(first:100){nodes{databaseId}}}}}}}`
  const res = await gh(["api", "graphql", "-f", `query=${query}`, "-F", `o=${owner}`, "-F", `r=${repo}`, "-F", `n=${number}`])
  const ids = new Set<string>()
  if (!res.ok) return ids
  try {
    const nodes = JSON.parse(res.out)?.data?.repository?.pullRequest?.reviewThreads?.nodes ?? []
    for (const t of nodes) {
      if (!t.isResolved) continue
      for (const c of t.comments?.nodes ?? []) ids.add(String(c.databaseId))
    }
  } catch {}
  return ids
}

function render(c: PrComment): string {
  const where = c.path ? ` on ${c.path}${c.line ? `:${c.line}` : ""}` : ""
  const state = c.resolved ? " [resolved]" : ""
  return `#${c.id} @${c.author}${where}${state}\n${c.body}`
}

export const PrCommentsTool = Tool.define<typeof PrCommentsInput, PrCommentsMetadata>(PR_COMMENTS_TOOL_ID, {
  description: DESCRIPTION,
  parameters: PrCommentsInput,
  async execute(params, ctx) {
    const target = await resolvePr(params.pr)
    if ("error" in target) return fail(params.action, target.error ?? "Could not find the pull request.")
    const { owner, repo, number, url } = target
    const base = `repos/${owner}/${repo}`

    if (params.action === "list") {
      const [review, issue] = await Promise.all([
        gh(["api", "--paginate", `${base}/pulls/${number}/comments`]),
        gh(["api", "--paginate", `${base}/issues/${number}/comments`]),
      ])
      if (!review.ok && !issue.ok) return fail("list", review.out)
      const resolved = await resolvedIds(owner, repo, number)
      const comments: PrComment[] = []
      const parse = (text: string): any[] => {
        try {
          // --paginate can emit several JSON arrays back to back
          return text.trim().split(/\]\s*\[/).flatMap((chunk, i, all) => {
            const s = (i > 0 ? "[" : "") + chunk + (i < all.length - 1 ? "]" : "")
            return JSON.parse(s)
          })
        } catch {
          return []
        }
      }
      if (review.ok)
        for (const c of parse(review.out))
          comments.push({
            id: String(c.id),
            author: String(c.user?.login ?? "unknown"),
            body: String(c.body ?? ""),
            path: c.path ?? undefined,
            line: typeof (c.line ?? c.original_line) === "number" ? (c.line ?? c.original_line) : undefined,
            resolved: resolved.has(String(c.id)),
            url: c.html_url,
          })
      if (issue.ok)
        for (const c of parse(issue.out))
          comments.push({
            id: String(c.id),
            author: String(c.user?.login ?? "unknown"),
            body: String(c.body ?? ""),
            url: c.html_url,
          })
      const shown = params.unresolved_only ? comments.filter((c) => !c.resolved) : comments
      const text = shown.length ? shown.map(render).join("\n\n---\n\n") : "No comments."
      return {
        title: `${shown.length} comment${shown.length === 1 ? "" : "s"} on PR #${number}`,
        output: shown.length ? fenceUntrusted(text, "connector") : text,
        metadata: { ok: true, action: "list", pr: number, url, comments: shown },
      }
    }

    if (!params.comment_id || !params.body) return fail("reply", "Reply needs comment_id and body.")
    await ctx.ask({
      permission: "pr_comments",
      patterns: [`${owner}/${repo}#${number}`],
      always: [],
      metadata: { pr: number, url, comment_id: params.comment_id, body: params.body },
    })
    let res = await gh(["api", "-X", "POST", `${base}/pulls/${number}/comments/${params.comment_id}/replies`, "-f", `body=${params.body}`])
    if (!res.ok) {
      // Not a review comment: answer in the PR conversation instead.
      res = await gh(["api", "-X", "POST", `${base}/issues/${number}/comments`, "-f", `body=${params.body}`])
    }
    if (!res.ok) return fail("reply", res.out)
    let replyId: string | undefined
    let replyUrl: string | undefined
    try {
      const json = JSON.parse(res.out)
      replyId = json.id !== undefined ? String(json.id) : undefined
      replyUrl = json.html_url
    } catch {}
    return {
      title: `Replied on PR #${number}`,
      output: `Posted the reply${replyUrl ? `: ${replyUrl}` : "."}`,
      metadata: { ok: true, action: "reply", pr: number, url: replyUrl ?? url, reply_id: replyId },
    }
  },
})
