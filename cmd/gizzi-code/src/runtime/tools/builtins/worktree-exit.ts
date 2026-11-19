/**
 * worktree_exit (G5, PR1). Returns the session to the folder it worked in
 * before worktree_enter, and keeps or removes the tree. A remove only goes
 * through for a tree this session created that has no uncommitted or unpushed
 * work; otherwise the tree is kept and the result says why. This tool never
 * discards work. Core: runtime/primitives/worktree.ts.
 *
 * Gated: the registry, the CLI bridge and the tool families list this tool
 * only while `available` is true.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { WORKTREE_EXIT_TOOL_ID, WorktreeExitInput, type WorktreeExitMetadata } from "@/runtime/primitives/gap-types"
import { SessionWorktree } from "@/runtime/primitives/worktree"
import { Instance } from "@/runtime/context/project/instance"
import { Session } from "@/runtime/session"
import DESCRIPTION from "@/runtime/tools/builtins/worktree-exit.txt"

export const available: boolean = true

function plural(n: number, word: string) {
  return `${n} ${word}${n === 1 ? "" : "s"}`
}

export const WorktreeExitTool = Tool.define<typeof WorktreeExitInput, WorktreeExitMetadata>(WORKTREE_EXIT_TOOL_ID, {
  description: DESCRIPTION,
  parameters: WorktreeExitInput,
  async execute(params, ctx) {
    const current = await SessionWorktree.leave(ctx.sessionID)
    if (!current) {
      return {
        title: "Not in a worktree",
        output: "This session is not working in a worktree, so there is nothing to leave.",
        metadata: { ok: false, action: params.action, result: "refused", reason: "not_in_worktree" },
      }
    }

    // Back to the original folder first, whatever happens to the tree.
    Instance.moveWorkdir(current.original)
    await Session.worktreeChanged(ctx.sessionID).catch(() => undefined)
    const back = `The session is back in ${current.original}.`
    const branch = current.branch ?? (await SessionWorktree.owner(current.path))?.branch

    if (params.action === "keep") {
      return {
        title: `Kept ${branch ?? "worktree"}`,
        output: `Kept the worktree at ${current.path}${branch ? ` on branch ${branch}` : ""}. ${back}`,
        metadata: { ok: true, action: "keep", result: "kept", path: current.path, branch },
      }
    }

    const removed = await SessionWorktree.remove({ path: current.path, sessionID: ctx.sessionID })
    if (removed.removed) {
      return {
        title: `Removed ${removed.branch ?? "worktree"}`,
        output: `Removed the worktree at ${removed.path}${removed.branch ? ` and its branch ${removed.branch}` : ""}. ${back}`,
        metadata: { ok: true, action: "remove", result: "removed", path: removed.path, branch: removed.branch ?? branch },
      }
    }

    const s = removed.status
    const why =
      removed.reason === "not_created_here"
        ? "this session did not create it"
        : removed.reason === "unpushed_work"
          ? `it has ${plural(s?.unpushed ?? 0, "unpushed commit")}`
          : removed.reason === "uncommitted_changes"
            ? s?.error
              ? `its state could not be read (${s.error})`
              : `it has ${plural(s?.uncommitted ?? 0, "uncommitted change")}`
            : `git could not remove it (${removed.detail ?? removed.reason ?? "unknown error"})`
    const reason: WorktreeExitMetadata["reason"] =
      removed.reason === "not_created_here" || removed.reason === "unpushed_work" || removed.reason === "uncommitted_changes"
        ? removed.reason
        : undefined
    const gone = removed.reason === "not_a_worktree"
    return {
      title: gone ? "Worktree already gone" : `Kept ${branch ?? "worktree"}`,
      output: gone
        ? `The worktree folder ${removed.path} no longer exists. ${back}`
        : `Did not remove ${removed.path} because ${why}; it is kept${branch ? ` on branch ${branch}` : ""}. ${back}`,
      metadata: {
        ok: true,
        action: "remove",
        result: gone ? "removed" : removed.reason === "not_created_here" ? "refused" : "kept",
        path: removed.path,
        branch,
        ...(reason ? { reason } : {}),
      },
    }
  },
})
