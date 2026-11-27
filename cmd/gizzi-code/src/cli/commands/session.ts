import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { Session } from "@/runtime/session"
import { bootstrap } from "@/cli/bootstrap"
import { UI } from "@/cli/ui"
import { Locale } from "@/shared/util/locale"
import { Flag } from "@/runtime/context/flag/flag"
import { Filesystem } from "@/shared/util/filesystem"
import { EOL } from "os"
import path from "path"
import * as prompts from "@clack/prompts"
import { SessionTrace } from "@/runtime/session/trace"

function pagerCmd(): string[] {
  const lessOptions = ["-R", "-S"]
  if (process.platform !== "win32") {
    return ["less", ...lessOptions]
  }

  // user could have less installed via other options
  const lessOnPath = Bun.which("less")
  if (lessOnPath) {
    if (Filesystem.stat(lessOnPath)?.size) return [lessOnPath, ...lessOptions]
  }

  if (Flag.GIZZI_GIT_BASH_PATH) {
    const less = path.join(Flag.GIZZI_GIT_BASH_PATH, "..", "..", "usr", "bin", "less.exe")
    if (Filesystem.stat(less)?.size) return [less, ...lessOptions]
  }

  const git = Bun.which("git")
  if (git) {
    const less = path.join(git, "..", "..", "usr", "bin", "less.exe")
    if (Filesystem.stat(less)?.size) return [less, ...lessOptions]
  }

  // Fall back to Windows built-in more (via cmd.exe)
  return ["cmd", "/c", "more"]
}

export const SessionCommand = cmd({
  command: "session",
  describe: "manage sessions",
  builder: (yargs: Argv) => yargs.command(SessionListCommand).command(SessionDeleteCommand).command(SessionExportCommand).demandCommand(),
  async handler() {},
})

export const SessionDeleteCommand = cmd({
  command: "delete <sessionID>",
  describe: "delete a session",
  builder: (yargs: Argv) => {
    return yargs.positional("sessionID", {
      describe: "session ID to delete",
      type: "string",
      demandOption: true,
    })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        await Session.get(args.sessionID)
      } catch {
        UI.error(`Session not found: ${args.sessionID}`)
        process.exit(1)
      }
      await Session.remove(args.sessionID)
      UI.println(UI.Style.TEXT_SUCCESS_BOLD + `Session ${args.sessionID} deleted` + UI.Style.TEXT_NORMAL)
    })
  },
})

export const SessionListCommand = cmd({
  command: "list",
  describe: "list sessions",
  builder: (yargs: Argv) => {
    return yargs
      .option("max-count", {
        alias: "n",
        describe: "limit to N most recent sessions",
        type: "number",
      })
      .option("format", {
        describe: "output format",
        type: "string",
        choices: ["table", "json"],
        default: "table",
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      const sessions = [...Session.list({ roots: true, limit: args.maxCount })]

      if (sessions.length === 0) {
        return
      }

      let output: string
      if (args.format === "json") {
        output = formatSessionJSON(sessions)
      } else {
        output = formatSessionTable(sessions)
      }

      const shouldPaginate = process.stdout.isTTY && !args.maxCount && args.format === "table"

      if (shouldPaginate) {
        const proc = Bun.spawn({
          cmd: pagerCmd(),
          stdin: "pipe",
          stdout: "inherit",
          stderr: "inherit",
        })

        proc.stdin.write(output)
        proc.stdin.end()
        await proc.exited
      } else {
        process.stdout.write(output + "\n")
      }
    })
  },
})

export const SessionExportCommand = cmd({
  command: "export [sessionID]",
  describe: "export session data as JSON",
  builder: (yargs: Argv) => {
    return yargs.positional("sessionID", {
      describe: "session id to export",
      type: "string",
    })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      let sessionID = args.sessionID

      if (!sessionID) {
        UI.empty()
        prompts.intro("Export session", {
          output: process.stderr,
        })

        const sessions = []
        for await (const session of Session.list()) {
          sessions.push(session)
        }

        if (sessions.length === 0) {
          prompts.log.error("No sessions found", {
            output: process.stderr,
          })
          prompts.outro("Done", {
            output: process.stderr,
          })
          return
        }

        sessions.sort((a, b) => b.time.updated - a.time.updated)

        const selectedSession = await prompts.autocomplete({
          message: "Select session to export",
          maxItems: 10,
          options: sessions.map((session) => ({
            label: displayTitle(session),
            value: session.id,
            hint: `${new Date(session.time.updated).toLocaleString()} • ${session.id.slice(-8)}`,
          })),
          output: process.stderr,
        })

        if (prompts.isCancel(selectedSession)) {
          throw new UI.CancelledError()
        }

        sessionID = selectedSession as string

        prompts.outro("Exporting session...", {
          output: process.stderr,
        })
      }

      try {
        const sessionInfo = await Session.get(sessionID!)
        const messages = await Session.messages({ sessionID: sessionID! })

        const exportData = {
          info: sessionInfo,
          messages: messages.map((msg) => ({
            info: msg.info,
            parts: msg.parts,
          })),
          trace: {
            head: SessionTrace.head(sessionID!),
            entries: SessionTrace.list({ sessionID: sessionID!, after: 0, limit: 5_000 }),
          },
        }

        process.stdout.write(JSON.stringify(exportData, null, 2))
        process.stdout.write(EOL)
      } catch (error) {
        UI.error(`Session not found: ${sessionID!}`)
        process.exit(1)
      }
    })
  },
})

/**
 * Same rule as allternit-ai src/lib/sessions/session-display-title.ts: a raw
 * default name ("New session - 2026-11-18T04:47:31.282Z") never shows; use
 * "New session · Nov 17" instead.
 */
function displayTitle(session: Session.Info): string {
  const title = (session.title ?? "").trim()
  if (title && !/^(New|Child) session - \d{4}-\d{2}-\d{2}T/.test(title)) return title
  const stamp = title.match(/\d{4}-\d{2}-\d{2}T[\d:.]+Z?/)?.[0]
  const ts = stamp ? new Date(stamp).getTime() : session.time.created
  const date = new Date(Number.isFinite(ts) ? ts : Date.now()).toLocaleDateString(undefined, { month: "short", day: "numeric" })
  return `${title.startsWith("Child") ? "Child session" : "New session"} · ${date}`
}

function formatSessionTable(sessions: Session.Info[]): string {
  const lines: string[] = []

  const maxIdWidth = Math.max(20, ...sessions.map((s) => s.id.length))
  const maxTitleWidth = Math.max(25, ...sessions.map((s) => displayTitle(s).length))

  const header = `Session ID${" ".repeat(maxIdWidth - 10)}  Title${" ".repeat(maxTitleWidth - 5)}  Updated      Status`
  lines.push(header)
  lines.push("─".repeat(header.length))
  for (const session of sessions) {
    const truncatedTitle = Locale.truncate(displayTitle(session), maxTitleWidth)
    const timeStr = Locale.todayTimeOrDateTime(session.time.updated)
    const mark = session.time.archived ? "○ archived" : "● active"
    const line = `${session.id.padEnd(maxIdWidth)}  ${truncatedTitle.padEnd(maxTitleWidth)}  ${timeStr.padEnd(12)} ${mark}`
    lines.push(line)
  }

  return lines.join(EOL)
}

function formatSessionJSON(sessions: Session.Info[]): string {
  const jsonData = sessions.map((session) => ({
    id: session.id,
    title: session.title,
    updated: session.time.updated,
    created: session.time.created,
    projectId: session.projectID,
    directory: session.directory,
  }))
  return JSON.stringify(jsonData, null, 2)
}
