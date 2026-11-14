import type { Argv } from "yargs"
import path from "path"
import { pathToFileURL } from "bun"
import { UI } from "@/cli/ui"
import { cmd } from "@/cli/commands/cmd"
import { Flag } from "@/runtime/context/flag/flag"
import { bootstrap } from "@/cli/bootstrap"
import { EOL } from "os"
import { Filesystem } from "@/shared/util/filesystem"
import { createAllternitClient, type AllternitClient } from "@allternit/sdk"
type Client = AllternitClient

// Local type definitions (SDK exports unknown)
type ToolPart = {
  id: string
  sessionID: string
  messageID: string
  type: "tool"
  tool: string
  state: {
    status: "pending" | "running" | "completed" | "error"
    input?: Record<string, unknown>
    output?: string
    error?: string
    title?: string
    metadata?: Record<string, unknown>
  }
}
import { Server } from "@/runtime/server/server"
import { Provider } from "@/runtime/providers/provider"
import { ReviewVerdict } from "@/runtime/review/verdict"
import { Agent } from "@/runtime/loop/agent"
import { AgentManager } from "@/runtime/loop/manager"
import { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { Config } from "@/runtime/context/config/config"
import { ToolPrinter, columns } from "@/cli/commands/run-print"
import { TurnSummary } from "@/shared/util/turn-summary"
import chalk from "@/shared/util/chalk"
import { runOnComputer } from "@/cli/commands/computers"
import { ModelThinking } from "@/runtime/providers/discovery/model-thinking"

/** Every level some model takes; the check when the model has no row in model-thinking.json. */
const KNOWN_EFFORTS = ["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"]

/**
 * Why `--effort` cannot be used with this model, or undefined when it can.
 * A model with a row in model-thinking.json takes only its own levels.
 */
export function effortProblem(effort: string, model?: string): string | undefined {
  const level = effort.trim().toLowerCase()
  const modelID = model ? model.slice(model.indexOf("/") + 1) : undefined
  const entry = modelID ? ModelThinking.of(modelID) : undefined
  if (entry) {
    if (entry.levels.includes(level)) return undefined
    return entry.levels.length === 0
      ? `--effort: ${model} has no effort levels`
      : `--effort: ${model} takes ${entry.levels.join(", ")} (got "${effort}")`
  }
  if (KNOWN_EFFORTS.includes(level)) return undefined
  return `--effort: unknown level "${effort}"; use one of ${KNOWN_EFFORTS.join(", ")}`
}

// Gizzi sand through the chalk shim: 256-color terminals get 180, NO_COLOR gets nothing.
const sand = (s: string) => chalk.rgb(212, 176, 140)(s)

function header(agent: string, modelID: string) {
  const D = UI.Style.TEXT_DIM
  const N = UI.Style.TEXT_NORMAL
  const B = UI.Style.TEXT_NORMAL_BOLD
  UI.empty()
  UI.println(`${sand("──")} ${B}${agent}${N}${D} · ${N}${modelID} ${sand("──")}`)
  UI.empty()
}

function fmtNum(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`
  return String(n)
}

export const RunCommand = cmd({
  command: "run [message..]",
  describe: "run gizzi with a message",
  builder: (yargs: Argv) => {
    return yargs
      .positional("message", {
        describe: "message to send",
        type: "string",
        array: true,
        default: [],
      })
      .option("command", {
        describe: "the command to run, use message for args",
        type: "string",
      })
      .option("continue", {
        alias: ["c"],
        describe: "continue the last session",
        type: "boolean",
      })
      .option("session", {
        alias: ["s"],
        describe: "session id to continue",
        type: "string",
      })
      .option("fork", {
        describe: "fork the session before continuing (requires --continue or --session)",
        type: "boolean",
      })
      .option("share", {
        type: "boolean",
        describe: "share the session",
      })
      .option("model", {
        type: "string",
        alias: ["m"],
        describe: "model to use in the format of provider/model",
      })
      .option("agent", {
        type: "string",
        describe: "agent to use",
      })
      .option("format", {
        type: "string",
        choices: ["default", "json"],
        default: "default",
        describe: "format: default (formatted) or json (raw JSON events)",
      })
      .option("file", {
        alias: ["f"],
        type: "string",
        array: true,
        describe: "file(s) to attach to message",
      })
      .option("title", {
        type: "string",
        describe: "title for the session (uses truncated prompt if no value provided)",
      })
      .option("attach", {
        type: "string",
        describe: "attach to a running gizzi server (e.g., http://localhost:4096)",
      })
      .option("on", {
        type: "string",
        describe: "run this session on a paired computer (name or id from `gizzi computers ls`)",
      })
      .option("dir", {
        type: "string",
        describe: "directory to run in, path on remote server if attaching",
      })
      .option("worktree", {
        type: "string",
        describe: "override the git worktree root for sandbox boundary",
      })
      .option("port", {
        type: "number",
        describe: "port for the local server (defaults to random port if no value provided)",
      })
      .option("variant", {
        type: "string",
        describe: "model variant (provider-specific reasoning effort, e.g., high, max, minimal)",
      })
      .option("thinking", {
        type: "boolean",
        describe: "show thinking blocks",
        default: false,
      })
      .option("print", {
        alias: "p",
        type: "boolean",
        describe: "print response and exit (pipe-friendly, no TUI)",
        default: false,
      })
      .option("print-background-mode", {
        type: "string",
        choices: ["exit", "drain", "steer"] as const,
        default: "exit" as const,
        describe: "background task behavior after the main print turn completes",
      })
      .option("print-wait-ceiling", {
        type: "number",
        default: 600,
        describe: "maximum seconds to wait for print-mode background drain or steering",
      })
      .option("print-max-turns", {
        type: "number",
        default: 20,
        describe: "maximum completion-driven parent turns in print steer mode",
      })
      .option("output-format", {
        type: "string",
        describe: "output format: text, json, stream-json",
        choices: ["text", "json", "stream-json"] as const,
      })
      .option("permission-mode", {
        type: "string",
        describe: "permission mode for tool execution",
        choices: ["default", "acceptEdits", "plan", "dontAsk", "bypassPermissions"] as const,
      })
      .option("dangerously-skip-permissions", {
        type: "boolean",
        describe: "skip all permission checks (use in sandboxed environments only)",
        default: false,
      })
      .option("dangerously-skip-sandbox", {
        type: "boolean",
        describe: "run the Bash tool fully unsandboxed (sandboxed by default otherwise)",
        default: false,
      })
      .option("allowedTools", {
        type: "array",
        string: true,
        describe: "whitelist of tool name patterns (glob)",
      })
      .option("disallowedTools", {
        type: "array",
        string: true,
        describe: "blacklist of tool name patterns (glob)",
      })
      .option("system-prompt", {
        type: "string",
        describe: "override the system prompt",
      })
      .option("append-system-prompt", {
        type: "string",
        describe: "append text to the system prompt",
      })
      .option("max-budget-usd", {
        type: "number",
        describe: "maximum cost budget in USD before aborting",
      })
      .option("effort", {
        type: "string",
        describe: "reasoning effort level, checked against the model's levels (alias for --variant)",
      })
      .option("fallback-model", {
        type: "string",
        describe: "fallback model when primary fails (format: provider/model)",
      })
      .option("input-format", {
        type: "string",
        describe: "input format for stdin: text (default) or stream-json (newline-delimited JSON messages)",
        choices: ["text", "stream-json"] as const,
      })
      .option("json-schema", {
        type: "string",
        describe: "JSON schema for structured output validation (inline JSON or file path)",
      })
      .option("from-pr", {
        type: "number",
        describe: "GitHub PR number to fetch context from",
      })
      .option("review", {
        type: "boolean",
        describe:
          "after the run (or alone when no message is given), review the changes with an independent reviewer; exit 0 PASS or nothing to review, 3 FAIL, 4 PARTIAL",
      })
      .option("review-until-pass", {
        type: "string",
        describe: "like --review, but loop review → fix → re-review until PASS (optional max rounds, default 5, cap 12)",
      })
      .option("plugin-dir", {
        type: "string",
        describe: "directory to load additional plugins from (scoped to this session)",
      })
  },
  handler: async (args) => {
    // Set permission flags before anything else
    if (args.permissionMode) Flag.GIZZI_PERMISSION_MODE = args.permissionMode
    if (args.dangerouslySkipPermissions) Flag.GIZZI_SKIP_PERMISSIONS = true
    if (args.dangerouslySkipSandbox) Flag.GIZZI_SANDBOX_DISABLE = true
    if (args.worktree) Flag.GIZZI_WORKTREE = path.resolve(args.worktree)
    if (args.fallbackModel) Flag.GIZZI_FALLBACK_MODEL = args.fallbackModel

    // The person's own plans answer only messages sent from the chat, never a headless run.
    const { refuseIfPersonOnly } = await import("@/runtime/providers/person-plan/rules")
    const personOnly = refuseIfPersonOnly(args.model, "headless") ?? refuseIfPersonOnly(args.fallbackModel, "headless")
    if (personOnly) {
      UI.error(personOnly)
      // Let stderr drain before exiting, or a pipe can lose the message.
      await new Promise<void>((resolve) => process.stderr.write("", () => resolve()))
      process.exit(1)
    }

    // Effort is an alias for variant, checked against the picked model's levels.
    if (args.effort) {
      const problem = effortProblem(args.effort, args.model)
      if (problem) {
        UI.error(problem)
        process.exit(1)
      }
      if (!args.variant) args.variant = args.effort.trim().toLowerCase()
    }

    // Plugin dir override
    if (args.pluginDir) {
      process.env.GIZZI_PLUGIN_DIR = path.resolve(args.pluginDir)
    }

    let message = [...args.message, ...(args["--"] || [])]
      .map((arg) => (arg.includes(" ") ? `"${arg.replace(/"/g, '\\"')}"` : arg))
      .join(" ")

    if (args.on) {
      process.exitCode = await runOnComputer(String(args.on), message, args.model, args.title)
      return
    }

    const directory = (() => {
      if (!args.dir) return undefined
      if (args.attach) return args.dir
      try {
        process.chdir(args.dir)
        return process.cwd()
      } catch {
        UI.error("Failed to change directory to " + args.dir)
        process.exit(1)
      }
    })()

    const files: { type: "file"; url: string; filename: string; mime: string }[] = []
    if (args.file) {
      const list = Array.isArray(args.file) ? args.file : [args.file]

      for (const filePath of list) {
        const resolvedPath = path.resolve(process.cwd(), filePath)
        if (!(await Filesystem.exists(resolvedPath))) {
          UI.error(`File not found: ${filePath}`)
          process.exit(1)
        }

        const mime = (await Filesystem.isDir(resolvedPath)) ? "application/x-directory" : "text/plain"

        files.push({
          type: "file",
          url: pathToFileURL(resolvedPath).href,
          filename: path.basename(resolvedPath),
          mime,
        })
      }
    }

    const stdinMessages: Array<{ role: string; content: string }>  = []
    if (!process.stdin.isTTY) {
      const stdinText = await Bun.stdin.text()
      if (args.inputFormat === "stream-json") {
        for (const line of stdinText.split("\n")) {
          const trimmed = line.trim()
          if (!trimmed) continue
          try {
            const parsed = JSON.parse(trimmed)
            if (parsed.content) {
              stdinMessages.push({ role: parsed.role || "user", content: parsed.content })
            }
          } catch {
            // non-JSON lines treated as plain text
            stdinMessages.push({ role: "user", content: trimmed })
          }
        }
        if (stdinMessages.length > 0 && !message.trim()) {
          // Use last message as the primary prompt, earlier ones as conversation history
          message = stdinMessages.pop()!.content
        }
      } else {
        message += "\n" + stdinText
      }
    }

    const reviewUntilPass = args.reviewUntilPass !== undefined
    const reviewRounds = reviewUntilPass && /^\d+$/.test(String(args.reviewUntilPass)) ? Number(args.reviewUntilPass) : undefined
    const reviewRequested = Boolean(args.review) || reviewUntilPass
    const reviewOnly = reviewRequested && message.trim().length === 0 && !args.command

    // Review routes are not in the generated SDK; call them on the same server.
    let serverFetch: ((route: string, init: RequestInit) => Promise<Response>) | undefined

    if (message.trim().length === 0 && !args.command && !reviewRequested) {
      UI.error("You must provide a message or a command")
      process.exit(1)
    }

    if (args.fork && !args.continue && !args.session) {
      UI.error("--fork requires --continue or --session")
      process.exit(1)
    }

    const rules: PermissionNext.Ruleset = [
      {
        permission: "question",
        action: "deny",
        pattern: "*",
      },
      {
        permission: "plan_enter",
        action: "deny",
        pattern: "*",
      },
      {
        permission: "plan_exit",
        action: "deny",
        pattern: "*",
      },
    ]

    // Tool filtering via --allowedTools / --disallowedTools
    if (args.allowedTools?.length) {
      // Deny everything first, then allow matching patterns
      rules.push({ permission: "*", action: "deny", pattern: "*" })
      for (const pattern of args.allowedTools) {
        rules.push({ permission: pattern, action: "allow", pattern: "*" })
      }
    }
    if (args.disallowedTools?.length) {
      for (const pattern of args.disallowedTools) {
        rules.push({ permission: pattern, action: "deny", pattern: "*" })
      }
    }

    function title() {
      if (args.title === undefined) return
      if (args.title !== "") return args.title
      return message.slice(0, 50) + (message.length > 50 ? "..." : "")
    }

    async function session(sdk: Client) {
      const baseID = args.continue
        ? (await sdk.session.list()).data?.find((s: { parentID?: string }) => !s.parentID)?.id
        : args.session

      if (baseID && args.fork) {
        const forked = await sdk.session.fork({ path: { sessionID: baseID }, body: {} })
        return (forked.data as { id?: string } | undefined)?.id
      }

      if (baseID) return baseID

      const name = title()
      const result = await sdk.session.create({
        body: { title: name, permission: rules } as any,
      })
      return (result.data as { id?: string } | undefined)?.id
    }

    async function share(sdk: Client, sessionID: string) {
      const cfg = await sdk.config.get()
      if (!cfg.data) return
      const cfgData = cfg.data as { share?: string }
      if (cfgData.share !== "auto" && !Flag.GIZZI_AUTO_SHARE && !args.share) return
      const res = await sdk.session.share({ path: { sessionID } }).catch((error: unknown) => {
        if (error instanceof Error && error.message.includes("disabled")) {
          UI.println(UI.Style.TEXT_DANGER_BOLD + "!  " + error.message)
        }
        return { error }
      })
      const resData = (res as { data?: { share?: { url?: string } } }).data
      if (!(res as { error?: unknown }).error && resData?.share?.url) {
        UI.println(UI.Style.TEXT_INFO_BOLD + "~  " + resData.share.url)
      }
    }

    async function execute(sdk: Client) {
      const printMode = args.print || args.outputFormat === "text"
      const streamJsonMode = args.outputFormat === "stream-json"
      const jsonMode = args.format === "json" || args.outputFormat === "json"
      const printBuffer: string[] = []
      const backgroundPolicy = printMode ? args.printBackgroundMode : "steer"
      const backgroundDeadline = Date.now() + Math.max(1, args.printWaitCeiling) * 1000
      const pendingBackground = new Set<string>()
      let parentIdle = false
      let parentIdleCount = 0

      function emit(type: string, data: Record<string, unknown>) {
        if (streamJsonMode) {
          process.stdout.write(JSON.stringify({ type, timestamp: Date.now(), sessionID, ...data }) + EOL)
          return true
        }
        if (jsonMode) {
          process.stdout.write(JSON.stringify({ type, timestamp: Date.now(), sessionID, ...data }) + EOL)
          return true
        }
        return false
      }

      const eventStream = sdk.events()
      let backgroundWaitTimer: ReturnType<typeof setTimeout> | undefined
      const armBackgroundWaitCeiling = () => {
        if (backgroundWaitTimer || !printMode || backgroundPolicy === "exit") return
        const remaining = Math.max(1, backgroundDeadline - Date.now())
        backgroundWaitTimer = setTimeout(() => {
          UI.error(`Print background wait ceiling reached (${args.printWaitCeiling}s)`)
          void (eventStream as any).return?.()
        }, remaining)
      }
      let error: string | undefined
      const printer = new ToolPrinter((line) => UI.println(line))
      const turnStart = Date.now()
      let sawText = false
      let lastText: string | undefined
      let lastFinishError = false
      let lastModel: string | undefined
      let totalCost = 0
      let totalTokensIn = 0
      let totalTokensOut = 0

      async function loop() {
        const toggles = new Map<string, string>()
        let lastMessageHeaderID = ""

        for await (const event of eventStream) {
          if ((event.type as string) === "background.task.started") {
            const task = event.properties as any
            if (task.parentSessionID !== sessionID) continue
            pendingBackground.add(task.id)
            emit("background_task_started", { task })
            continue
          }

          if ((event.type as string) === "background.task.finished") {
            const task = event.properties as any
            if (task.parentSessionID !== sessionID) continue
            pendingBackground.delete(task.id)
            emit("background_task_finished", { task })
            if (printMode && backgroundPolicy === "drain" && parentIdle && pendingBackground.size === 0) break
            if (printMode && Date.now() >= backgroundDeadline) {
              UI.error(`Print background wait ceiling reached (${args.printWaitCeiling}s)`)
              break
            }
            continue
          }

          // Per-message assistant header
          if (
            event.type === "message.updated" &&
            event.properties.info.role === "assistant" &&
            !jsonMode && !streamJsonMode && !printMode
          ) {
            const info = event.properties.info as any
            if (info.modelID) lastModel = info.modelID
            if (info.id && info.id !== lastMessageHeaderID) {
              lastMessageHeaderID = info.id
              header(info.agent ?? "gizzi", info.modelID ?? "unknown")
            }
          }

          // Track cost + tokens for budget limit and summary
          if (
            event.type === "message.updated" &&
            event.properties.info.role === "assistant" &&
            event.properties.info.time.completed
          ) {
            const msg = event.properties.info as { cost?: number; tokens?: { input?: number; output?: number }; finish?: string }
            // A CLI brain reports its own failures (unknown model, quota) as
            // reply text with finish "error": that turn failed, not "Done".
            lastFinishError = msg.finish === "error"
            if (msg.cost) totalCost += msg.cost
            if (msg.tokens) {
              totalTokensIn += msg.tokens.input ?? 0
              totalTokensOut += msg.tokens.output ?? 0
            }
            if (args.maxBudgetUsd && totalCost >= args.maxBudgetUsd) {
              UI.error(`Budget limit reached: $${totalCost.toFixed(4)} >= $${args.maxBudgetUsd}`)
              await sdk.session.abort({ path: { sessionID: sessionID! } }).catch(() => {})
              break
            }
          }

          if (event.type === "message.part.updated") {
            const part = event.properties.part
            if (part.sessionID !== sessionID) continue

            if (part.type === "tool") {
              if (part.state.status === "running") {
                if (toggles.has(part.id)) {
                  // Live output (bash metadata.output) streams in the text format only.
                  if (!jsonMode && !streamJsonMode && !printMode) printer.update(part as ToolPart)
                  continue
                }
                toggles.set(part.id, "running")
                if (!emit("tool_running", { part }) && !printMode) printer.update(part as ToolPart)
                continue
              }
              if (part.state.status === "completed" || part.state.status === "error") {
                if (toggles.get(part.id) === "done") continue
                toggles.set(part.id, "done")
                if (emit("tool_use", { part })) continue
                if (printMode) continue
                printer.update(part as ToolPart)
                continue
              }
            }

            if (part.type === "step-start") {
              if (emit("step_start", { part })) continue
            }

            if (part.type === "step-finish") {
              if (emit("step_finish", { part })) continue
            }

            if (part.type === "text" && part.time?.end) {
              if (emit("text", { part })) continue
              const text = part.text.trim()
              if (!text) continue
              if (printMode) {
                printBuffer.push(text)
                continue
              }
              sawText = true
              lastText = text
              const rendered = process.stderr.isTTY ? UI.markdown(text) : text
              if (!process.stdout.isTTY) {
                process.stdout.write(rendered + EOL)
                continue
              }
              printer.separate()
              UI.empty()
              UI.println(rendered)
              UI.empty()
            }

            if (part.type === "reasoning" && part.time?.end && args.thinking) {
              if (emit("reasoning", { part })) continue
              const text = part.text.trim()
              if (!text) continue
              const line = `Thinking: ${text}`
              if (process.stdout.isTTY) {
                UI.empty()
                UI.println(`${UI.Style.TEXT_DIM}\u001b[3m${line}\u001b[0m${UI.Style.TEXT_NORMAL}`)
                UI.empty()
                continue
              }
              process.stdout.write(line + EOL)
            }
          }

          // A session paused before a usage limit waits for the reset (days for a
          // monthly limit). Nobody can resume a headless run, so end it with the reason.
          if ((event.type as string) === "session.paused") {
            const props = event.properties as unknown as { sessionID: string; until: number; limit: string }
            if (props.sessionID !== sessionID) continue
            const err = `Paused before a usage limit (${props.limit}) until ${new Date(props.until).toISOString()}. Run again with --model on another provider.`
            error = error ? error + EOL + err : err
            if (!emit("error", { error: { name: "SessionPaused", data: { message: err, until: props.until, limit: props.limit } } })) UI.error(err)
            process.exitCode = 1
            break
          }

          if (event.type === "session.error") {
            const props = event.properties
            if (props.sessionID !== sessionID || !props.error) continue
            let err = String(props.error.name)
            if ("data" in props.error && props.error.data && typeof props.error.data === "object" && "message" in (props.error.data as object)) {
              err = String((props.error.data as any).message)
            }
            error = error ? error + EOL + err : err
            if (emit("error", { error: props.error })) continue
            UI.error(err)
          }

          if (
            event.type === "session.status" &&
            event.properties.sessionID === sessionID &&
            event.properties.status.type === "idle"
          ) {
            // Flush print buffer in print mode
            if (printMode && printBuffer.length > 0) {
              process.stdout.write(printBuffer.join("\n") + EOL)
              printBuffer.length = 0
            }
            // Done line, or "Stopped" when the turn failed or produced nothing.
            if (!jsonMode && !streamJsonMode && !printMode) {
              const durationMs = Date.now() - turnStart
              const files = printer.files
              UI.empty()
              if (error || lastFinishError || (!sawText && printer.tools === 0)) {
                const cause = error ?? (lastFinishError ? lastText : undefined) ?? "the model returned nothing"
                UI.println(TurnSummary.stopped({ durationMs, files, cause, width: columns() }))
              } else {
                const extra: string[] = []
                if (totalTokensIn > 0) extra.push(`${fmtNum(totalTokensIn)} in · ${fmtNum(totalTokensOut)} out`)
                if (totalCost > 0) extra.push(`$${totalCost.toFixed(4)}`)
                UI.println(`${sand("▞▪▚")} ` + TurnSummary.done({ durationMs, files, toolCount: printer.tools, model: lastModel, extra }))
                const list = TurnSummary.fileList(files, columns())
                if (list) UI.println(list)
              }
            }
            if (!printMode) {
              if (pendingBackground.size === 0) break
              parentIdle = true
              continue
            }

            if (backgroundPolicy === "exit") break
            parentIdle = true
            parentIdleCount += 1
            if (pendingBackground.size === 0) break
            if (Date.now() >= backgroundDeadline) {
              UI.error(`Print background wait ceiling reached (${args.printWaitCeiling}s)`)
              break
            }
            if (backgroundPolicy === "steer" && parentIdleCount > args.printMaxTurns + 1) {
              UI.error(`Print steer max turns reached (${args.printMaxTurns})`)
              break
            }
            armBackgroundWaitCeiling()
            continue
          }

          if ((event as any).type === "permission.asked") {
            const permission = (event as any).properties
            if (permission.sessionID !== sessionID) continue
            if (!printMode) {
              UI.println(
                UI.Style.TEXT_WARNING_BOLD + "!",
                UI.Style.TEXT_NORMAL +
                  `permission requested: ${permission.permission} (${permission.patterns.join(", ")}); auto-rejecting`,
              )
            }
            await sdk.permission.reply({
              path: { requestID: permission.id },
              body: { reply: "reject" },
            }).catch(() => {})
          }
        }
        if (backgroundWaitTimer) clearTimeout(backgroundWaitTimer)
      }

      // Validate agent if specified
      const agent = await (async () => {
        if (!args.agent) return undefined
        // Attached: there is no local project instance; the server resolves the agent.
        if (args.attach) return args.agent
        const entry = await AgentManager.get(args.agent)
        if (!entry) {
          UI.println(
            UI.Style.TEXT_WARNING_BOLD + "!",
            UI.Style.TEXT_NORMAL,
            `agent "${args.agent}" not found. Falling back to default agent`,
          )
          return undefined
        }
        if (entry.mode === "subagent") {
          UI.println(
            UI.Style.TEXT_WARNING_BOLD + "!",
            UI.Style.TEXT_NORMAL,
            `agent "${args.agent}" is a subagent, not a primary agent. Falling back to default agent`,
          )
          return undefined
        }
        return args.agent
      })()

      /** Run the independent review and print the verdict; sets the exit code
       * (0 PASS / nothing to review, 3 FAIL, 4 PARTIAL) unless an error code is already set. */
      async function runReviewStep(reviewSessionID?: string) {
        if (!serverFetch) throw new Error("review server unavailable")
        const untilPass = reviewUntilPass && reviewSessionID
        const response = await serverFetch(reviewSessionID ? `/session/${reviewSessionID}/review` : "/review", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(
            reviewSessionID
              ? { untilPass: Boolean(untilPass), wait: true, ...(reviewRounds ? { maxRounds: reviewRounds } : {}) }
              : {},
          ),
        })
        if (!response.ok) throw new Error(`review failed (${response.status}): ${await response.text()}`)
        const body = (await response.json()) as { verdict: ReviewVerdict.Stored | null; rounds?: number; stopped?: string }
        const verdict = body.verdict
        if (jsonMode || streamJsonMode) {
          process.stdout.write(
            JSON.stringify({
              type: "review",
              timestamp: Date.now(),
              sessionID: reviewSessionID,
              verdict: verdict?.verdict ?? null,
              review: verdict
                ? {
                    verdict: verdict.verdict,
                    summary: verdict.summary,
                    findings: verdict.findings,
                    unverified: verdict.unverified,
                    verified: verdict.verified,
                    files: verdict.files,
                    model: verdict.model,
                    cost: verdict.cost,
                    tokens: verdict.tokens,
                    ...(body.rounds !== undefined ? { rounds: body.rounds, stopped: body.stopped } : {}),
                  }
                : null,
            }) + EOL,
          )
        } else if (verdict) {
          UI.println(ReviewVerdict.format(verdict) + (body.rounds !== undefined ? `\nRounds: ${body.rounds} (${body.stopped})` : ""))
        } else {
          UI.println("Review: nothing to review (no changes)")
        }
        if (!process.exitCode) process.exitCode = ReviewVerdict.exitCode(verdict?.verdict ?? null)
      }

      if (reviewOnly && !reviewUntilPass) {
        await runReviewStep().catch((e) => {
          UI.error(e instanceof Error ? e.message : String(e))
          process.exitCode = 1
        })
        setTimeout(() => process.exit(process.exitCode ?? 0), 100)
        return
      }

      const sessionID = await session(sdk)
      if (!sessionID) {
        UI.error("Session not found")
        process.exit(1)
      }
      await share(sdk, sessionID)

      const loopDone = loop().catch((e) => {
        process.stderr.write(String(e?.stack ?? e) + "\n")
        process.exit(1)
      })

      if (reviewOnly) {
        // --review-until-pass with no message: no prompt; the review loop sends the fix turns.
      } else if (args.command) {
        await sdk.session.command({
          path: { sessionID },
          body: {
            agent,
            model: args.model,
            command: args.command,
            arguments: message,
            variant: args.variant,
            backgroundPolicy,
          } as any,
        })
      } else {
        const model = args.model ? Provider.parseModel(args.model) : undefined
        // If stream-json input provided earlier conversation context, prepend it
        let fullMessage = message
        if (stdinMessages.length > 0) {
          const history = stdinMessages
            .map((m) => `[${m.role}]: ${m.content}`)
            .join("\n")
          fullMessage = `<conversation_history>\n${history}\n</conversation_history>\n\n${message}`
        }
        // Fetch PR context if --from-pr is specified
        if (args.fromPr) {
          try {
            const prProc = Bun.spawn(
              ["gh", "pr", "view", String(args.fromPr), "--json", "title,body,url,headRefName,baseRefName"],
              { cwd: process.cwd(), stdout: "pipe", stderr: "pipe" },
            )
            const prJson = await new Response(prProc.stdout).text()
            const diffProc = Bun.spawn(
              ["gh", "pr", "diff", String(args.fromPr)],
              { cwd: process.cwd(), stdout: "pipe", stderr: "pipe" },
            )
            const prDiff = await new Response(diffProc.stdout).text()
            const prData = JSON.parse(prJson.trim())
            const prContext = [
              `<pr_context>`,
              `PR #${args.fromPr}: ${prData.title}`,
              `Branch: ${prData.headRefName} -> ${prData.baseRefName}`,
              `URL: ${prData.url}`,
              prData.body ? `\nDescription:\n${prData.body}` : "",
              prDiff.trim() ? `\nDiff:\n${prDiff.slice(0, 50000)}` : "",
              `</pr_context>`,
            ].join("\n")
            fullMessage = prContext + "\n\n" + fullMessage
          } catch (e) {
            UI.error(`Failed to fetch PR #${args.fromPr}: ${e instanceof Error ? e.message : String(e)}`)
          }
        }

        // Build system prompt override if specified
        // --system-prompt replaces the entire system prompt
        // --append-system-prompt appends (prefixed with + so server knows to append)
        const systemOverride = (() => {
          if (args.systemPrompt) return args.systemPrompt
          if (args.appendSystemPrompt) return "+" + args.appendSystemPrompt
          return undefined
        })()

        // Parse --json-schema if provided
        const format = await (async () => {
          if (!args.jsonSchema) return undefined
          try {
            // Try as inline JSON first
            const schema = args.jsonSchema.trim().startsWith("{")
              ? JSON.parse(args.jsonSchema)
              : JSON.parse(await Filesystem.readText(path.resolve(args.jsonSchema)))
            return { type: "json_schema" as const, schema }
          } catch (e) {
            UI.error(`Invalid --json-schema: ${e instanceof Error ? e.message : String(e)}`)
            process.exit(1)
          }
        })()

        await sdk.session.prompt({
          path: { sessionID },
          body: {
            agent,
            model,
            variant: args.variant,
            ...(systemOverride ? { system: systemOverride } : {}),
            ...(format ? { format } : {}),
            backgroundPolicy,
            parts: [...files, { type: "text", text: fullMessage }],
          } as any,
        })
      }

      if (!reviewOnly) await loopDone
      // A failed turn (provider error, auth, connect, a CLI's own error reply)
      // exits non-zero; review and usage-limit codes already set stay.
      if (!reviewOnly && (error || lastFinishError) && !process.exitCode) process.exitCode = 1

      if (reviewRequested) {
        await runReviewStep(sessionID).catch((e) => {
          UI.error(e instanceof Error ? e.message : String(e))
          process.exitCode = 1
        })
      }

      // run and exec are one-shot: once the run loop finishes, the process
      // MUST terminate. Background runtime handles — agent communication
      // runtime, lazily re-created Instances, db watchers — can outlive the
      // answer and would otherwise hold the event loop open forever (exec, and
      // plain `gizzi run` too, hung for minutes after the prompt completed).
      // Give stdout/stderr 100ms to drain, then exit hard.
      setTimeout(() => process.exit(process.exitCode ?? 0), 100)
    }

    if (args.attach) {
      const sdk = createAllternitClient({ baseUrl: args.attach, directory })
      const attachURL = args.attach
      serverFetch = (route, init) =>
        fetch(new URL(route, attachURL), {
          ...init,
          headers: { ...(init.headers as Record<string, string>), "x-gizzi-directory": directory ?? process.cwd() },
        })
      return await execute(sdk)
    }

    await bootstrap(process.cwd(), async () => {
      const config = await Config.get()
      if (!args.model) {
        // No --model: the shared default-brain resolver decides (a real pin,
        // else the plan/sign-in rule). Reading config.model directly ran a
        // stale or mistaken pin. An explicit "auto" in config stays auto.
        const configured = config.model?.trim().toLowerCase()
        if (configured === "auto" || configured === "auto/auto") {
          args.model = config.model
        } else {
          const { currentDefaultBrain } = await import("@/runtime/providers/default-brain")
          const brain = await currentDefaultBrain().catch(() => undefined)
          args.model = brain?.model ?? undefined
        }
        // A config pin on a plan model still cannot run headless.
        const pinned = refuseIfPersonOnly(args.model, "headless")
        if (pinned) {
          UI.error(pinned)
          // Let stderr drain before exiting, or a pipe can lose the message.
          await new Promise<void>((resolve) => process.stderr.write("", () => resolve()))
          process.exit(1)
        }
      }
      if (config.sandbox?.enabled !== undefined && process.env.GIZZI_SANDBOX === undefined && process.env.GIZZI_SANDBOX_DISABLE === undefined && !args.dangerouslySkipSandbox) {
        Flag.GIZZI_SANDBOX = config.sandbox.enabled
        Flag.GIZZI_SANDBOX_DISABLE = !config.sandbox.enabled
      }
      if (config.sandbox?.allow_network !== undefined && process.env.GIZZI_SANDBOX_ALLOW_NETWORK === undefined) {
        Flag.GIZZI_SANDBOX_ALLOW_NETWORK = config.sandbox.allow_network
      }
      if (config.sandbox?.allowed_domains !== undefined && process.env.GIZZI_SANDBOX_ALLOWED_DOMAINS === undefined) {
        Flag.GIZZI_SANDBOX_ALLOWED_DOMAINS = config.sandbox.allowed_domains
      }
      const profile = config.auth?.active_profile
        ? config.auth.profiles?.[config.auth.active_profile]
        : undefined
      if (profile) {
        const key = profile.api_key ?? (profile.api_key_env ? process.env[profile.api_key_env] : undefined)
        if (key) process.env[`${profile.provider.toUpperCase().replace(/-/g, "_")}_API_KEY`] ??= key
        if (profile.base_url) process.env[`${profile.provider.toUpperCase().replace(/-/g, "_")}_BASE_URL`] ??= profile.base_url
      }
      // CLI models (claude-cli, codex-cli…) call back into this session's tool bridge over HTTP.
      const loopback = Server.listenLoopback(typeof args.port === "number" ? args.port : 0)
      const fetchFn = (async (input: RequestInfo | URL, init?: RequestInit) => {
        const request = new Request(input, init)
        return Server.App().fetch(request)
      }) as typeof globalThis.fetch
      const sdk = createAllternitClient({ baseUrl: "http://gizzi.internal", fetch: fetchFn })
      serverFetch = (route, init) =>
        fetchFn(new URL(route, "http://gizzi.internal"), {
          ...init,
          headers: { ...(init.headers as Record<string, string>), "x-gizzi-directory": process.cwd() },
        })
      try {
        await execute(sdk)
      } finally {
        // An open listener keeps the process alive after the run.
        loopback.stop(true)
      }
    })
  },
})

/** Pipe-safe alias for automation and CI. Uses the same execution engine as `run`. */
export const ExecCommand = {
  ...RunCommand,
  command: "exec [message..]",
  describe: "run gizzi non-interactively and exit",
  handler: async (args: any) =>
    RunCommand.handler!({
      ...args,
      print: true,
      outputFormat: args.outputFormat ?? "text",
      permissionMode: args.permissionMode ?? "dontAsk",
    } as any),
}
