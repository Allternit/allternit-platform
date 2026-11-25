/**
 * `gizzi team` — your organization's roster of live sessions (agent network WP13).
 *
 *   gizzi team list                       people → machines → sessions and bots
 *   gizzi team send <address> <message>   message a teammate's session (person/session)
 *   gizzi team map [--json]               people → bots → sub-bots, with asks and waiting jobs
 *   gizzi team asks [--approve <id> | --decline <id>]
 *   gizzi team rules [list|add|rm]        approval rules (add: --app --tool --field --op --value --approver)
 *   gizzi team grant <bot> <app> <browser|app|full>
 *
 * list/send: allternit-cloud-api /api/v1/team/roster and /api/v1/team/send.
 * map/asks/rules/grant: runtime api (local allternit-api, else the cloud relay), see runtime-client.ts.
 */
import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { EOL } from "os"
import { formatRoster, formatTeamMap, formatAsks, formatRules, teamErrorMessage, type Roster, type TeamMap, type AskRow, type RuleRow } from "@/cli/commands/team-format"
import { Flag } from "@/runtime/context/flag/flag"
import { runtimeJson, setComputer } from "@/cli/commands/runtime-client"

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

async function call<T>(method: "GET" | "POST" | "PUT" | "DELETE", path: string, body?: unknown): Promise<T> {
  const t = await token()
  if (!t) throw new Error("Not signed in. Run `gizzi auth login` first.")
  let res: Response
  try {
    res = await fetch(`${API_BASE}${path}`, {
      method,
      headers: { "Content-Type": "application/json", Accept: "application/json", Authorization: `Bearer ${t}` },
      body: body ? JSON.stringify(body) : undefined,
    })
  } catch {
    throw new Error(teamErrorMessage(0, "no_runtime", ""))
  }
  if (!res.ok) {
    const text = await res.text().catch(() => "")
    let code: string | undefined
    let msg = ""
    try {
      const j = JSON.parse(text)
      const e = j?.error
      code = j?.code ?? (typeof e === "string" ? e : e?.data?.code ?? e?.code)
      msg = j?.message ?? (typeof e === "string" && e !== code ? e : e?.message ?? "")
    } catch {}
    throw new Error(teamErrorMessage(res.status, code, msg || `HTTP ${res.status}${text ? `: ${text}` : ""}`))
  }
  return (await res.json()) as T
}

/** Team routes live on the runtime api (local, else cloud relay), not cloud-api. */
const rt = <T>(method: "GET" | "POST" | "PUT" | "DELETE", path: string, body?: unknown) =>
  runtimeJson<T>(method, path, body, (status, code, msg) => teamErrorMessage(status, code, msg ?? `HTTP ${status}`))

/** "kind:ref" or an app name seen in the team map's grants. */
function resolveApp(map: TeamMap, app: string): { kind: string; ref: string } {
  const m = /^(connector|vendor|artifact):(.+)$/.exec(app)
  if (m) return { kind: m[1], ref: m[2] }
  const hit = map.grants.map((g) => g.app).find((a) => a.name.toLowerCase() === app.toLowerCase() || a.ref === app)
  if (!hit) throw new Error(`Unknown app "${app}". Use kind:ref (e.g. connector:mcp_stripe) or an app a bot already has.`)
  return { kind: hit.kind, ref: hit.ref }
}

