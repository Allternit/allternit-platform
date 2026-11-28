/**
 * `gizzi inbox` — what is waiting on you, and your decision on it.
 *
 *   gizzi inbox [list] [--json]
 *   gizzi inbox approve <id>
 *   gizzi inbox deny <id>
 *
 * approve and deny are for a person at their own terminal. The model's tool has
 * no such action and the guard refuses `gizzi inbox approve|deny` from its shell
 * (AllternitOS CONTRACT 4.3, 7.2). Same client as the TUI's `/inbox`.
 */
import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { inboxLine, listInbox, resolveInbox } from "@/runtime/services/api/inbox"

async function list(json: boolean): Promise<number> {
  try {
    const items = await listInbox("needs_you")
    if (json) console.log(JSON.stringify(items, null, 2))
    else if (!items.length) console.log("Nothing needs you.")
    else for (const i of items) console.log(inboxLine(i))
    return 0
  } catch (e) {
    console.error(e instanceof Error ? e.message : String(e))
    return 1
  }
}

async function decide(id: string, decision: "approve" | "deny"): Promise<number> {
  try {
    await resolveInbox(id, decision)
    console.log(`${decision === "approve" ? "Approved" : "Denied"} ${id}.`)
    return 0
  } catch (e) {
    console.error(e instanceof Error ? e.message : String(e))
    return 1
  }
}

const idArg = (y: Argv) => y.positional("id", { type: "string", demandOption: true, describe: "the item id from `gizzi inbox`" })

export const InboxCommand = cmd({
  command: "inbox",
  describe: "See what needs you, and approve or deny it yourself",
  builder: (yargs: Argv) =>
    yargs
      .command({
        command: ["list", "$0"],
        describe: "items waiting on you",
        builder: (y: Argv) => y.option("json", { type: "boolean", default: false, describe: "one JSON document on stdout" }),
        handler: async (argv: any) => {
          process.exitCode = await list(!!argv.json)
        },
      })
      .command({
        command: "approve <id>",
        describe: "approve one item (a person's decision; the model cannot)",
        builder: idArg,
        handler: async (argv: any) => {
          process.exitCode = await decide(String(argv.id), "approve")
        },
      })
      .command({
        command: "deny <id>",
        describe: "deny one item (a person's decision; the model cannot)",
        builder: idArg,
        handler: async (argv: any) => {
          process.exitCode = await decide(String(argv.id), "deny")
        },
      }),
  handler: () => {},
})
