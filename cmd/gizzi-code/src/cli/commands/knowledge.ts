import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { runKnowledge } from "@/runtime/knowledge/client"

/**
 * `gizzi knowledge list|approve <id>|reject <id>|dismiss <id>` (and `show <id>`):
 * review what the knowledge curator proposed, from a terminal.
 */
export const KnowledgeCommand = cmd({
  command: "knowledge [action] [id]",
  describe: "review knowledge flags and proposals (list, show, approve, reject, dismiss)",
  builder: (yargs) =>
    yargs
      .positional("action", { type: "string", choices: ["list", "show", "approve", "reject", "dismiss"], default: "list" })
      .positional("id", { type: "string" }),
  handler: async (args) => {
    UI.println(await runKnowledge([String(args.action), ...(args.id ? [String(args.id)] : [])]))
  },
})