function resolveBot(map: TeamMap, bot: string) {
  const n = map.nodes.find((x) => x.kind === "bot" && (x.id === bot || x.id === `b:${bot}` || x.id === `v:${bot}` || x.name.toLowerCase() === bot.toLowerCase()))
  if (!n) throw new Error(teamErrorMessage(404, "bot_not_found", ""))
  return { kind: n.id.startsWith("v:") ? "vendor" : "agent", id: n.id.slice(2) }
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

const TeamMapCommand = cmd({
  command: "map",
  describe: "the team tree: people, their bots and sub-bots, asks and waiting jobs",
  builder: (y: Argv) => y.option("json", { type: "boolean", describe: "print raw JSON" }),
  async handler(args) {
    const m = await rt<TeamMap>("GET", "/api/v1/team/map")
    UI.println(args.json ? JSON.stringify(m, null, 2) : formatTeamMap(m).join(EOL))
  },
})

const TeamAsksCommand = cmd({
  command: "asks",
  describe: "asks waiting on you; --approve <id> or --decline <id> to decide",
  builder: (y: Argv) =>
    y
      .option("approve", { type: "string", describe: "ask id to approve" })
      .option("decline", { type: "string", describe: "ask id to decline" })
      .option("json", { type: "boolean", describe: "print raw JSON" })
      .conflicts("approve", "decline"),
  async handler(args) {
    const r = await rt<{ asks: AskRow[]; partial?: string[] }>("GET", "/api/v1/team/approvals?role=approver&state=pending")
    const id = (args.approve ?? args.decline) as string | undefined
    if (!id) {
      UI.println(args.json ? JSON.stringify(r, null, 2) : formatAsks(r.asks, r.partial).join(EOL))
      return
    }
    const ask = r.asks.find((a) => a.id === id)
    if (!ask) throw new Error(`No pending ask "${id}". Run \`gizzi team asks\` to list them.`)
    if (!ask.resolve) throw new Error("This ask cannot be decided here. Open it in Allternit.")
    const body = { ...(ask.resolve.body as Record<string, unknown>), decision: args.approve ? "approve" : "deny" }
    await call(ask.resolve.method as "POST", ask.resolve.path, body)
    UI.println(`${args.approve ? "Approved" : "Declined"}: ${ask.title}`)
  },
})

const TeamRulesCommand = cmd({
  command: "rules [action] [id]",
  describe: "approval rules: list | add | rm <id>",
  builder: (y: Argv) =>
    y
      .positional("action", { type: "string", choices: ["list", "add", "rm"], default: "list" })
      .positional("id", { type: "string", describe: "rule id (rm)" })
      .option("app", { type: "string", describe: "app name or kind:ref (omit for every app)" })
      .option("tool", { type: "string", describe: 'tool name, "*" for any', default: "*" })
      .option("field", { type: "string", describe: "argument to check, e.g. amount" })
      .option("op", { type: "string", choices: ["always", "gt", "gte", "eq", "in"], describe: "condition" })
      .option("value", { type: "string", describe: "value to compare (numbers are sent as numbers)" })
      .option("approver", { type: "string", describe: "owner, manager or a user id" })
      .option("bot", { type: "string", describe: "limit the rule to one bot (default: whole org)" })
      .option("json", { type: "boolean" }),
  async handler(args) {
    const action = String(args.action ?? "list")
    if (action === "list") {
      const r = await rt<{ rules: RuleRow[] }>("GET", "/api/v1/team/rules")
      UI.println(args.json ? JSON.stringify(r, null, 2) : formatRules(r.rules).join(EOL))
      return
    }
    if (action === "rm") {
      if (!args.id) throw new Error("Usage: gizzi team rules rm <id>")
      await rt("DELETE", `/api/v1/team/rules/${encodeURIComponent(String(args.id))}`)
      UI.println(`Removed rule ${args.id}`)
      return
    }
    if (action !== "add") throw new Error("Usage: gizzi team rules [list|add|rm]")
    if (!args.op || !args.approver) throw new Error("Usage: gizzi team rules add --op <always|gt|gte|eq|in> --approver <owner|manager|user_id> [--app --tool --field --value --bot]")
    const map = args.app || args.bot ? await rt<TeamMap>("GET", "/api/v1/team/map") : null
    const app = args.app ? resolveApp(map!, String(args.app)) : null
    const bot = args.bot ? resolveBot(map!, String(args.bot)) : null
    const raw = args.value as string | undefined
    const value = raw === undefined ? undefined : args.op === "in" ? raw.split(",") : raw !== "" && !isNaN(Number(raw)) ? Number(raw) : raw
    const approver = String(args.approver)
    const r = await rt<{ rule: RuleRow }>("POST", "/api/v1/team/rules", {
      scope_kind: bot ? "bot" : "org",
      scope_ref: bot ? `${bot.kind === "vendor" ? "v" : "b"}:${bot.id}` : undefined,
      app_kind: app?.kind,
      app_ref: app?.ref,
      tool: args.tool,
      field: args.field,
      op: args.op,
      value,
      approver_kind: approver === "owner" || approver === "manager" ? approver : "user",
      approver_user_id: approver === "owner" || approver === "manager" ? undefined : approver,
    })
    UI.println(args.json ? JSON.stringify(r, null, 2) : `Added: ${r.rule.text}`)
  },
})

const TeamGrantCommand = cmd({
  command: "grant <bot> <app> <level>",
  describe: "let a bot use an app at browser, app or full level",
  builder: (y: Argv) =>
    y
      .positional("bot", { type: "string", demandOption: true, describe: "bot name or id" })
      .positional("app", { type: "string", demandOption: true, describe: "app name or kind:ref" })
      .positional("level", { type: "string", choices: ["browser", "app", "full"], demandOption: true }),
  async handler(args) {
    const map = await rt<TeamMap>("GET", "/api/v1/team/map")
    const bot = resolveBot(map, String(args.bot))
    const app = resolveApp(map, String(args.app))
    const r = await rt<{ grant: { level: string; app?: { name: string } } }>("PUT", "/api/v1/team/grants", {
      bot_kind: bot.kind,
      bot_id: bot.id,
      app_kind: app.kind,
      app_ref: app.ref,
      level: args.level,
    })
    UI.println(`${args.bot} can use ${r.grant.app?.name ?? args.app} at ${r.grant.level} level.`)
  },
})

export const TeamCommand = cmd({
  command: "team",
  describe: "your organization's roster: teammates' live sessions",
  builder: (y: Argv) =>
    y
      .option("computer", { type: "string", describe: "which online computer to use for map/asks/rules/grant when more than one is connected" })
      .middleware((a: any) => setComputer(a.computer))
      .command(TeamListCommand)
      .command(TeamSendCommand)
      .command(TeamMapCommand)
      .command(TeamAsksCommand)
      .command(TeamRulesCommand)
      .command(TeamGrantCommand)
      .demandCommand(),
  async handler() {},
})
