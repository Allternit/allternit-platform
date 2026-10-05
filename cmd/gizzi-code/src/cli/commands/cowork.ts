/**
 * Cowork Runtime CLI Command
 *
 * Terminal interface for managing runs in the Cowork Runtime.
 * Provides commands for run lifecycle, scheduling, approvals, and checkpoints.
 *
 * Usage:
 *   gizzi workspace tasks list                    # List all runs
 *   gizzi workspace tasks start <name>            # Create and start a run
 *   gizzi workspace tasks attach <run-id>         # Attach to a running run
 *   gizzi workspace tasks stop <run-id>           # Stop a run
 *   gizzi workspace tasks schedule create <name>  # Create a schedule
 *   gizzi workspace tasks approval list           # List pending approvals
 *
 * @module cowork-command
 * @see ../../COWORK.md for full documentation
 */

import type { Argv } from "yargs"
import yargs from "yargs"
import * as React from "react"
import { cmd } from "@/cli/commands/cmd"
import { bootstrap } from "@/cli/bootstrap"
import { UI } from "@/cli/ui"
import * as prompts from "@clack/prompts"
import { Global } from "@/runtime/context/global"
import { legacyEnv } from "@/shared/constants/cloudUrls"
import open from "open"
import { render } from "@/ink"
import { IntelliTaskScreen } from "@/screens/IntelliTaskScreen"
import { classifyAllternitToken } from "@/shared/utils/allternitToken"

// ============================================================================
// Types
// ============================================================================

interface RunSummary {
  id: string
  name: string
  mode: string
  status: string
  completed_steps: number
  total_steps?: number
  created_at: string
  updated_at: string
}

interface RunDetails {
  id: string
  name: string
  description?: string
  mode: string
  status: string
  step_cursor?: string
  total_steps?: number
  completed_steps: number
  config: Record<string, unknown>
  created_at: string
  started_at?: string
  completed_at?: string
  error_message?: string
}

interface Event {
  id: string
  sequence: number
  event_type: string
  payload: Record<string, unknown>
  created_at: string
}

interface ScheduleSummary {
  id: string
  name: string
  enabled: boolean
  cron_expr: string
  natural_lang?: string
  next_run_at?: string
  run_count: number
}

interface ApprovalSummary {
  id: string
  run_id: string
  status: string
  priority: string
  title: string
  action_type?: string
  created_at: string
  responded_at?: string
}

interface CheckpointSummary {
  id: string
  name?: string
  step_cursor: string
  resumable: boolean
  created_at: string
}

interface TaskItem {
  id: string
  workspace_id: string
  title: string
  description?: string
  status: string
  priority: number
  estimated_minutes?: number
  estimatedMinutes?: number
  deadline?: number | string
  dependencies: string[]
  assignee_type?: string
  assignee_id?: string
  assignee_name?: string
  optimize_rank?: number
}

interface Comment {
  id: string
  task_id: string
  author: string
  body: string
  created_at: string
}

// ============================================================================
// API Client Helper
// ============================================================================

// Local Allternit backend (allternit-api / desktop-cloud). No public URL exists
// yet — see the Backend B productionization track before changing this default.
const API_BASE = process.env.ALLTERNIT_API_URL || "http://localhost:3001"

async function apiCall<T>(
  method: "GET" | "POST" | "PUT" | "DELETE",
  path: string,
  body?: unknown
): Promise<T> {
  const url = `${API_BASE}${path}`
  let token = legacyEnv("ALLTERNIT_API_TOKEN", "Allternit_API_TOKEN")
  if (!token) {
    try {
      const fs = require("fs")
      const path = require("path")
      const os = require("os")
      const configDir = process.env.GIZZI_CONFIG_DIR ?? path.join(os.homedir(), ".config", "gizzi-code")
      const sessionPath = path.join(configDir, "session.json")
      if (fs.existsSync(sessionPath)) {
        const session = JSON.parse(fs.readFileSync(sessionPath, "utf8"))
        token = session?.accessToken || null
        if (token) {
          const info = classifyAllternitToken(token)
          if (info.kind === "jwt" && info.expiresAt && info.expiresAt.getTime() - Date.now() < 60_000) {
            process.stderr.write(
              "warning: stored session token is expired (or expires within 60s); run a fresh `gizzi login` or store a durable alt_ token with `gizzi api-keys set`\n",
            )
          }
        }
      }
    } catch {
      // Ignore reading error
    }
  }

  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    Accept: "application/json",
  }

  // Never fall back to a hardcoded dev token: send credentials only when a
  // real token is configured.
  if (token) {
    headers["Authorization"] = `Bearer ${token}`
  }

  const response = await fetch(url, {
    method,
    headers,
    body: body ? JSON.stringify(body) : undefined,
  })

  if (!response.ok) {
    const error = await response.text()
    throw new Error(`API Error ${response.status}: ${error}`)
  }

  if (response.status === 204) {
    return undefined as T
  }

  return response.json() as Promise<T>
}

// ============================================================================
// Main Command
// ============================================================================

export const CoworkCommand = cmd({
  command: "tasks",
  describe: "the task queue: runs, schedules, approvals and checkpoints",
  builder: (yargs: Argv) => {
    return yargs
      .command(CoworkListCommand)
      .command(CoworkStartCommand)
      .command(CoworkAttachCommand)
      .command(CoworkDetachCommand)
      .command(CoworkStopCommand)
      .command(CoworkLogsCommand)
      .command(CoworkShowCommand)
      .command(CoworkPauseCommand)
      .command(CoworkResumeCommand)
      .command(CoworkScheduleCommand)
      .command(CoworkApprovalCommand)
      .command(CoworkCheckpointCommand)
      .command(CoworkWebCommand)
      .command(CoworkTasksCommand)
      .command(CoworkQueueCommand)
      .command(CoworkBoardCommand)
      .command(CoworkOptimizeCommand)
      .command(CoworkTimerCommand)
      .command(CoworkRuntimeCommand)
      .command(CoworkWorkspaceCommand)
      .command(CoworkRunDaemonCommand)
      .demandCommand(1, "Choose a command")
  },
  async handler() {
    // Show help if no subcommand
    UI.println(UI.Style.TEXT_NORMAL_BOLD + "Cowork Runtime Management" + UI.Style.TEXT_NORMAL)
    UI.println("")
    UI.println("Commands:")
    UI.println("  list, ls          List all runs")
    UI.println("  start, new        Create and start a new run")
    UI.println("  attach            Attach to a running run")
    UI.println("  detach            Detach from a run")
    UI.println("  stop, cancel      Stop/cancel a run")
    UI.println("  logs              View run logs")
    UI.println("  show, info        Show run details")
    UI.println("  pause             Pause a run")
    UI.println("  resume            Resume a paused run")
    UI.println("  schedule          Manage schedules")
    UI.println("  approval          Manage approvals")
    UI.println("  checkpoint        Manage checkpoints")
    UI.println("  web               Mirror session to web browser")
    UI.println("  tasks, t          View and optimize cowork tasks (TUI)")
    UI.println("  board             Open board view")
    UI.println("  optimize          Run schedule optimization")
    UI.println("  timer             Start/stop task timer")
    UI.println("  runtime           List or register runtimes")
    UI.println("  workspace         List or switch workspaces")
    UI.println("")
    UI.println(`Run ${UI.Style.TEXT_NORMAL_BOLD}gizzi workspace tasks <command> --help${UI.Style.TEXT_NORMAL} for more information`)
  },
})

// ============================================================================
// Run Commands
// ============================================================================

