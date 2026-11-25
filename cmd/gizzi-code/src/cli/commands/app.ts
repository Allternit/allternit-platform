/**
 * `gizzi app …`: apps made from artifacts (Apps + MCP 2026).
 *
 *   gizzi app list
 *   gizzi app open <slug>
 *   gizzi app install <slug> [--level browser|app|full]
 *   gizzi app export <slug> --mcp on|off
 */
import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import {
  AppsError,
  LEVELS,
  appLink,
  cloud,
  consentLines,
  fail,
  findApp,
  formatApps,
  shelf,
  validSlug,
  type Level,
} from "@/cli/apps/client"

const slugArg = (y: Argv) => y.positional("slug", { type: "string", demandOption: true, describe: "the app's slug" })

const List = cmd({
  command: "list",
  describe: "list your apps",
  builder: (y: Argv) => y.option("json", { type: "boolean", describe: "print JSON" }),
  handler: async (args: { json?: boolean }) => {
    try {
      const apps = await shelf()
      process.stdout.write((args.json ? JSON.stringify(apps, null, 2) : formatApps(apps)) + "\n")
    } catch (e) {
      fail(e)
    }
  },
})

const Open = cmd({
  command: "open <slug>",
  describe: "open an app in the browser",
  builder: (y: Argv) => slugArg(y),
  handler: async (args: { slug: string }) => {
    if (!validSlug(args.slug)) fail(new Error(`"${args.slug}" is not an app slug (lowercase letters and digits, letter first).`))
    const url = appLink(args.slug)
    process.stdout.write(url + "\n")
    try {
      const { default: open } = await import("open")
      await open(url)
    } catch {}
  },
})

const Install = cmd({
  command: "install <slug>",
  describe: "install an app",
  builder: (y: Argv) =>
    slugArg(y)
      .option("level", { type: "string", choices: [...LEVELS], default: "browser", describe: "permission level to grant" })
      .option("yes", { type: "boolean", describe: "allow what the app asks for" }),
  handler: async (args: { slug: string; level?: string; yes?: boolean }) => {
    try {
      const app = await findApp(args.slug)
      const level = (args.level ?? "browser") as Level
      const base = `/api/v1/apps/${app.kind}/${encodeURIComponent(app.ref)}/install`
      const first = await cloud<{ install: unknown; needs_consent: boolean; requested_tier?: string; added_capabilities?: string[] }>(
        "POST",
        base,
        { granted_tier: level },
      )
      if (first.needs_consent) {
        // Never auto-consent: print what the app needs, install only with --yes.
        process.stdout.write(consentLines(first).join("\n") + "\n")
        if (!args.yes) {
          process.stdout.write(`Run again with --yes to allow it: gizzi app install ${args.slug} --level ${level} --yes\n`)
          process.exit(2)
        }
        await cloud("POST", base, { granted_tier: level, consent: true })
      }
      process.stdout.write(`Installed ${app.name} at the ${level} level.\n`)
    } catch (e) {
      if (e instanceof AppsError && e.code === "tier_not_allowed") fail(new Error(`${e.message} Try a lower --level.`))
      fail(e)
    }
  },
})

const Export = cmd({
  command: "export <slug>",
  describe: "turn the app's MCP export on or off",
  builder: (y: Argv) => slugArg(y).option("mcp", { type: "string", choices: ["on", "off"], demandOption: true }),
  handler: async (args: { slug: string; mcp: string }) => {
    try {
      const app = await findApp(args.slug)
      if (app.kind !== "artifact") fail(new Error("Only your own artifact apps can be exported."))
      const cur = (await cloud<{ app: Record<string, any> }>("GET", `/api/v2/artifacts/${encodeURIComponent(app.ref)}/app`)).app
      const snapshot = { ...(cur.snapshot ?? cur.capability_snapshot ?? {}), export_mcp: args.mcp === "on" }
      await cloud("PUT", `/api/v2/artifacts/${encodeURIComponent(app.ref)}/app`, {
        name: cur.name ?? app.name,
        ...(cur.icon ? { icon: cur.icon } : {}),
        tier: cur.tier ?? cur.permission_tier ?? app.tier,
        ...(cur.install_scope ? { install_scope: cur.install_scope } : {}),
        ...(cur.tools ? { tools: cur.tools } : {}),
        ...(cur.skill_ref ? { skill_ref: cur.skill_ref } : {}),
        snapshot,
      })
      process.stdout.write(`MCP export is ${args.mcp} for ${app.name}.\n`)
    } catch (e) {
      fail(e)
    }
  },
})

export const AppCommand = cmd({
  command: "app",
  describe: "apps made from artifacts: list, open, install, export",
  builder: (y: Argv) => y.command(List).command(Open).command(Install).command(Export).demandCommand(1),
  handler: () => {},
})
