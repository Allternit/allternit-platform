/**
 * `gizzi team` — your organization's roster of live sessions (agent network WP13).
 *
 *   gizzi team list                       people → machines → sessions and bots
 *   gizzi team send <address> <message>   message a teammate's session (person/session)
 *
 * Backed by allternit-cloud-api GET /api/v1/team/roster and POST /api/v1/team/send.
 */
import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { EOL } from "os"
import { formatRoster, type Roster } from "@/cli/commands/team-format"
import { Flag } from "@/runtime/context/flag/flag"

const API_BASE = Flag.GIZZI_PLATFORM_API_URL.replace(/\/+$/, "")

async function token(): Promise<string | undefined> {
  if (process.env.ALLTERNIT_API_TOKEN) return process.env.ALLTERNIT_API_TOKEN
  try {
    const fs = await import("fs")
    const path = await import("path")
    const os = await import("os")
    const dir = process.env.GIZZI_CONFIG_DIR ?? path.join(os.homedir(), ".config", "gizzi-code")
    return JSON.parse(fs.readFileSync(path.join(dir, "session.json"), "utf8"))?.accessToken || undefined
  } catch {
    return undefined
  }
}

async function call<T>(method: "GET" | "POST", path: string, body?: unknown): Promise<T> {
  const t = await token()
  if (!t) throw new Error("Not signed in. Run `gizzi auth login` first.")
  const res = await fetch(`${API_BASE}${path}`, {
    method,
    headers: { "Content-Type": "application/json", Accept: "application/json", Authorization: `Bearer ${t}` },
    body: body ? JSON.stringify(body) : undefined,
  })
  if (!res.ok) {
    const text = await res.text().catch(() => "")
    throw new Error(`HTTP ${res.status}${text ? `: ${text}` : ""}`)
  }
  return (await res.json()) as T
}

const TeamListCommand = cmd({
  command: "list",
  describe: "teammates' live sessions: people, machines, sessions and bots",
  builder: (y: Argv) => y.option("json", { type: "boolean", describe: "print raw JSON" }),
  async handler(args) {
    const r = await call<Roster>("GET", "/api/v1/team/roster")
    UI.println(args.json ? JSON.stringify(r, null, 2) : formatRoster(r).join(EOL))
  },
})

const TeamSendCommand = cmd({
  command: "send <address> <message..>",
  describe: "message a teammate's session or bot (person/session)",
  builder: (y: Argv) =>
    y
      .positional("address", { type: "string", demandOption: true })
      .positional("message", { type: "string", array: true, demandOption: true })
      .option("reply-to", { type: "string", describe: "hop id this replies to" }),
  async handler(args) {
    const r = await call<{ hopId: string | null; state: string; tier: string; note: string }>("POST", "/api/v1/team/send", {
      to: String(args.address),
      message: (args.message as string[]).join(" "),
      replyTo: args["reply-to"] as string | undefined,
    })
    UI.println(`${r.state} (${r.tier}): ${r.note}${r.hopId ? `  [hop ${r.hopId}]` : ""}`)
  },
})

export const TeamCommand = cmd({
  command: "team",
  describe: "your organization's roster: teammates' live sessions",
  builder: (y: Argv) => y.command(TeamListCommand).command(TeamSendCommand).demandCommand(),
  async handler() {},
})
