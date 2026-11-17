/**
 * send_user_message: a message to the session's own user, with files and an
 * optional push (runtime/primitives/user-message.ts). Every client gets it
 * (not bot-gated): Gizzi Code, plain chats, workers and bot chats alike. The
 * Ink adapter wraps the same core and shares send-user-message.txt.
 *
 * The result is the message: clients render `metadata` ({text, files, push})
 * as a message card; the model reads one short line back.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { Instance } from "@/runtime/context/project/instance"
import { Scratchpad } from "@/runtime/session/scratchpad"
import { UserMessage } from "@/runtime/primitives/user-message"
import DESCRIPTION from "@/runtime/tools/builtins/send-user-message.txt"

/**
 * Where a file may come from: the folders Instance.containsPath treats as the
 * project (the instance directory, the session's working folder, the git
 * worktree unless it is "/"), plus this session's scratchpad.
 */
export async function allowedRoots(sessionID: string): Promise<string[]> {
  const roots = [Instance.directory, Instance.workdir]
  if (Instance.worktree && Instance.worktree !== "/") roots.push(Instance.worktree)
  const scratch = await Scratchpad.scope(sessionID).catch(() => undefined)
  if (scratch) roots.push(scratch.privateDirectory, scratch.sharedDirectory)
  return [...new Set(roots)]
}

export const SendUserMessageTool = Tool.define("send_user_message", {
  description: DESCRIPTION,
  parameters: UserMessage.parameters,
  async execute(params, ctx) {
    const notify = params.notify ?? "none"
    await ctx.ask({
      permission: "send_user_message",
      patterns: ["*"],
      always: ["*"],
      metadata: { notify, files: (params.attachments ?? []).map((a) => a.path) },
    })
    const result = await UserMessage.sendUserMessage({
      sessionID: ctx.sessionID,
      text: params.text,
      attachments: params.attachments,
      notify,
      ctx: { roots: await allowedRoots(ctx.sessionID), cwd: Instance.workdir, ref: ctx.messageID },
    })
    const metadata: UserMessage.Metadata = result
    return {
      title: UserMessage.title(result.text),
      output: UserMessage.summary(result, notify),
      metadata,
    }
  },
})
