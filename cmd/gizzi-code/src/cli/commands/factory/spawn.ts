/**
 * `gizzi spawn <cli> "<prompt>"` — a short form of start-anywhere's start call
 * on THIS computer (D7). Same `POST /api/v1/session-start` route and the same
 * client as `gizzi start`, so a spawned session shows up on the phone, web and
 * Desktop too. The prompt and an optional handoff file travel as a task file,
 * never as typed shell text. `--window` then opens a terminal window that only
 * attaches (D8); closing it never ends the session.
 */
import { spawn as spawnProcess, spawnSync } from "node:child_process"
import { randomBytes } from "node:crypto"
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs"
import { homedir } from "node:os"
import { join, resolve } from "node:path"
import type { Argv } from "yargs"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { callerStartMode, checkStart, isStartMode, START_MODES } from "@/cli/work/start-guards"
import { buildStartBody, parentDepth, startSession, type StartArgs } from "@/cli/work/start-session"
import { attachCommand, buildLaunch, formatLaunch, pickTerminal, TERMINALS } from "@/shared/utils/terminalLauncher"

/** Friendly names for runtime ids; anything else goes to the route as typed. */
export function normalizeRuntime(cli: string): string {
  const id = cli.trim().toLowerCase()
  return id === "gizzi" ? "allternit" : id
}

export function taskFilePath(now = Date.now(), rand = randomBytes(4).toString("hex")): string {
  return join(homedir(), ".gizzi", "spawn", `task-${now}-${rand}.md`)
}

/** Task file body: the handoff first (context), then the prompt (the ask). */
export function taskFileText(prompt: string | undefined, handoff: string | undefined): string {
  const parts: string[] = []
  if (handoff?.trim()) parts.push("# Handoff\n\n" + handoff.trim())
  if (prompt?.trim()) parts.push("# Task\n\n" + prompt.trim())
  return parts.join("\n\n") + "\n"
}

/** The one line the session is started with; it points at the task file. */
export function seedPrompt(file: string): string {
  return `Your task is in the file ${file}. Read it first, then carry it out.`
}

function onPath(program: string): boolean {
  const r = spawnSync(process.platform === "win32" ? "where" : "which", [program], { stdio: "ignore" })
  return r.status === 0
}

export const SpawnCommand = cmd({
  command: "spawn <cli> [prompt]",
  describe: "start a session of <cli> on this computer with a prompt (--window opens a terminal for it)",
  builder: (y: Argv) =>
    y
      .positional("cli", { type: "string", demandOption: true, describe: "coding tool id: claude, codex, agy, kimi, opencode, gizzi …" })
      .positional("prompt", { type: "string", describe: "first task for the session" })
      .option("cwd", { type: "string", describe: "folder to work in (default: here)" })
      .option("handoff", { type: "string", describe: "a handoff file the session reads first" })
      .option("runtime", { type: "string", describe: "runtime id; overrides <cli>" })
      .option("mode", { type: "string", choices: [...START_MODES], describe: "permission mode" })
      .option("window", { type: "boolean", describe: "open a new terminal window attached to the session" })
      .option("terminal", { type: "string", choices: [...TERMINALS], describe: "terminal to open (default: detect)" })
      .option("dry-run", { type: "boolean", describe: "print the request (and launch command) without starting anything" })
      .option("json", { type: "boolean", describe: "print JSON" }),
  handler: async (args) => {
    try {
      const prompt = args.prompt ? String(args.prompt) : undefined
      if (!prompt && !args.handoff) {
        UI.error('Give a prompt: gizzi spawn <cli> "<prompt>" (or --handoff <file>).')
        process.exitCode = 1
        return
      }
      let handoff: string | undefined
      if (args.handoff) {
        const hp = resolve(String(args.handoff))
        if (!existsSync(hp)) {
          UI.error(`Handoff file not found: ${hp}`)
          process.exitCode = 1
          return
        }
        handoff = readFileSync(hp, "utf8")
      }
      const envMode = process.env.ALLTERNIT_SESSION_START_MODE
      const callerMode = isStartMode(envMode) ? envMode : process.stdin.isTTY ? "bypass" : callerStartMode("default")
      const guard = checkStart({
        callerMode,
        requestedMode: isStartMode(args.mode) ? args.mode : undefined,
        parentDepth: parentDepth(),
      })
      if (!guard.ok) {
        UI.error(guard.message)
        process.exitCode = 1
        return
      }
      const folder = resolve(args.cwd ? String(args.cwd) : process.cwd())
      const file = taskFilePath()
      const startArgs: StartArgs = {
        folder,
        runtime: normalizeRuntime(String(args.runtime ?? args.cli)),
        mode: guard.mode,
        prompt: seedPrompt(file),
        depth: guard.depth,
      }
      const terminal = args.window ? pickTerminal({ platform: process.platform, env: process.env, override: args.terminal, has: onPath }) : undefined

      if (args["dry-run"]) {
        // The id is unknown until the session starts; the window attaches with a placeholder here.
        const launch = terminal ? buildLaunch(terminal, attachCommand("engine", "<session-id>"), folder) : undefined
        const out = {
          taskFile: file,
          taskFileText: taskFileText(prompt, handoff),
          request: { method: "POST", path: "/api/v1/session-start", body: buildStartBody(startArgs) },
          ...(launch ? { launch: launch.argv, launchCwd: launch.cwd } : {}),
        }
        process.stdout.write(args.json ? JSON.stringify(out, null, 2) + "\n" : formatDryRun(out, launch && formatLaunch(launch)))
        return
      }

      mkdirSync(join(homedir(), ".gizzi", "spawn"), { recursive: true, mode: 0o700 })
      writeFileSync(file, taskFileText(prompt, handoff), { mode: 0o600 })
      const result = await startSession(startArgs)
      const attach = attachCommand(result.kind, result.id)
      if (terminal) {
        const launch = buildLaunch(terminal, attach, result.directory)
        const [program, ...rest] = launch.argv
        const child = spawnProcess(program!, rest, { cwd: launch.cwd, detached: true, stdio: "ignore" })
        child.on("error", (e) => UI.error(`Could not open ${terminal}: ${e.message}. Attach with: ${attach.join(" ")}`))
        child.unref()
      }
      if (args.json) {
        process.stdout.write(JSON.stringify({ ...result, taskFile: file, attach: attach.join(" ") }, null, 2) + "\n")
        return
      }
      process.stdout.write(`${result.line}\nAttach: ${attach.join(" ")}\n`)
    } catch (e) {
      UI.error((e as Error).message)
      process.exitCode = 1
    }
  },
})

function formatDryRun(o: { taskFile: string; request: { method: string; path: string; body: unknown } }, launch?: string): string {
  const lines = [`Task file: ${o.taskFile}`, `${o.request.method} ${o.request.path}`, JSON.stringify(o.request.body, null, 2)]
  if (launch) lines.push(`Launch: ${launch}`)
  return lines.join("\n") + "\n"
}