export const CoworkListCommand = cmd({
  command: "list",
  aliases: ["ls"],
  describe: "list all runs",
  builder: (yargs: Argv) => {
    return yargs
      .option("status", {
        alias: "s",
        describe: "Filter by status (pending,running,completed,failed)",
        type: "string",
      })
      .option("mode", {
        alias: "m",
        describe: "Filter by mode (local,remote,cloud)",
        type: "string",
      })
      .option("limit", {
        alias: "l",
        describe: "Maximum results",
        type: "number",
        default: 20,
      })
      .option("format", {
        describe: "Output format",
        type: "string",
        choices: ["table", "json"],
        default: "table",
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        // Build query params
        const params = new URLSearchParams()
        if (args.status) params.append("status", args.status)
        if (args.mode) params.append("mode", args.mode)
        params.append("limit", String(args.limit))

        const query = params.toString() ? `?${params.toString()}` : ""
        const runs = await apiCall<RunSummary[]>("GET", `/api/v1/runs${query}`)

        if (args.format === "json") {
          UI.println(JSON.stringify(runs, null, 2))
          return
        }

        if (runs.length === 0) {
          UI.println("No runs found")
          return
        }

        // Table output
        UI.println(`${UI.Style.TEXT_NORMAL_BOLD}${"ID".padEnd(10)} ${"NAME".padEnd(20)} ${"MODE".padEnd(8)} ${"STATUS".padEnd(12)} ${"PROGRESS".padEnd(10)} ${"CREATED"}${UI.Style.TEXT_NORMAL}`)
        UI.println("-".repeat(90))

        for (const run of runs) {
          const id = run.id.slice(0, 8)
          const name = run.name.slice(0, 20).padEnd(20)
          const mode = run.mode.padEnd(8)
          const status = run.status.padEnd(12)
          const progress = run.total_steps
            ? `${run.completed_steps}/${run.total_steps}`.padEnd(10)
            : "-".padEnd(10)
          const created = new Date(run.created_at).toLocaleString()

          UI.println(`${id} ${name} ${mode} ${status} ${progress} ${created}`)
        }

        UI.println("")
        UI.println(`Showing ${runs.length} run(s)`)
      } catch (error) {
        UI.error(`Failed to list runs: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkStartCommand = cmd({
  command: "start <name>",
  aliases: ["new"],
  describe: "create and start a new run",
  builder: (yargs: Argv) => {
    return yargs
      .positional("name", {
        describe: "Run name",
        type: "string",
        demandOption: true,
      })
      .option("mode", {
        alias: "m",
        describe: "Execution mode",
        type: "string",
        choices: ["local", "remote", "cloud"],
        default: "local",
      })
      .option("runtime", {
        alias: "r",
        describe: "Runtime type (for local mode)",
        type: "string",
        choices: ["shell", "docker", "vm"],
        default: "shell",
      })
      .option("command", {
        alias: "c",
        describe: "Command to execute",
        type: "string",
      })
      .option("working-dir", {
        alias: "w",
        describe: "Working directory",
        type: "string",
      })
      .option("attach", {
        alias: "a",
        describe: "Auto-attach after starting",
        type: "boolean",
        default: false,
      })
      .option("env", {
        alias: "e",
        describe: "Environment variables (KEY=value)",
        type: "string",
        array: true,
        default: [],
      })
      .option("image", {
        describe: "Docker image (for docker runtime)",
        type: "string",
      })
      // Remote mode options
      .option("host", {
        describe: "Remote host (remote mode)",
        type: "string",
      })
      .option("port", {
        describe: "SSH port (remote mode)",
        type: "number",
        default: 22,
      })
      .option("username", {
        describe: "SSH username (remote mode)",
        type: "string",
      })
      .option("ssh-key", {
        describe: "SSH private key path (remote mode)",
        type: "string",
      })
      // Cloud mode options
      .option("provider", {
        describe: "Cloud provider (cloud mode)",
        type: "string",
        choices: ["hetzner", "aws", "digitalocean"],
      })
      .option("region", {
        describe: "Cloud region (cloud mode)",
        type: "string",
      })
      .option("instance-type", {
        describe: "Instance type (cloud mode)",
        type: "string",
      })
      .option("storage-gb", {
        describe: "Storage size in GB (cloud mode)",
        type: "number",
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        // Parse environment variables
        const envMap: Record<string, string> = {}
        for (const envStr of args.env) {
          const [key, ...valueParts] = envStr.split("=")
          if (key && valueParts.length > 0) {
            envMap[key] = valueParts.join("=")
          }
        }

        // Build config
        const config: Record<string, unknown> = {
          command: args.command,
          working_dir: args.workingDir,
          env: Object.keys(envMap).length > 0 ? envMap : undefined,
        }

        // Add mode-specific options
        if (args.mode === "local") {
          config["runtime"] = args.runtime
          if (args.image) {
            config["image"] = args.image
          }
        }

        if (args.mode === "remote") {
          if (args.host) config["host"] = args.host
          if (args.port) config["port"] = args.port
          if (args.username) config["username"] = args.username
          if (args.sshKey) {
            const keyContent = await Bun.file(args.sshKey).text().catch(() => null)
            if (keyContent) {
              config["ssh_key"] = keyContent
            }
          }
        }

        if (args.mode === "cloud") {
          if (args.provider) config["provider"] = args.provider
          if (args.region) config["region"] = args.region
          if (args.instanceType) config["instance_type"] = args.instanceType
          if (args.storageGb) config["storage_gb"] = args.storageGb
        }

        const body = {
          name: args.name,
          mode: args.mode,
          config,
          auto_start: true,
        }

        const run = await apiCall<RunDetails>("POST", "/api/v1/runs", body)

        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Run created:${UI.Style.TEXT_NORMAL} ${run.name} (${run.id.slice(0, 8)})`)
        UI.println(`  Status: ${run.status}`)
        UI.println(`  Mode: ${run.mode}`)

        if (args.attach) {
          UI.println("")
          await attachToRun(run.id)
        }
      } catch (error) {
        UI.error(`Failed to create run: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkAttachCommand = cmd({
  command: "attach <run-id>",
  describe: "attach to a running run and stream events",
  builder: (yargs: Argv) => {
    return yargs.positional("run-id", {
      describe: "Run ID (full or first 8 characters)",
      type: "string",
      demandOption: true,
    })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        await attachToRun(args["run-id"])
      } catch (error) {
        UI.error(`Failed to attach: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

async function attachToRun(runId: string): Promise<void> {
  // Get run details first
  const run = await apiCall<RunDetails>("GET", `/api/v1/runs/${runId}`)

  UI.println(`${UI.Style.TEXT_NORMAL_BOLD}Attaching to run:${UI.Style.TEXT_NORMAL} ${run.name} (${run.id.slice(0, 8)})`)
  UI.println(`Status: ${run.status}`)
  UI.println("")
  UI.println(`${UI.Style.TEXT_DIM}Press Ctrl+C to detach (run will continue in background)${UI.Style.TEXT_NORMAL}`)
  UI.println("")

  // Stream events via SSE
  const url = `${API_BASE}/api/v1/runs/${runId}/events/stream`
  const token = legacyEnv("ALLTERNIT_API_TOKEN", "Allternit_API_TOKEN")

  const headers: Record<string, string> = {
    Accept: "text/event-stream",
  }
  if (token) {
    headers["Authorization"] = `Bearer ${token}`
  }

  const response = await fetch(url, { headers })

  if (!response.ok) {
    throw new Error(`Failed to start event stream: ${response.status}`)
  }

  if (!response.body) {
    throw new Error("No response body")
  }

  // Read stream
  const reader = response.body.getReader()
  const decoder = new TextDecoder()

  try {
    while (true) {
      const { done, value } = await reader.read()
      if (done) break

      const text = decoder.decode(value)
      const lines = text.split("\n")

      for (const line of lines) {
        if (line.startsWith("data: ")) {
          const data = line.slice(6)
          try {
            const event = JSON.parse(data) as Event
            printEvent(event)
          } catch {
            // Ignore parse errors
          }
        }
      }
    }
  } finally {
    reader.releaseLock()
  }
}

function printEvent(event: Event): void {
  switch (event.event_type) {
    case "stdout":
    case "output": {
      const content = event.payload.content as string
      if (content) process.stdout.write(content)
      break
    }
    case "stderr": {
      const content = event.payload.content as string
      if (content) process.stderr.write(content)
      break
    }
    case "step_started": {
      const name = event.payload.step_name as string
      if (name) UI.println(`\n${UI.Style.TEXT_NORMAL_BOLD}[STEP] Starting:${UI.Style.TEXT_NORMAL} ${name}`)
      break
    }
    case "step_completed": {
      const name = event.payload.step_name as string
      if (name) UI.println(`${UI.Style.TEXT_SUCCESS}[STEP] Completed:${UI.Style.TEXT_NORMAL} ${name}`)
      break
    }
    case "step_failed": {
      const name = event.payload.step_name as string
      if (name) UI.println(`${UI.Style.TEXT_WARNING_BOLD}[STEP] Failed:${UI.Style.TEXT_NORMAL} ${name}`)
      break
    }
    case "approval_needed": {
      const title = event.payload.title as string
      if (title) UI.println(`\n${UI.Style.TEXT_WARNING}[APPROVAL NEEDED]${UI.Style.TEXT_NORMAL} ${title}`)
      break
    }
    case "approval_given":
      UI.println(`${UI.Style.TEXT_SUCCESS}[APPROVAL]${UI.Style.TEXT_NORMAL} Approved`)
      break
    case "approval_denied":
      UI.println(`${UI.Style.TEXT_WARNING_BOLD}[APPROVAL]${UI.Style.TEXT_NORMAL} Denied`)
      break
    case "run_completed":
      UI.println(`\n${UI.Style.TEXT_SUCCESS}✓ Run completed${UI.Style.TEXT_NORMAL}`)
      break
    case "run_failed":
      UI.println(`\n${UI.Style.TEXT_WARNING_BOLD}✗ Run failed${UI.Style.TEXT_NORMAL}`)
      break
  }
}

export const CoworkDetachCommand = cmd({
  command: "detach [run-id]",
  describe: "detach from a run (client-side only)",
  builder: (yargs: Argv) => {
    return yargs.positional("run-id", {
      describe: "Run ID (uses current if not specified)",
      type: "string",
    })
  },
  handler: async () => {
    // Detach is handled by the attach command itself (Ctrl+C)
    // This command is mainly for explicit detachment intent
    UI.println("Detached from run")
  },
})

export const CoworkStopCommand = cmd({
  command: "stop <run-id>",
  aliases: ["cancel", "kill"],
  describe: "stop or cancel a run",
  builder: (yargs: Argv) => {
    return yargs
      .positional("run-id", {
        describe: "Run ID to stop",
        type: "string",
        demandOption: true,
      })
      .option("force", {
        alias: "f",
        describe: "Force immediate stop",
        type: "boolean",
        default: false,
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const runId = args["run-id"]
        const shortId = runId.slice(0, 8)

        if (args.force) {
          UI.println(`Force stopping run ${shortId}...`)
        } else {
          UI.println(`Stopping run ${shortId}...`)
        }

        await apiCall("POST", `/api/v1/runs/${runId}/cancel`, {})
        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Run stopped${UI.Style.TEXT_NORMAL}`)
      } catch (error) {
        UI.error(`Failed to stop run: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkLogsCommand = cmd({
  command: "logs <run-id>",
  describe: "view logs for a run",
  builder: (yargs: Argv) => {
    return yargs
      .positional("run-id", {
        describe: "Run ID to view logs for",
        type: "string",
        demandOption: true,
      })
      .option("follow", {
        alias: "f",
        describe: "Follow log output",
        type: "boolean",
        default: false,
      })
      .option("lines", {
        alias: "n",
        describe: "Number of lines to show",
        type: "number",
        default: 100,
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const runId = args["run-id"]

        // Fetch events
        const events = await apiCall<Event[]>("GET", `/api/v1/runs/${runId}/events?limit=${args.lines}`)

        for (const event of events) {
          printEvent(event)
        }

        if (args.follow) {
          // Continue streaming
          await attachToRun(runId)
        }
      } catch (error) {
        UI.error(`Failed to fetch logs: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkShowCommand = cmd({
  command: "show <run-id>",
  aliases: ["info"],
  describe: "display detailed information about a run",
  builder: (yargs: Argv) => {
    return yargs.positional("run-id", {
      describe: "Run ID",
      type: "string",
      demandOption: true,
    })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const run = await apiCall<RunDetails>("GET", `/api/v1/runs/${args["run-id"]}`)

        UI.println(`${UI.Style.TEXT_NORMAL_BOLD}Run:${UI.Style.TEXT_NORMAL} ${run.name}`)
        UI.println(`${UI.Style.TEXT_NORMAL_BOLD}ID:${UI.Style.TEXT_NORMAL} ${run.id}`)
        UI.println(`${UI.Style.TEXT_NORMAL_BOLD}Status:${UI.Style.TEXT_NORMAL} ${run.status}`)
        UI.println(`${UI.Style.TEXT_NORMAL_BOLD}Mode:${UI.Style.TEXT_NORMAL} ${run.mode}`)

        if (run.description) {
          UI.println(`${UI.Style.TEXT_NORMAL_BOLD}Description:${UI.Style.TEXT_NORMAL} ${run.description}`)
        }

        if (run.step_cursor) {
          UI.println(`${UI.Style.TEXT_NORMAL_BOLD}Step:${UI.Style.TEXT_NORMAL} ${run.step_cursor}`)
        }

        if (run.total_steps !== undefined) {
          UI.println(`${UI.Style.TEXT_NORMAL_BOLD}Progress:${UI.Style.TEXT_NORMAL} ${run.completed_steps}/${run.total_steps}`)
        }

        UI.println(`${UI.Style.TEXT_NORMAL_BOLD}Created:${UI.Style.TEXT_NORMAL} ${new Date(run.created_at).toLocaleString()}`)

        if (run.started_at) {
          UI.println(`${UI.Style.TEXT_NORMAL_BOLD}Started:${UI.Style.TEXT_NORMAL} ${new Date(run.started_at).toLocaleString()}`)
        }

        if (run.completed_at) {
          UI.println(`${UI.Style.TEXT_NORMAL_BOLD}Completed:${UI.Style.TEXT_NORMAL} ${new Date(run.completed_at).toLocaleString()}`)
        }

        if (run.error_message) {
          UI.println(`${UI.Style.TEXT_WARNING_BOLD}Error:${UI.Style.TEXT_NORMAL} ${run.error_message}`)
        }
      } catch (error) {
        UI.error(`Failed to get run details: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkPauseCommand = cmd({
  command: "pause <run-id>",
  describe: "pause a running run",
  builder: (yargs: Argv) => {
    return yargs.positional("run-id", {
      describe: "Run ID to pause",
      type: "string",
      demandOption: true,
    })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        await apiCall("POST", `/api/v1/runs/${args["run-id"]}/pause`, {})
        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Run paused${UI.Style.TEXT_NORMAL}`)
      } catch (error) {
        UI.error(`Failed to pause run: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkResumeCommand = cmd({
  command: "resume <run-id>",
  describe: "resume a paused run",
  builder: (yargs: Argv) => {
    return yargs.positional("run-id", {
      describe: "Run ID to resume",
      type: "string",
      demandOption: true,
    })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        await apiCall("POST", `/api/v1/runs/${args["run-id"]}/resume`, {})
        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Run resumed${UI.Style.TEXT_NORMAL}`)
      } catch (error) {
        UI.error(`Failed to resume run: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

// ============================================================================
// Schedule Commands
// ============================================================================

export const CoworkScheduleCommand = cmd({
  command: "schedule",
  describe: "manage schedules",
  builder: (yargs: Argv) => {
    return yargs
      .command(CoworkScheduleListCommand)
      .command(CoworkScheduleCreateCommand)
      .command(CoworkScheduleEnableCommand)
      .command(CoworkScheduleDisableCommand)
      .command(CoworkScheduleDeleteCommand)
      .command(CoworkScheduleTriggerCommand)
      .demandCommand(1, "Choose a schedule command")
  },
  async handler() {
    // Show schedule help
    UI.println("Available schedule commands: list, create, enable, disable, delete, trigger")
    UI.println("Use --help with any command for more information")
  },
})

export const CoworkScheduleListCommand = cmd({
  command: "list",
  aliases: ["ls"],
  describe: "list all schedules",
  builder: (yargs: Argv) => {
    return yargs.option("enabled", {
      alias: "e",
      describe: "Show only enabled schedules",
      type: "boolean",
      default: false,
    })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const query = args.enabled ? "?enabled=true" : ""
        const schedules = await apiCall<ScheduleSummary[]>("GET", `/api/v1/schedules${query}`)

        if (schedules.length === 0) {
          UI.println("No schedules found")
          return
        }

        UI.println(`${UI.Style.TEXT_NORMAL_BOLD}${"ID".padEnd(10)} ${"NAME".padEnd(20)} ${"ENABLED".padEnd(8)} ${"NEXT RUN".padEnd(20)} ${"SCHEDULE"}${UI.Style.TEXT_NORMAL}`)
        UI.println("-".repeat(90))

        for (const s of schedules) {
          const id = s.id.slice(0, 8)
          const name = s.name.slice(0, 20).padEnd(20)
          const enabled = (s.enabled ? "✓" : "✗").padEnd(8)
          const nextRun = s.next_run_at
            ? new Date(s.next_run_at).toLocaleString().slice(0, 20)
            : "-".padEnd(20)
          const scheduleStr = (s.natural_lang || s.cron_expr).slice(0, 25)

          UI.println(`${id} ${name} ${enabled} ${nextRun} ${scheduleStr}`)
        }

        UI.println("")
        UI.println(`Showing ${schedules.length} schedule(s)`)
      } catch (error) {
        UI.error(`Failed to list schedules: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkScheduleCreateCommand = cmd({
  command: "create <name>",
  describe: "create a new schedule",
  builder: (yargs: Argv) => {
    return yargs
      .positional("name", {
        describe: "Schedule name",
        type: "string",
        demandOption: true,
      })
      .option("schedule", {
        alias: "s",
        describe: "Cron expression or natural language",
        type: "string",
        demandOption: true,
      })
      .option("command", {
        alias: "c",
        describe: "Command to execute",
        type: "string",
        demandOption: true,
      })
      .option("working-dir", {
        alias: "w",
        describe: "Working directory",
        type: "string",
      })
      .option("enabled", {
        alias: "e",
        describe: "Enable immediately",
        type: "boolean",
        default: true,
      })
      .option("mode", {
        alias: "m",
        describe: "Execution mode",
        type: "string",
        choices: ["local", "remote", "cloud"],
        default: "local",
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const body = {
          name: args.name,
          cron_expr: args.schedule,
          job_template: {
            command: args.command,
            working_dir: args.workingDir,
          },
          enabled: args.enabled,
          mode: args.mode,
        }

        const schedule = await apiCall<ScheduleSummary>("POST", "/api/v1/schedules", body)
        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Schedule created:${UI.Style.TEXT_NORMAL} ${schedule.name} (${schedule.id.slice(0, 8)})`)
      } catch (error) {
        UI.error(`Failed to create schedule: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkScheduleEnableCommand = cmd({
  command: "enable <schedule-id>",
  describe: "enable a schedule",
  builder: (yargs: Argv) => {
    return yargs.positional("schedule-id", {
      describe: "Schedule ID",
      type: "string",
      demandOption: true,
    })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        await apiCall("POST", `/api/v1/schedules/${args["schedule-id"]}/enable`, {})
        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Schedule enabled${UI.Style.TEXT_NORMAL}`)
      } catch (error) {
        UI.error(`Failed to enable schedule: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkScheduleDisableCommand = cmd({
  command: "disable <schedule-id>",
  describe: "disable a schedule",
  builder: (yargs: Argv) => {
    return yargs.positional("schedule-id", {
      describe: "Schedule ID",
      type: "string",
      demandOption: true,
    })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        await apiCall("POST", `/api/v1/schedules/${args["schedule-id"]}/disable`, {})
        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Schedule disabled${UI.Style.TEXT_NORMAL}`)
      } catch (error) {
        UI.error(`Failed to disable schedule: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkScheduleDeleteCommand = cmd({
  command: "delete <schedule-id>",
  aliases: ["rm"],
  describe: "delete a schedule",
  builder: (yargs: Argv) => {
    return yargs
      .positional("schedule-id", {
        describe: "Schedule ID",
        type: "string",
        demandOption: true,
      })
      .option("yes", {
        alias: "y",
        describe: "Skip confirmation",
        type: "boolean",
        default: false,
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        if (!args.yes) {
          const confirmed = await prompts.confirm({
            message: `Delete schedule ${args["schedule-id"].slice(0, 8)}?`,
            initialValue: false,
          })
          if (!confirmed || prompts.isCancel(confirmed)) {
            UI.println("Cancelled")
            return
          }
        }

        await apiCall("DELETE", `/api/v1/schedules/${args["schedule-id"]}`, {})
        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Schedule deleted${UI.Style.TEXT_NORMAL}`)
      } catch (error) {
        UI.error(`Failed to delete schedule: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkScheduleTriggerCommand = cmd({
  command: "trigger <schedule-id>",
  describe: "manually trigger a schedule",
  builder: (yargs: Argv) => {
    return yargs.positional("schedule-id", {
      describe: "Schedule ID",
      type: "string",
      demandOption: true,
    })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        await apiCall("POST", `/api/v1/schedules/${args["schedule-id"]}/trigger`, {})
        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Schedule triggered${UI.Style.TEXT_NORMAL}`)
      } catch (error) {
        UI.error(`Failed to trigger schedule: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

// ============================================================================
// Approval Commands
// ============================================================================

export const CoworkApprovalCommand = cmd({
  command: "approval",
  describe: "manage approval requests",
  builder: (yargs: Argv) => {
    return yargs
      .command(CoworkApprovalListCommand)
      .command(CoworkApprovalShowCommand)
      .command(CoworkApprovalApproveCommand)
      .command(CoworkApprovalDenyCommand)
      .demandCommand(1, "Choose an approval command")
  },
  async handler() {
    UI.println("Available approval commands: list, show, approve, deny")
    UI.println("Use --help with any command for more information")
  },
})

export const CoworkApprovalListCommand = cmd({
  command: "list",
  aliases: ["ls"],
  describe: "list approval requests",
  builder: (yargs: Argv) => {
    return yargs
      .option("run-id", {
        alias: "r",
        describe: "Filter by run ID",
        type: "string",
      })
      .option("all", {
        alias: "a",
        describe: "Show all statuses",
        type: "boolean",
        default: false,
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const params = new URLSearchParams()
        if (args.runId) params.append("run_id", args.runId)
        if (!args.all) params.append("status", "pending")

        const query = params.toString() ? `?${params.toString()}` : ""
        const approvals = await apiCall<ApprovalSummary[]>("GET", `/api/v1/approvals${query}`)

        if (approvals.length === 0) {
          UI.println(args.all ? "No approvals found" : "No pending approvals")
          return
        }

        UI.println(`${UI.Style.TEXT_NORMAL_BOLD}${"ID".padEnd(10)} ${"STATUS".padEnd(10)} ${"PRIORITY".padEnd(10)} ${"TITLE".padEnd(25)} ${"ACTION"}${UI.Style.TEXT_NORMAL}`)
        UI.println("-".repeat(90))

        for (const a of approvals) {
          const id = a.id.slice(0, 8)
          const status = a.status.padEnd(10)
          const priority = a.priority.padEnd(10)
          const title = a.title.slice(0, 25).padEnd(25)
          const action = a.action_type || "-"

          UI.println(`${id} ${status} ${priority} ${title} ${action}`)
        }

        UI.println("")
        UI.println(`Showing ${approvals.length} approval(s)`)
      } catch (error) {
        UI.error(`Failed to list approvals: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkApprovalShowCommand = cmd({
  command: "show <approval-id>",
  aliases: ["info"],
  describe: "show approval details",
  builder: (yargs: Argv) => {
    return yargs.positional("approval-id", {
      describe: "Approval ID",
      type: "string",
      demandOption: true,
    })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const approval = await apiCall<{
          id: string
          run_id: string
          status: string
          priority: string
          title: string
          description?: string
          action_type?: string
          reasoning?: string
          created_at: string
        }>("GET", `/api/v1/approvals/${args["approval-id"]}`)

        UI.println(`${UI.Style.TEXT_NORMAL_BOLD}Approval:${UI.Style.TEXT_NORMAL} ${approval.title}`)
        UI.println(`${UI.Style.TEXT_NORMAL_BOLD}ID:${UI.Style.TEXT_NORMAL} ${approval.id}`)
        UI.println(`${UI.Style.TEXT_NORMAL_BOLD}Run:${UI.Style.TEXT_NORMAL} ${approval.run_id}`)
        UI.println(`${UI.Style.TEXT_NORMAL_BOLD}Status:${UI.Style.TEXT_NORMAL} ${approval.status}`)
        UI.println(`${UI.Style.TEXT_NORMAL_BOLD}Priority:${UI.Style.TEXT_NORMAL} ${approval.priority}`)

        if (approval.description) {
          UI.println(`${UI.Style.TEXT_NORMAL_BOLD}Description:${UI.Style.TEXT_NORMAL} ${approval.description}`)
        }

        if (approval.action_type) {
          UI.println(`${UI.Style.TEXT_NORMAL_BOLD}Action:${UI.Style.TEXT_NORMAL} ${approval.action_type}`)
        }

        if (approval.reasoning) {
          UI.println(`${UI.Style.TEXT_NORMAL_BOLD}Reasoning:${UI.Style.TEXT_NORMAL} ${approval.reasoning}`)
        }

        UI.println(`${UI.Style.TEXT_NORMAL_BOLD}Created:${UI.Style.TEXT_NORMAL} ${new Date(approval.created_at).toLocaleString()}`)
      } catch (error) {
        UI.error(`Failed to get approval: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkApprovalApproveCommand = cmd({
  command: "approve <approval-id>",
  aliases: ["yes"],
  describe: "approve a pending request",
  builder: (yargs: Argv) => {
    return yargs
      .positional("approval-id", {
        describe: "Approval ID",
        type: "string",
        demandOption: true,
      })
      .option("message", {
        alias: "m",
        describe: "Optional approval message",
        type: "string",
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const body = args.message ? { message: args.message } : {}
        await apiCall("POST", `/api/v1/approvals/${args["approval-id"]}/approve`, body)
        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Approval granted${UI.Style.TEXT_NORMAL}`)
      } catch (error) {
        UI.error(`Failed to approve: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkApprovalDenyCommand = cmd({
  command: "deny <approval-id>",
  aliases: ["reject", "no"],
  describe: "deny a pending request",
  builder: (yargs: Argv) => {
    return yargs
      .positional("approval-id", {
        describe: "Approval ID",
        type: "string",
        demandOption: true,
      })
      .option("reason", {
        alias: "r",
        describe: "Optional reason for denial",
        type: "string",
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const body = args.reason ? { reason: args.reason } : {}
        await apiCall("POST", `/api/v1/approvals/${args["approval-id"]}/deny`, body)
        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Approval denied${UI.Style.TEXT_NORMAL}`)
      } catch (error) {
        UI.error(`Failed to deny: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

// ============================================================================
// Checkpoint Commands
// ============================================================================

export const CoworkCheckpointCommand = cmd({
  command: "checkpoint",
  aliases: ["ckpt"],
  describe: "manage checkpoints",
  builder: (yargs: Argv) => {
    return yargs
      .command(CoworkCheckpointListCommand)
      .command(CoworkCheckpointCreateCommand)
      .command(CoworkCheckpointRestoreCommand)
      .command(CoworkCheckpointDeleteCommand)
      .demandCommand(1, "Choose a checkpoint command")
  },
  async handler() {
    UI.println("Available checkpoint commands: list, create, restore, delete")
    UI.println("Use --help with any command for more information")
  },
})

export const CoworkCheckpointListCommand = cmd({
  command: "list <run-id>",
  aliases: ["ls"],
  describe: "list checkpoints for a run",
  builder: (yargs: Argv) => {
    return yargs.positional("run-id", {
      describe: "Run ID",
      type: "string",
      demandOption: true,
    })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const checkpoints = await apiCall<CheckpointSummary[]>(
          "GET",
          `/api/v1/runs/${args["run-id"]}/checkpoints`
        )

        if (checkpoints.length === 0) {
          UI.println("No checkpoints found")
          return
        }

        UI.println(`${UI.Style.TEXT_NORMAL_BOLD}${"ID".padEnd(10)} ${"NAME".padEnd(20)} ${"STEP".padEnd(20)} ${"RESUMABLE".padEnd(10)} ${"CREATED"}${UI.Style.TEXT_NORMAL}`)
        UI.println("-".repeat(90))

        for (const ckpt of checkpoints) {
          const id = ckpt.id.slice(0, 8)
          const name = (ckpt.name || "-").slice(0, 20).padEnd(20)
          const step = ckpt.step_cursor.slice(0, 20).padEnd(20)
          const resumable = (ckpt.resumable ? "✓" : "✗").padEnd(10)
          const created = new Date(ckpt.created_at).toLocaleString()

          UI.println(`${id} ${name} ${step} ${resumable} ${created}`)
        }

        UI.println("")
        UI.println(`Showing ${checkpoints.length} checkpoint(s)`)
      } catch (error) {
        UI.error(`Failed to list checkpoints: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkCheckpointCreateCommand = cmd({
  command: "create <run-id>",
  describe: "create a checkpoint",
  builder: (yargs: Argv) => {
    return yargs
      .positional("run-id", {
        describe: "Run ID",
        type: "string",
        demandOption: true,
      })
      .option("name", {
        alias: "n",
        describe: "Checkpoint name",
        type: "string",
      })
      .option("description", {
        alias: "d",
        describe: "Description",
        type: "string",
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const body = {
          name: args.name,
          description: args.description,
        }

        const checkpoint = await apiCall<CheckpointSummary>(
          "POST",
          `/api/v1/runs/${args["run-id"]}/checkpoints`,
          body
        )

        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Checkpoint created:${UI.Style.TEXT_NORMAL} ${checkpoint.id.slice(0, 8)}`)
      } catch (error) {
        UI.error(`Failed to create checkpoint: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkCheckpointRestoreCommand = cmd({
  command: "restore <run-id> <checkpoint-id>",
  describe: "restore a run from a checkpoint",
  builder: (yargs: Argv) => {
    return yargs
      .positional("run-id", {
        describe: "Run ID",
        type: "string",
        demandOption: true,
      })
      .positional("checkpoint-id", {
        describe: "Checkpoint ID",
        type: "string",
        demandOption: true,
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        await apiCall("POST", `/api/v1/runs/${args["run-id"]}/restore`, {
          checkpoint_id: args["checkpoint-id"],
        })
        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Run restored from checkpoint${UI.Style.TEXT_NORMAL}`)
      } catch (error) {
        UI.error(`Failed to restore checkpoint: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkCheckpointDeleteCommand = cmd({
  command: "delete <checkpoint-id>",
  aliases: ["rm"],
  describe: "delete a checkpoint",
  builder: (yargs: Argv) => {
    return yargs
      .positional("checkpoint-id", {
        describe: "Checkpoint ID",
        type: "string",
        demandOption: true,
      })
      .option("yes", {
        alias: "y",
        describe: "Skip confirmation",
        type: "boolean",
        default: false,
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        if (!args.yes) {
          const confirmed = await prompts.confirm({
            message: `Delete checkpoint ${args["checkpoint-id"].slice(0, 8)}?`,
            initialValue: false,
          })
          if (!confirmed || prompts.isCancel(confirmed)) {
            UI.println("Cancelled")
            return
          }
        }

        await apiCall("DELETE", `/api/v1/checkpoints/${args["checkpoint-id"]}`, {})
        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Checkpoint deleted${UI.Style.TEXT_NORMAL}`)
      } catch (error) {
        UI.error(`Failed to delete checkpoint: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

// ============================================================================
// Web Mirror Command (Session Mirroring)
// ============================================================================

export const CoworkWebCommand = cmd({
  command: "web [run-id]",
  describe: "mirror current terminal session to web browser (Kimi /web equivalent)",
  builder: (yargs: Argv) => {
    return yargs
      .positional("run-id", {
        describe: "Run ID to mirror (defaults to current/active run)",
        type: "string",
      })
      .option("port", {
        alias: "p",
        describe: "Local port for Cowork controller (default: 3010)",
        type: "number",
        default: 3010,
      })
      .option("open", {
        alias: "o",
        describe: "Automatically open browser",
        type: "boolean",
        default: true,
      })
      .option("qr", {
        describe: "Display QR code for mobile access",
        type: "boolean",
        default: true,
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        // 1. Resolve run ID (get current/active if not provided)
        let runId = args["run-id"]
        if (!runId) {
          // Get most recent running run
          const runs = await apiCall<RunSummary[]>("GET", "/api/v1/runs?status=running&limit=1")
          if (runs.length === 0) {
            UI.error("No active run found. Start a run with 'gizzi workspace tasks start' or provide a run-id.")
            process.exit(1)
          }
          runId = runs[0].id
        }

        // 2. Get run details
        const run = await apiCall<RunDetails>("GET", `/api/v1/runs/${runId}`)

        // 3. Request mirror session from Cowork controller
        const mirrorResponse = await apiCall<{
          mirror_url: string
          session_token: string
          expires_at: string
        }>("POST", `/api/v1/runs/${runId}/mirror`, {
          port: args.port,
        })

        const mirrorUrl = mirrorResponse.mirror_url

        // 4. Display mirror information
        UI.println("")
        UI.println(UI.Style.TEXT_NORMAL_BOLD + "🔮 Session Mirroring Enabled" + UI.Style.TEXT_NORMAL)
        UI.println("")
        UI.println(`${UI.Style.TEXT_INFO_BOLD}Run:${UI.Style.TEXT_NORMAL} ${run.name} (${run.id.slice(0, 8)})`)
        UI.println(`${UI.Style.TEXT_INFO_BOLD}Status:${UI.Style.TEXT_NORMAL} ${run.status}`)
        UI.println("")
        UI.println(`${UI.Style.TEXT_SUCCESS_BOLD}Web URL:${UI.Style.TEXT_NORMAL} ${mirrorUrl}`)
        UI.println("")

        // 5. Display QR code if requested (ASCII representation)
        if (args.qr) {
          UI.println("Scan to open on your device:")
          UI.println("")
          // Simple ASCII QR representation or use qrcode-terminal if available
          const qr = generateSimpleQR(mirrorUrl)
          UI.println(qr)
          UI.println("")
        }

        // 6. Open browser if requested
        if (args.open) {
          await open(mirrorUrl)
          UI.println(`${UI.Style.TEXT_DIM}Opening browser...${UI.Style.TEXT_NORMAL}`)
        }

        // 7. Show instructions
        UI.println(`${UI.Style.TEXT_DIM}Press Ctrl+C to stop mirroring (run will continue)${UI.Style.TEXT_NORMAL}`)
        UI.println("")

        // 8. Keep alive and stream events
        await streamMirrorEvents(runId, args.port)
      } catch (error) {
        UI.error(`Failed to start web mirror: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

// ============================================================================
// Tasks Commands
// ============================================================================

export const CoworkTasksCommand = cmd({
  command: "tasks",
  aliases: ["t"],
  describe: "view and optimize cowork tasks (IntelliSchedule TUI)",
  builder: (yargs: Argv) => {
    return yargs
      .option("workspace", {
        alias: "w",
        describe: "Workspace ID",
        type: "string",
        demandOption: true,
      })
      .command(CoworkTasksListCommand)
      .command(CoworkTasksCreateCommand)
      .command(CoworkTasksReadCommand)
      .command(CoworkTasksEditCommand)
      .command(CoworkTasksMoveCommand)
      .command(CoworkTasksAssignCommand)
      .command(CoworkTasksDeleteCommand)
      .command(CoworkTasksCommentCommand)
      .command(CoworkTasksOptimizeCommand)
  },
  handler: async (args) => {
    try {
      const resp = await apiCall<any>("GET", `/api/v1/tasks?workspace_id=${args.workspace}&limit=100`)
      const tasks = (Array.isArray(resp) ? resp : resp?.tasks) || []

      if (tasks.length === 0) {
        UI.println(UI.Style.TEXT_WARNING + "No tasks found in workspace." + UI.Style.TEXT_NORMAL)
        UI.println("Create tasks in the Allternit platform or run:")
        UI.println("  gizzi workspace tasks tasks create \"<title>\" --workspace=" + args.workspace)
        return
      }

      const commentsMap: Record<string, any[]> = {};
      await Promise.all(
        tasks.map(async (t: any) => {
          try {
            const comments = await apiCall<any[]>("GET", `/api/v1/tasks/${t.id}/comments`);
            commentsMap[t.id] = comments.map(c => ({
              id: c.id || c.comment_id || String(Math.random()),
              author: c.author_name || c.author_id || "Unknown",
              body: c.body,
              createdAt: c.created_at
            }));
          } catch {
            commentsMap[t.id] = [];
          }
        })
      );

      const { unmount } = await render(
        React.createElement(IntelliTaskScreen, {
          tasks: tasks.map(t => ({
            id: t.id,
            title: t.title,
            priority: Number(t.priority) || 1,
            estimatedMinutes: t.estimated_minutes || t.estimatedMinutes || 60,
            deadline: t.deadline ? new Date(t.deadline).getTime() : undefined,
            dependencies: t.dependencies || [],
            status: t.status || 'todo',
            assignee_id: t.assignee_id || undefined,
            assignee_name: t.assignee_name || undefined,
            assignee_type: t.assignee_type || undefined,
          })),
          comments: commentsMap,
          onSelect: (taskId: string) => {
            UI.println(`${UI.Style.TEXT_INFO_BOLD}Selected task:${UI.Style.TEXT_NORMAL} ${taskId}`)
          },
          onQuit: () => {
            unmount()
            process.exit(0)
          },
          onStatusChange: (taskId: string, status: string) => {
            apiCall("PUT", `/api/v1/tasks/${taskId}`, { status })
              .catch(err => {
                UI.error(`Failed to update task status: ${err instanceof Error ? err.message : String(err)}`)
              });
          },
          onAssign: async (taskId: string, currentAssigneeId?: string) => {
            try {
              const me = await apiCall<any>("GET", "/api/v1/me");
              const isAssignedToMe = currentAssigneeId === me.id;

              const body = isAssignedToMe ? {
                assignee_type: null,
                assignee_id: null,
                assignee_name: null,
              } : {
                assignee_type: "human",
                assignee_id: me.id,
                assignee_name: me.name || me.email || "You",
              };

              await apiCall("POST", `/api/v1/tasks/${taskId}/assign`, body);
              return {
                assignee_id: body.assignee_id,
                assignee_name: body.assignee_name,
              };
            } catch (err) {
              UI.error(`Failed to assign task: ${err instanceof Error ? err.message : String(err)}`);
              return null;
            }
          },
          onAddComment: async (taskId: string, body: string) => {
            try {
              await apiCall("POST", `/api/v1/tasks/${taskId}/comments`, { body });
              const updated = await apiCall<any[]>("GET", `/api/v1/tasks/${taskId}/comments`);
              return updated.map(c => ({
                id: c.id || c.comment_id || String(Math.random()),
                author: c.author_name || c.author_id || "Unknown",
                body: c.body,
                createdAt: c.created_at
              }));
            } catch (err) {
              UI.error(`Failed to add comment: ${err instanceof Error ? err.message : String(err)}`);
              return null;
            }
          },
        })
      )
    } catch (error) {
      UI.error(`Failed to load tasks: ${error instanceof Error ? error.message : String(error)}`)
      process.exit(1)
    }
  },
})

export const CoworkTasksListCommand = cmd({
  command: "list",
  aliases: ["ls"],
  describe: "list tasks",
  builder: (yargs: Argv) => {
    return yargs
      .option("workspace", {
        alias: "w",
        describe: "Workspace ID",
        type: "string",
        demandOption: true,
      })
      .option("status", {
        alias: "s",
        describe: "Filter by status (backlog,todo,in-progress,in-review,done)",
        type: "string",
      })
      .option("limit", {
        alias: "l",
        describe: "Maximum results",
        type: "number",
        default: 50,
      })
      .option("format", {
        describe: "Output format",
        type: "string",
        choices: ["table", "json"],
        default: "table",
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const params = new URLSearchParams()
        params.append("workspace_id", args.workspace)
        if (args.status) params.append("status", args.status)
        params.append("limit", String(args.limit))

        const query = params.toString() ? `?${params.toString()}` : ""
        const resp = await apiCall<any>("GET", `/api/v1/tasks${query}`)
        const tasks = (Array.isArray(resp) ? resp : resp?.tasks) || []

        if (args.format === "json") {
          UI.println(JSON.stringify(tasks, null, 2))
          return
        }

        if (tasks.length === 0) {
          UI.println("No tasks found")
          return
        }

        UI.println(`${UI.Style.TEXT_NORMAL_BOLD}${"ID".padEnd(10)} ${"TITLE".padEnd(25)} ${"STATUS".padEnd(12)} ${"PRI".padEnd(4)} ${"EST".padEnd(6)} ${"ASSIGNEE"}${UI.Style.TEXT_NORMAL}`)
        UI.println("-".repeat(90))

        for (const task of tasks) {
          const id = task.id.slice(0, 8)
          const title = task.title.slice(0, 25).padEnd(25)
          const status = task.status.padEnd(12)
          const priority = String(task.priority).padEnd(4)
          const estimate = task.estimatedMinutes || task.estimated_minutes ? `${task.estimatedMinutes || task.estimated_minutes}m`.padEnd(6) : "-".padEnd(6)
          const assignee = task.assignee_name || task.assignee_id || "-"

          UI.println(`${id} ${title} ${status} ${priority} ${estimate} ${assignee}`)
        }

        UI.println("")
        UI.println(`Showing ${tasks.length} task(s)`)
      } catch (error) {
        UI.error(`Failed to list tasks: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkTasksCreateCommand = cmd({
  command: "create <title>",
  aliases: ["new"],
  describe: "create a new task in a workspace",
  builder: (yargs: Argv) => {
    return yargs
      .positional("title", {
        describe: "Task title",
        type: "string",
        demandOption: true,
      })
      .option("workspace", {
        alias: "w",
        describe: "Workspace ID",
        type: "string",
        demandOption: true,
      })
      .option("description", {
        alias: "d",
        describe: "Task description",
        type: "string",
      })
      .option("priority", {
        alias: "p",
        describe: "Priority (0-100)",
        type: "number",
        default: 50,
      })
      .option("estimate", {
        alias: "e",
        describe: "Estimated minutes",
        type: "number",
      })
      .option("deadline", {
        describe: "Deadline (ISO 8601)",
        type: "string",
      })
      .option("status", {
        alias: "s",
        describe: "Initial status",
        type: "string",
        choices: ["backlog", "todo", "in-progress", "in-review", "done"],
        default: "backlog",
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const body: Record<string, unknown> = {
          title: args.title,
          workspace_id: args.workspace,
          description: args.description,
          priority: args.priority,
          status: args.status,
        }
        if (args.estimate !== undefined) {
          body.estimated_minutes = args.estimate
        }
        if (args.deadline !== undefined) {
          body.deadline = args.deadline
        }

        const resp = await apiCall<any>("POST", "/api/v1/tasks", body)
        const task = resp?.task || resp
        UI.println(UI.Style.TEXT_SUCCESS_BOLD + "✓ Task created" + UI.Style.TEXT_NORMAL)
        UI.println(`  ID: ${task.id}`)
        UI.println(`  Title: ${task.title}`)
      } catch (error) {
        UI.error(`Failed to create task: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkTasksEditCommand = cmd({
  command: "edit <id>",
  describe: "edit a task",
  builder: (yargs: Argv) => {
    return yargs
      .positional("id", {
        describe: "Task ID",
        type: "string",
        demandOption: true,
      })
      .option("workspace", {
        alias: "w",
        describe: "Workspace ID",
        type: "string",
        demandOption: true,
      })
      .option("estimate", {
        alias: "e",
        describe: "Estimated minutes",
        type: "number",
      })
      .option("priority", {
        alias: "p",
        describe: "Priority (0-100)",
        type: "number",
      })
      .option("status", {
        alias: "s",
        describe: "Status",
        type: "string",
        choices: ["backlog", "todo", "in-progress", "in-review", "done"],
      })
      .option("title", {
        alias: "t",
        describe: "Title",
        type: "string",
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const updates: Record<string, unknown> = {}
        if (args.estimate !== undefined) updates.estimated_minutes = args.estimate
        if (args.priority !== undefined) updates.priority = args.priority
        if (args.status) updates.status = args.status
        if (args.title) updates.title = args.title

        const resp = await apiCall<any>("PUT", `/api/v1/tasks/${args.id}`, updates)
        const task = resp?.task || resp
        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Updated task:${UI.Style.TEXT_NORMAL} ${task.id}`)
      } catch (error) {
        UI.error(`Failed to update task: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkTasksMoveCommand = cmd({
  command: "move <id>",
  describe: "move a task to a new status",
  builder: (yargs: Argv) => {
    return yargs
      .positional("id", {
        describe: "Task ID",
        type: "string",
        demandOption: true,
      })
      .option("workspace", {
        alias: "w",
        describe: "Workspace ID",
        type: "string",
        demandOption: true,
      })
      .option("status", {
        alias: "s",
        describe: "New status",
        type: "string",
        demandOption: true,
        choices: ["backlog", "todo", "in-progress", "in-review", "done"],
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const resp = await apiCall<any>("PUT", `/api/v1/tasks/${args.id}`, { status: args.status })
        const task = resp?.task || resp
        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Moved task:${UI.Style.TEXT_NORMAL} ${task.id} → ${task.status}`)
      } catch (error) {
        UI.error(`Failed to move task: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkTasksAssignCommand = cmd({
  command: "assign <id>",
  describe: "assign a task to an agent or user",
  builder: (yargs: Argv) => {
    return yargs
      .positional("id", {
        describe: "Task ID",
        type: "string",
        demandOption: true,
      })
      .option("workspace", {
        alias: "w",
        describe: "Workspace ID",
        type: "string",
        demandOption: true,
      })
      .option("agent", {
        alias: "a",
        describe: "Agent ID to assign",
        type: "string",
      })
      .option("user", {
        alias: "u",
        describe: "User ID to assign",
        type: "string",
      })
      .option("name", {
        alias: "n",
        describe: "Assignee display name",
        type: "string",
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        if (args.agent) {
          const body = {
            assignee_type: "agent",
            assignee_id: args.agent,
            assignee_name: args.name || args.agent,
          }
          const task = await apiCall<TaskItem>("POST", `/api/v1/tasks/${args.id}/assign`, body)
          UI.println(`${UI.Style.TEXT_SUCCESS}✓ Assigned task:${UI.Style.TEXT_NORMAL} ${task.id}`)
        } else if (args.user) {
          const body = {
            assignee_type: "human",
            assignee_id: args.user,
            assignee_name: args.name || args.user,
          }
          const task = await apiCall<TaskItem>("POST", `/api/v1/tasks/${args.id}/assign`, body)
          UI.println(`${UI.Style.TEXT_SUCCESS}✓ Assigned task:${UI.Style.TEXT_NORMAL} ${task.id}`)
        } else {
          const body = {
            assignee_type: null,
            assignee_id: null,
            assignee_name: null,
          }
          const task = await apiCall<TaskItem>("POST", `/api/v1/tasks/${args.id}/assign`, body)
          UI.println(`${UI.Style.TEXT_SUCCESS}✓ Unassigned task:${UI.Style.TEXT_NORMAL} ${task.id}`)
        }
      } catch (error) {
        UI.error(`Failed to assign task: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkTasksDeleteCommand = cmd({
  command: "delete <id>",
  aliases: ["rm"],
  describe: "delete a task",
  builder: (yargs: Argv) => {
    return yargs
      .positional("id", {
        describe: "Task ID",
        type: "string",
        demandOption: true,
      })
      .option("workspace", {
        alias: "w",
        describe: "Workspace ID",
        type: "string",
        demandOption: true,
      })
      .option("yes", {
        alias: "y",
        describe: "Skip confirmation",
        type: "boolean",
        default: false,
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        if (!args.yes) {
          const confirmed = await prompts.confirm({
            message: `Delete task ${args.id.slice(0, 8)}?`,
            initialValue: false,
          })
          if (!confirmed || prompts.isCancel(confirmed)) {
            UI.println("Cancelled")
            return
          }
        }

        await apiCall("DELETE", `/api/v1/tasks/${args.id}`, {})
        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Task deleted${UI.Style.TEXT_NORMAL}`)
      } catch (error) {
        UI.error(`Failed to delete task: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkTasksCommentCommand = cmd({
  command: "comment <id> <body>",
  describe: "add a comment to a task",
  builder: (yargs: Argv) => {
    return yargs
      .positional("id", {
        describe: "Task ID",
        type: "string",
        demandOption: true,
      })
      .positional("body", {
        describe: "Comment body",
        type: "string",
        demandOption: true,
      })
      .option("workspace", {
        alias: "w",
        describe: "Workspace ID",
        type: "string",
        demandOption: true,
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const comment = await apiCall<Comment>("POST", `/api/v1/tasks/${args.id}/comments`, {
          author: process.env.USER || "cli-user",
          body: args.body,
        })
        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Added comment:${UI.Style.TEXT_NORMAL} ${comment.id}`)
      } catch (error) {
        UI.error(`Failed to add comment: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkTasksReadCommand = cmd({
  command: "read <id>",
  aliases: ["show", "get", "info"],
  describe: "read task details and comments",
  builder: (yargs: Argv) => {
    return yargs
      .positional("id", {
        describe: "Task ID",
        type: "string",
        demandOption: true,
      })
      .option("workspace", {
        alias: "w",
        describe: "Workspace ID",
        type: "string",
        demandOption: true,
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const task = await apiCall<any>("GET", `/api/v1/tasks/${args.id}`)
        let comments: any[] = []
        try {
          comments = await apiCall<any[]>("GET", `/api/v1/tasks/${args.id}/comments`)
        } catch {
          // comments offline
        }
        UI.println(UI.Style.TEXT_NORMAL_BOLD + "Task Details" + UI.Style.TEXT_NORMAL)
        UI.println(`  ID:          ${task.id}`)
        UI.println(`  Title:       ${task.title}`)
        UI.println(`  Description: ${task.description || "N/A"}`)
        UI.println(`  Status:      ${task.status}`)
        UI.println(`  Priority:    ${task.priority || "N/A"}`)
        UI.println(`  Estimate:    ${task.estimated_minutes || task.estimatedMinutes || "N/A"} minutes`)
        UI.println(`  Deadline:    ${task.deadline || "N/A"}`)
        UI.println(`  Assignee:    ${task.assignee_name ? `${task.assignee_name} (${task.assignee_type}: ${task.assignee_id})` : "Unassigned"}`)
        UI.println("")
        UI.println(UI.Style.TEXT_NORMAL_BOLD + `Comments (${comments.length})` + UI.Style.TEXT_NORMAL)
        if (comments.length === 0) {
          UI.println("  No comments.")
        } else {
          for (const c of comments) {
            UI.println(`  - [${new Date(c.created_at || c.createdAt).toLocaleString()}] ${c.author_name || c.author_id || "Unknown"}: ${c.body}`)
          }
        }
      } catch (error) {
        UI.error(`Failed to read task: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

// ============================================================================
// Tasks Optimize Command
// ============================================================================

export const CoworkTasksOptimizeCommand = cmd({
  command: "optimize",
  aliases: ["opt"],
  describe: "optimize task ordering for a workspace (IntelliSchedule)",
  builder: (yargs: Argv) => {
    return yargs
      .option("workspace", {
        alias: "w",
        describe: "Workspace ID",
        type: "string",
        demandOption: true,
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const resp = await apiCall<any>("POST", `/api/v1/tasks/optimize?workspace_id=${args.workspace}`)
        const tasks = (Array.isArray(resp) ? resp : resp?.tasks) || []
        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Optimized ${tasks.length} tasks for workspace ${args.workspace}${UI.Style.TEXT_NORMAL}`)
        for (const t of tasks.slice(0, 10)) {
          UI.println(`  #${t.optimize_rank} ${t.title} (priority: ${t.priority})`)
        }
        if (tasks.length > 10) {
          UI.println(`  ... and ${tasks.length - 10} more`)
        }
      } catch (error) {
        UI.error(`Failed to optimize tasks: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

// ============================================================================
// Queue Commands
// ============================================================================

export const CoworkQueueCommand = cmd({
  command: "queue",
  aliases: ["q"],
  describe: "manage agent task queue",
  builder: (yargs: Argv) => {
    return yargs
      .command(CoworkQueueListCommand)
      .command(CoworkQueueClaimCommand)
      .command(CoworkQueueCompleteCommand)
  },
  handler: async () => {
    UI.println(UI.Style.TEXT_NORMAL_BOLD + "Agent Task Queue" + UI.Style.TEXT_NORMAL)
    UI.println("")
    UI.println("Subcommands:")
    UI.println("  list      List queue items")
    UI.println("  claim     Claim the next pending item")
    UI.println("  complete  Mark a queue item as completed/failed")
  },
})

export const CoworkQueueListCommand = cmd({
  command: "list",
  aliases: ["ls"],
  describe: "list queue items",
  builder: (yargs: Argv) => {
    return yargs
      .option("workspace", {
        alias: "w",
        describe: "Workspace ID",
        type: "string",
      })
      .option("status", {
        alias: "s",
        describe: "Filter by status (pending, claimed, running, completed, failed)",
        type: "string",
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const query = new URLSearchParams()
        if (args.workspace) query.set("workspace_id", args.workspace)
        if (args.status) query.set("status", args.status)
        const qs = query.toString()
        const items = await apiCall<Array<{
          id: string
          task_id: string
          status: string
          agent_id?: string
          agent_role?: string
          created_at: string
        }>>("GET", `/api/v1/queue${qs ? "?" + qs : ""}`)

        if (items.length === 0) {
          UI.println("No queue items found")
          return
        }

        UI.println(UI.Style.TEXT_NORMAL_BOLD + "Queue Items" + UI.Style.TEXT_NORMAL)
        UI.println("")
        for (const item of items) {
          const statusColor =
            item.status === "pending"
              ? UI.Style.TEXT_WARNING
              : item.status === "completed"
                ? UI.Style.TEXT_SUCCESS
                : item.status === "failed"
                  ? UI.Style.TEXT_ERROR
                  : UI.Style.TEXT_INFO
          UI.println(
            `  ${statusColor}${item.status.padEnd(12)}${UI.Style.TEXT_NORMAL} ${item.task_id.slice(0, 8)}  ${item.agent_id ? `agent: ${item.agent_id}` : "unclaimed"}`
          )
        }
      } catch (error) {
        UI.error(`Failed to list queue: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkQueueClaimCommand = cmd({
  command: "claim",
  describe: "claim the next pending queue item",
  builder: (yargs: Argv) => {
    return yargs
      .option("agent-id", {
        alias: "a",
        describe: "Agent ID",
        type: "string",
        demandOption: true,
      })
      .option("agent-role", {
        alias: "r",
        describe: "Agent role",
        type: "string",
      })
      .option("workspace", {
        alias: "w",
        describe: "Workspace ID",
        type: "string",
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const item = await apiCall<{
          id: string
          task_id: string
          status: string
        } | null>("POST", "/api/v1/queue/claim", {
          agent_id: args["agent-id"],
          agent_role: args["agent-role"] || null,
          workspace_id: args.workspace || null,
        })

        if (!item) {
          UI.println(UI.Style.TEXT_WARNING + "No pending queue items available" + UI.Style.TEXT_NORMAL)
          return
        }

        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Claimed queue item:${UI.Style.TEXT_NORMAL} ${item.id}`)
        UI.println(`  Task: ${item.task_id}`)
        UI.println(`  Status: ${item.status}`)
      } catch (error) {
        UI.error(`Failed to claim queue item: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

export const CoworkQueueCompleteCommand = cmd({
  command: "complete <id>",
  describe: "mark a queue item as completed or failed",
  builder: (yargs: Argv) => {
    return yargs
      .positional("id", {
        describe: "Queue item ID",
        type: "string",
        demandOption: true,
      })
      .option("result", {
        describe: "Result JSON string",
        type: "string",
      })
      .option("error", {
        describe: "Error message (if failed)",
        type: "string",
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      try {
        const item = await apiCall<{
          id: string
          status: string
        }>("POST", `/api/v1/queue/${args.id}/complete`, {
          result: args.result || null,
          error: args.error || null,
        })

        UI.println(`${UI.Style.TEXT_SUCCESS}✓ Queue item ${item.status}:${UI.Style.TEXT_NORMAL} ${item.id}`)
      } catch (error) {
        UI.error(`Failed to complete queue item: ${error instanceof Error ? error.message : String(error)}`)
        process.exit(1)
      }
    })
  },
})

/**
 * Generate a simple ASCII representation of content for terminal display
 * In production, this could use a proper QR code library
 */
function generateSimpleQR(url: string): string {
  // For now, return a bordered box with the URL
  // In production, integrate with qrcode-terminal or similar
  const width = Math.min(url.length + 4, 50)
  const line = "█".repeat(width)
  const padding = " ".repeat(width - 2)
  
  const result = []
  result.push(line)
  result.push("█" + padding + "█")
  
  // Split URL into chunks if too long
  const chunks = url.match(/.{1,46}/g) || [url]
  for (const chunk of chunks) {
    const padded = chunk.padEnd(width - 2, " ")
    result.push("█ " + padded + " █")
  }
  
  result.push("█" + padding + "█")
  result.push(line)
  
  return result.join("\n")
}

/**
 * Stream events to keep the mirror connection alive
 */
async function streamMirrorEvents(runId: string, port: number): Promise<void> {
  const token = legacyEnv("ALLTERNIT_API_TOKEN", "Allternit_API_TOKEN")
  const url = `${API_BASE}/api/v1/runs/${runId}/events/stream`

  const headers: Record<string, string> = {
    Accept: "text/event-stream",
  }
  if (token) {
    headers["Authorization"] = `Bearer ${token}`
  }

  try {
    const response = await fetch(url, { headers })

    if (!response.ok) {
      throw new Error(`Failed to start event stream: ${response.status}`)
    }

    if (!response.body) {
      throw new Error("No response body")
    }

    const reader = response.body.getReader()
    const decoder = new TextDecoder()

    while (true) {
      const { done, value } = await reader.read()
      if (done) break

      const text = decoder.decode(value)
      const lines = text.split("\n")

      for (const line of lines) {
        if (line.startsWith("data: ")) {
          try {
            const event = JSON.parse(line.slice(6))
            // Events are being streamed to the web client via Cowork controller
            // We just keep the connection alive here
            if (event.event_type === "mirror" && event.payload?.status === "disconnected") {
              UI.println(`${UI.Style.TEXT_WARNING}Mirror client disconnected${UI.Style.TEXT_NORMAL}`)
            }
          } catch {
            // Ignore parse errors
          }
        }
      }
    }
  } catch (error) {
    UI.error(`Event stream error: ${error instanceof Error ? error.message : String(error)}`)
  }
}

// ============================================================================
// NEW: Multica / Taskdog Integration Commands
// ============================================================================

export const CoworkBoardCommand = cmd({
  command: "board",
  describe: "open board view (simplified list)",
  builder: (yargs: Argv) => yargs,
  handler: async () => {
    UI.println(UI.Style.TEXT_NORMAL_BOLD + "Board View" + UI.Style.TEXT_NORMAL)
    UI.println("Use the Allternit platform console drawer for full Kanban board.")
    UI.println("")
    UI.println("Keyboard shortcuts in TUI:")
    UI.println("  j/k    navigate tasks")
    UI.println("  o      optimize schedule")
    UI.println("  Enter  select task")
  },
})

export const CoworkOptimizeCommand = cmd({
  command: "optimize",
  describe: "run schedule optimization with chosen strategy",
  builder: (yargs: Argv) => {
    return yargs.option("strategy", {
      type: "string",
      choices: ["greedy", "balanced", "priority-first", "earliest-deadline", "genetic", "monte-carlo"],
      default: "greedy",
      describe: "optimization strategy",
    })
  },
  handler: async (args) => {
    UI.println(UI.Style.TEXT_NORMAL_BOLD + `Optimizing with strategy: ${args.strategy}` + UI.Style.TEXT_NORMAL)
    UI.println("Connect to the Allternit platform for full optimization UI.")
  },
})

export const CoworkTimerCommand = cmd({
  command: "timer <action>",
  describe: "start or stop time tracking for a task",
  builder: (yargs: Argv) => {
    return yargs
      .positional("action", {
        type: "string",
        choices: ["start", "stop"],
        describe: "timer action",
      })
      .option("task-id", {
        type: "string",
        describe: "task identifier",
      })
  },
  handler: async (args) => {
    const action = args.action as string
    const taskId = args["task-id"] as string
    if (!taskId) {
      UI.error("--task-id is required")
      return
    }
    UI.println(`${action === "start" ? "Started" : "Stopped"} timer for task ${taskId}`)
  },
})

export const CoworkRuntimeCommand = cmd({
  command: "runtime <action>",
  describe: "list or register agent runtimes",
  builder: (yargs: Argv) => {
    return yargs.positional("action", {
      type: "string",
      choices: ["list", "register"],
      describe: "runtime action",
    })
  },
  handler: async (args) => {
    const action = args.action as string
    if (action === "list") {
      UI.println(UI.Style.TEXT_NORMAL_BOLD + "Discovered Runtimes" + UI.Style.TEXT_NORMAL)
      UI.println("Use the Allternit platform runtime dashboard for full details.")
    } else {
      UI.println("Registering local runtime...")
      UI.println("Use the Allternit platform to register this machine.")
    }
  },
})

export const CoworkWorkspaceCommand = cmd({
  command: "workspace <action>",
  describe: "list or switch workspaces",
  builder: (yargs: Argv) => {
    return yargs.positional("action", {
      type: "string",
      choices: ["list", "switch"],
      describe: "workspace action",
    })
  },
  handler: async (args) => {
    const action = args.action as string
    if (action === "list") {
      UI.println(UI.Style.TEXT_NORMAL_BOLD + "Workspaces" + UI.Style.TEXT_NORMAL)
      UI.println("Use the Allternit platform board drawer to manage workspaces.")
    } else {
      UI.println("Switching workspace...")
      UI.println("Use the Allternit platform board drawer to switch workspaces.")
    }
  },
})

export const CoworkRunDaemonCommand = cmd({
  command: "run",
  describe: "run the agent cowork worker daemon loop",
  builder: (yargs: Argv) => {
    return yargs
      .option("agent-id", {
        alias: "a",
        describe: "Agent ID to run as",
        type: "string",
        default: "gizzi-agent-1",
      })
      .option("agent-role", {
        alias: "r",
        describe: "Agent role",
        type: "string",
        default: "cowork",
      })
      .option("workspace", {
        alias: "w",
        describe: "Workspace ID to filter tasks",
        type: "string",
      })
      .option("interval", {
        alias: "i",
        describe: "Polling interval in seconds",
        type: "number",
        default: 5,
      })
  },
  handler: async (args) => {
    await bootstrap(process.cwd(), async () => {
      UI.println(`${UI.Style.TEXT_SUCCESS_BOLD}⚡ Starting Cowork Worker Daemon...${UI.Style.TEXT_NORMAL}`)
      UI.println(`  Agent ID:   ${args["agent-id"]}`)
      UI.println(`  Agent Role: ${args["agent-role"]}`)
      if (args.workspace) {
        UI.println(`  Workspace:  ${args.workspace}`)
      }
      UI.println(`  Interval:   ${args.interval}s`)
      UI.println("")

      const { spawn } = require("child_process")

      let running = true
      process.on("SIGINT", () => {
        UI.println(`\n${UI.Style.TEXT_WARNING}Stopping worker daemon...${UI.Style.TEXT_NORMAL}`)
        running = false
      })

      while (running) {
        try {
          // 1. Claim a queue item
          const claimBody = {
            agent_id: args["agent-id"],
            agent_role: args["agent-role"],
            workspace_id: args.workspace || null,
          }
          const item = await apiCall<{
            id: string
            task_id: string
            status: string
          } | null>("POST", "/api/v1/queue/claim", claimBody)

          if (!item) {
            // Sleep and poll again
            await new Promise((resolve) => setTimeout(resolve, (args.interval as number) * 1000))
            continue
          }

          UI.println(`${UI.Style.TEXT_SUCCESS}✓ Claimed task queue item:${UI.Style.TEXT_NORMAL} ${item.id} (Task ID: ${item.task_id})`)

          // Mark queue item as running
          await apiCall("POST", `/api/v1/queue/${item.id}/start`, {})

          // Fetch the task description
          const taskRes = await apiCall<any>("GET", `/api/v1/tasks/${item.task_id}`)
          const task = taskRes.task || taskRes
          
          UI.println(`  Executing instruction: "${task.title || "No Title"}"`)

          // Create a cowork run session to store logs and stream events
          const run = await apiCall<{ id: string }>("POST", "/api/v1/runs", {
            tenant_id: "default",
            workspace_id: task.workspace_id || "default",
            initiator: args["agent-id"] as string,
            mode: "cowork",
            entrypoint: task.title || "No Title",
            policy_profile: "default",
          })

          UI.println(`  Created run session: ${run.id}`)

          // Spawn the agent execution
          const binPath = process.argv[1]
          const isCompiled = binPath.startsWith("/$bunfs/") || !process.execPath.endsWith("bun");
          const spawnOpts = {
            stdio: ["ignore", "pipe", "pipe"] as const,
            env: {
              ...process.env,
              GIZZI_DISABLE_MODELS_FETCH: "true",
              DISABLE_MODELS_FETCH: "true"
            }
          };
          const child = isCompiled
            ? spawn(process.execPath, ["run", task.title || "No Title"], spawnOpts)
            : spawn("bun", [binPath, "run", task.title || "No Title"], spawnOpts);

          let stdoutData = ""
          let stderrData = ""

          child.stdout.on("data", async (data: Buffer) => {
            const chunk = data.toString()
            stdoutData += chunk
            process.stdout.write(chunk)
            try {
              await apiCall("POST", `/api/v1/runs/${run.id}/events`, {
                event_type: "stdout",
                payload: { content: chunk },
              })
            } catch {
              // Ignore network drops
            }
          })

          child.stderr.on("data", async (data: Buffer) => {
            const chunk = data.toString()
            stderrData += chunk
            process.stderr.write(chunk)
            try {
              await apiCall("POST", `/api/v1/runs/${run.id}/events`, {
                event_type: "stderr",
                payload: { content: chunk },
              })
            } catch {
              // Ignore network drops
            }
          })

          const exitCode = await new Promise<number>((resolve) => {
            child.on("close", resolve)
          })

          const start_time = Date.now()
          const duration_ms = Date.now() - start_time
          const error_msg = exitCode === 0 ? null : `Process exited with code ${exitCode}`

          // Post final exit status event
          try {
            await apiCall("POST", `/api/v1/runs/${run.id}/events`, {
              event_type: exitCode === 0 ? "step_completed" : "step_failed",
              payload: { step_name: exitCode === 0 ? "Execution Completed" : `Execution Failed (code ${exitCode})` },
            })
          } catch {
            // Ignore
          }

          // Complete queue item
          await apiCall("POST", `/api/v1/queue/${item.id}/complete`, {
            result: JSON.stringify({
              success: exitCode === 0,
              duration_ms,
              stdout: stdoutData,
              stderr: stderrData,
            }),
            error: error_msg,
          })

          // Update task status to done or todo (on error)
          await apiCall("PUT", `/api/v1/tasks/${item.task_id}`, {
            status: error_msg ? "todo" : "done",
          })

          UI.println(`  Reported task completion back to backend.\n`)

        } catch (error: any) {
          UI.error(`Error in daemon loop: ${error.message || String(error)}`)
          await new Promise((resolve) => setTimeout(resolve, (args.interval as number) * 1000))
        }
      }
    })
  },
})
