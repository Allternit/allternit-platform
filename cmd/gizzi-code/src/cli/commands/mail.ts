/**
 * `gizzi orchestration mail` — the local half.
 *
 * `mail list|read|send|decide` forward to the Allternit Factory engine
 * (SPEC §7; the engine owns the mail that the old `gizzi mail` read over
 * HTTP). What stays local is the agent-email (mailflare) rail:
 * `mail send-external` and `mail email-status`.
 */

import type { Argv } from "yargs"
import { getAgentEmailStatus, sendAgentEmail } from "@/cli/agent-email-client"

/** Mount the local mail verbs on a yargs builder. */
export function withLocalMailCommands(yargs: Argv): Argv {
  return yargs
      .command(
        "send-external",
        "Send an external email from a provisioned agent (approval-gated; review it with `gizzi orchestration mail decide <threadId>`)",
        (yargs) =>
          yargs
            .option("agent-id", {
              type: "string",
              describe: "Agent id with a provisioned mailflare email channel",
              demandOption: true,
            })
            .option("to", {
              type: "string",
              describe: "Recipient email address",
              demandOption: true,
            })
            .option("subject", {
              type: "string",
              describe: "Subject line",
              demandOption: true,
            })
            .option("text", {
              type: "string",
              describe: "Plain-text body",
            })
            .option("html", {
              type: "string",
              describe: "HTML body",
            }),
        async (argv) => {
          try {
            if (argv.text === undefined && argv.html === undefined) {
              throw new Error("one of --text or --html is required")
            }
            const result = await sendAgentEmail({
              agentId: argv.agentId as string,
              to: argv.to as string,
              subject: argv.subject as string,
              text: argv.text as string | undefined,
              html: argv.html as string | undefined,
            })
            if (result.status === "pending_approval") {
              console.log(`✉️  Email queued for approval — review thread: ${result.thread}`)
              console.log(`   Approve with: gizzi orchestration mail decide "${result.thread}" --approve`)
            } else {
              console.log(`✅ Email sent (message id: ${result.messageId ?? result.id})`)
            }
            process.exit(0)
          } catch (err) {
            console.error(`❌ ${(err as Error).message}`)
            process.exit(1)
          }
        },
      )
      .command(
        "email-status",
        "Show the agent-email (mailflare) rail status",
        () => {},
        async () => {
          try {
            const status = await getAgentEmailStatus()
            if (!status.configured) {
              console.log("✉️  Agent email rail: not configured (mailflare env unset; provisioning falls back to CommRails mint)")
            } else {
              console.log(`✉️  Agent email rail: configured — domain ${status.domain ?? "?"} — ${status.reachable ? "reachable" : "UNREACHABLE"}`)
            }
            process.exit(0)
          } catch (err) {
            console.error(`❌ ${(err as Error).message}`)
            process.exit(1)
          }
        },
      )
}
