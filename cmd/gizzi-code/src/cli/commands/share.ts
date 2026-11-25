/**
 * `gizzi share …`: share links (Apps + MCP 2026).
 *
 *   gizzi share list
 *   gizzi share stop <id>      revoke a link
 *   gizzi share pause <id>     pause a link (`--resume` turns it back on)
 *   gizzi share <app-slug>     a hosted link for an app
 *   gizzi share <port>         a live link to a local port: prints the URL + token, Ctrl-C stops it
 */
import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { classifyTarget } from "@/cli/apps/share-target"
import { shareLive } from "@/cli/apps/live-share"
import { cloud, fail, findApp, formatLinks, listLinks, pauseLink, revokeLink, slugOf, type ShareLink } from "@/cli/apps/client"

const ListCmd = cmd({
  command: "list",
  describe: "list your share links",
  builder: (y: Argv) => y.option("all", { type: "boolean", describe: "include revoked links" }).option("json", { type: "boolean" }),
  handler: async (args: { all?: boolean; json?: boolean }) => {
    try {
      const links = await listLinks(!!args.all)
      process.stdout.write((args.json ? JSON.stringify(links, null, 2) : formatLinks(links)) + "\n")
    } catch (e) {
      fail(e)
    }
  },
})

const StopCmd = cmd({
  command: "stop <id>",
  describe: "revoke a share link (a running live link stops on its next heartbeat)",
  builder: (y: Argv) => y.positional("id", { type: "string", demandOption: true }),
  handler: async (args: { id: string }) => {
    try {
      await revokeLink(args.id)
      process.stdout.write(`Stopped ${args.id}.\n`)
    } catch (e) {
      fail(e)
    }
  },
})

const PauseCmd = cmd({
  command: "pause <id>",
  describe: "pause a share link (a live link stops serving until it is started again)",
  builder: (y: Argv) =>
    y.positional("id", { type: "string", demandOption: true }).option("resume", { type: "boolean", describe: "turn the link back on" }),
  handler: async (args: { id: string; resume?: boolean }) => {
    try {
      await pauseLink(args.id, !args.resume)
      process.stdout.write(`${args.resume ? "Resumed" : "Paused"} ${args.id}.\n`)
    } catch (e) {
      fail(e)
    }
  },
})

export const ShareCommand = cmd({
  command: "share [target]",
  describe: "share an app (hosted link) or list and manage share links",
  builder: (y: Argv) =>
    y
      .positional("target", { type: "string", describe: "app slug, or a local port for a live link" })
      .command(ListCmd)
      .command(StopCmd)
      .command(PauseCmd),
  handler: async (args: { target?: string }) => {
    if (!args.target) return fail(new Error("Say what to share: gizzi share <app-slug>, or see `gizzi share list`."))
    const t = classifyTarget(args.target)
    if (t.kind === "port") {
      return shareLive(t.port)
    }
    if (t.kind === "invalid") return fail(new Error(`"${args.target}" is neither an app slug nor a port.`))
    try {
      const app = await findApp(t.slug)
      const r = await cloud<{ link: ShareLink }>("POST", "/api/v1/share-links", {
        kind: "hosted",
        subject: "artifact",
        artifact_id: app.ref,
        app_kind: "artifact",
        subdomain: slugOf(app) ?? t.slug,
        label: app.name,
      })
      process.stdout.write(`${r.link?.url ?? "Link created."}\n`)
    } catch (e) {
      fail(e)
    }
  },
})
