import * as prompts from "@clack/prompts"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { runKnowledge } from "@/runtime/knowledge/client"

/**
 * `gizzi knowledge list|show <id>|approve <id>|reject <id>|dismiss <id>|activate <id>`:
 * review what the knowledge curator proposed, from a terminal. Approving a
 * skill proposal, or `activate`, turns it on after the skill_manage ask: a
 * yes/no prompt in a terminal, `--yes` in a script.
 */
export const KnowledgeCommand = cmd({
  command: "knowledge [action] [id]",
  describe: "review knowledge flags and proposals (list, show, approve, reject, dismiss, activate)",
  builder: (yargs) =>
    yargs
      .positional("action", {
        type: "string",
        choices: ["list", "show", "approve", "reject", "dismiss", "activate"],
        default: "list",
      })
      .positional("id", { type: "string" })
      .option("yes", { type: "boolean", default: false, describe: "answer yes to the turn-on question (for scripts)" }),
  handler: async (args) => {
    const confirm = args.yes
      ? async () => true
      : process.stdin.isTTY
        ? async (question: string) => {
            const answer = await prompts.confirm({ message: question, initialValue: false })
            return answer === true && !prompts.isCancel(answer)
          }
        : undefined
    UI.println(await runKnowledge([String(args.action), ...(args.id ? [String(args.id)] : [])], { confirm }))
  },
})
