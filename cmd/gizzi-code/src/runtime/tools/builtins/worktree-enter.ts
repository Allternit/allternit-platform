/**
 * worktree_enter (G5, PR1). Creates a git worktree (or reuses the one with that
 * name) and moves the session's working folder into it. Shapes in
 * runtime/primitives/gap-types.ts; the git work and the registry of who
 * created which tree live in runtime/primitives/worktree.ts.
 *
 * Gated: the registry, the CLI bridge and the tool families list this tool
 * only while `available` is true.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { WORKTREE_ENTER_TOOL_ID, WorktreeEnterInput, type WorktreeEnterMetadata } from "@/runtime/primitives/gap-types"
import { SessionWorktree } from "@/runtime/primitives/worktree"
import { Instance } from "@/runtime/context/project/instance"
import { Session } from "@/runtime/session"
import DESCRIPTION from "@/runtime/tools/builtins/worktree-enter.txt"

export const available: boolean = true

export const WorktreeEnterTool = Tool.define<typeof WorktreeEnterInput, WorktreeEnterMetadata>(WORKTREE_ENTER_TOOL_ID, {
  description: DESCRIPTION,
  parameters: WorktreeEnterInput,
  async execute(params, ctx) {
    const current = await SessionWorktree.active(ctx.sessionID)
    if (current) {
      return {
        title: "Already in a worktree",
        output: `This session already works in ${current.path}. Call worktree_exit before entering another worktree.`,
        metadata: { ok: false, path: current.path, branch: current.branch },
      }
    }

    const original = Instance.workdir
    let tree: SessionWorktree.CreateResult
    try {
      tree = await SessionWorktree.create({ cwd: original, sessionID: ctx.sessionID, name: params.name, base: params.base })
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      return { title: "Could not create the worktree", output: message, metadata: { ok: false } }
    }

    await SessionWorktree.enter(ctx.sessionID, tree.path, original, tree.branch)
    Instance.moveWorkdir(tree.path)
    await Session.worktreeChanged(ctx.sessionID).catch(() => undefined)

    const lines = [
      `${tree.existed ? "Reusing" : "Created"} worktree ${tree.path}`,
      `Branch: ${tree.branch} (from ${tree.base.slice(0, 12)})`,
      `The session now works in this folder until worktree_exit. You came from ${original}.`,
    ]
    if (tree.existed && tree.created_by !== ctx.sessionID) {
      lines.push("Another session or tool made this tree, so this session will keep it on exit, never remove it.")
    }
    return {
      title: tree.branch,
      output: lines.join("\n"),
      metadata: { ok: true, path: tree.path, branch: tree.branch, base: tree.base, created_by: tree.created_by },
    }
  },
})
