/**
 * `gizzi agents bot threads` (spec P9.3): the bot threads Desktop
 * shows, from the terminal — list them, read one, steer it (send a message
 * into it; a running turn picks it up at its next step), resolve it. Uses the
 * platform threads client the pet HUD shares (`runtime/bots/platform-threads`).
 */
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import {
  listSessionMessages,
  messageHandoff,
  sendThreadTurn,
  threadApi,
  type PlatformThread,
} from "@/runtime/bots/platform-threads"

function ago(iso: string): string {
  const s = Math.max(0, (Date.now() - Date.parse(iso)) / 1000)
  if (!Number.isFinite(s)) return ""
  if (s < 60) return "now"
  if (s < 3600) return `${Math.floor(s / 60)}m`
  if (s < 86400) return `${Math.floor(s / 3600)}h`
  return `${Math.floor(s / 86400)}d`
}

/** Threads grouped the way the Threads panel does. */
const GROUP: Record<string, string> = {
  needs_you: "waiting",
  blocked: "waiting",
  working: "working",
  planning: "working",
  queued: "queued",
  review: "waiting",
  paused: "idle",
  idle: "idle",
  done: "resolved",
  failed: "resolved",
}

export function formatThreadRows(threads: PlatformThread[]): string[] {
  const order = ["waiting", "working", "queued", "idle", "resolved"]
  const rows = threads
    .filter((t) => !t.incognito)
    .map((t) => ({ t, group: GROUP[t.status] ?? "idle" }))
    .sort((a, b) => order.indexOf(a.group) - order.indexOf(b.group) || b.t.lastActivityAt.localeCompare(a.t.lastActivityAt))
  const idw = Math.max(2, ...rows.map((r) => r.t.id.slice(0, 8).length))
  const tw = Math.min(48, Math.max(5, ...rows.map((r) => r.t.title.length)))
  return rows.map(({ t, group }) =>
    [t.id.slice(0, 8).padEnd(idw), group.padEnd(8), t.title.slice(0, tw).padEnd(tw), t.status.padEnd(9), ago(t.lastActivityAt)].join("  ").trimEnd(),
  )
}

async function findThread(idOrPrefix: string): Promise<PlatformThread> {
  try {
    return await threadApi.get(idOrPrefix)
  } catch {
    const all = await threadApi.list({})
    const hits = all.filter((t) => t.id.startsWith(idOrPrefix))
    if (hits.length === 1) return hits[0]
    throw new Error(hits.length ? `"${idOrPrefix}" matches ${hits.length} threads; use more of the id` : `No thread "${idOrPrefix}"`)
  }
}

async function run(fn: () => Promise<void>) {
  try {
    await fn()
  } catch (e) {
    UI.error(e instanceof Error ? e.message : String(e))
    process.exitCode = 1
  }
}

export const ThreadListCommand = cmd({
  command: ["list [bot]", "ls [bot]"],
  describe: "List bot threads (optionally one bot's, by id)",
  builder: (y) => y.positional("bot", { type: "string", describe: "bot id" }),
  handler: async (args) =>
    run(async () => {
      const threads = await threadApi.list(args.bot ? { botId: String(args.bot) } : {})
      if (threads.length === 0) {
        UI.println("No threads.")
        return
      }
      UI.println(UI.Style.TEXT_INFO_BOLD + "ID        GROUP     TITLE" + UI.Style.RESET)
      for (const line of formatThreadRows(threads)) UI.println(line)
    }),
})

export const ThreadShowCommand = cmd({
  command: "show <thread>",
  describe: "Show a thread: status and its latest messages",
  builder: (y) =>
    y
      .positional("thread", { type: "string", demandOption: true, describe: "thread id (or a unique prefix)" })
      .option("last", { type: "number", default: 6, describe: "how many messages to show" }),
  handler: async (args) =>
    run(async () => {
      const t = await findThread(String(args.thread))
      UI.println(`${UI.Style.TEXT_INFO_BOLD}${t.title}${UI.Style.RESET}  ${t.status} · window ${t.generation}` + (t.contextUsed != null ? ` · ${Math.round(t.contextUsed * 100)}% of context` : ""))
      if (!t.currentSessionId) return
      const messages = (await listSessionMessages(t.currentSessionId)).slice(-Number(args.last ?? 6))
      for (const m of messages) {
        const handoff = messageHandoff(m)
        if (handoff) {
          UI.println(`${UI.Style.TEXT_DIM}── Fresh context · window ${t.generation} ──${UI.Style.RESET}`)
          continue
        }
        const who = m.role === "user" ? "You" : "Bot"
        UI.println(`${UI.Style.TEXT_DIM}${who}:${UI.Style.RESET} ${m.content.trim()}`)
      }
    }),
})

export const ThreadSteerCommand = cmd({
  command: "steer <thread> <message..>",
  describe: "Send a message into a thread (a running turn picks it up at its next step) and print the reply",
  builder: (y) =>
    y
      .positional("thread", { type: "string", demandOption: true })
      .positional("message", { type: "string", array: true, demandOption: true }),
  handler: async (args) =>
    run(async () => {
      const t = await findThread(String(args.thread))
      const text = (args.message as string[]).join(" ").trim()
      if (!text) throw new Error("Nothing to send")
      const reply = await sendThreadTurn(t, text)
      UI.println(reply.content.trim() || "(no reply)")
    }),
})

export const ThreadResolveCommand = cmd({
  command: "resolve <thread>",
  describe: "Mark a task thread done (or --failed)",
  builder: (y) =>
    y.positional("thread", { type: "string", demandOption: true }).option("failed", { type: "boolean", default: false }),
  handler: async (args) =>
    run(async () => {
      const t = await findThread(String(args.thread))
      const done = await threadApi.resolve(t.id, args.failed ? "failed" : "done")
      UI.println(`${done.title}: ${done.status}`)
    }),
})

/**
 * `gizzi agents bot threads list|show|steer|resolve` — the hosted bots'
 * threads (folded in from the old `gizzi thread`). The engine's threads
 * across every binding are `gizzi orchestration threads`.
 */
export const BotThreadsCommand = cmd({
  command: "threads",
  describe: "Hosted bot threads: list, show, steer, resolve",
  builder: (y) =>
    y
      .command(ThreadListCommand)
      .command(ThreadShowCommand)
      .command(ThreadSteerCommand)
      .command(ThreadResolveCommand)
      .demandCommand(1, "Specify a threads command: list | show | steer | resolve"),
  handler: async () => {},
})
