/**
 * gizzi workspace team — Team board CLI
 *
 * Usage:
 *   gizzi workspace team board list <workspaceId>
 *   gizzi workspace team board status <workspaceId>
 *   gizzi workspace team board assign <itemId> <assigneeId>
 *   gizzi workspace team skills list <workspaceId>
 */

import { cmd } from "@/cli/commands/cmd"
import * as prompts from "@clack/prompts"

// ─── API ─────────────────────────────────────────────────────────────────────

const PLATFORM_BASE =
  process.env.ALLTERNIT_PLATFORM_URL ||
  process.env.ALLTERNIT_API_URL ||
  "http://localhost:3000"

async function platformCall<T>(
  method: "GET" | "POST" | "PUT" | "DELETE",
  path: string,
  body?: unknown,
): Promise<T> {
  const url = `${PLATFORM_BASE}${path}`
  const token = process.env.ALLTERNIT_SESSION_TOKEN || process.env.ALLTERNIT_API_TOKEN

  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    Accept: "application/json",
  }
  if (token) headers["Authorization"] = `Bearer ${token}`

  const response = await fetch(url, {
    method,
    headers,
    body: body ? JSON.stringify(body) : undefined,
  })

  if (!response.ok) {
    const text = await response.text()
    throw new Error(`Platform API ${response.status}: ${text}`)
  }

  if (response.status === 204) return undefined as T
  return response.json() as Promise<T>
}

// ─── Types ───────────────────────────────────────────────────────────────────

interface BoardItem {
  id: string
  title: string
  status: string
  priority: number
  assigneeType?: string
  assigneeId?: string
  assigneeName?: string
  labels?: string[]
  workspaceId: string
}

interface TeamSkill {
  id: string
  name: string
  description?: string
  version: string
  installedBy: string
  installedAt: string
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

function statusDot(status: string): string {
  const dots: Record<string, string> = {
    backlog: "○",
    todo: "◦",
    in_progress: "●",
    in_review: "◈",
    done: "✓",
    blocked: "✗",
  }
  return dots[status] ?? "·"
}

function priorityLabel(p: number): string {
  if (p >= 76) return "high"
  if (p >= 51) return "med"
  if (p >= 26) return "low"
  return "min"
}

function padEnd(s: string, n: number): string {
  return s.length >= n ? s.slice(0, n) : s + " ".repeat(n - s.length)
}

// ─── Commands ────────────────────────────────────────────────────────────────

async function boardListCommand(workspaceId: string): Promise<void> {
  prompts.intro(`Board — workspace: ${workspaceId}`)

  let items: BoardItem[] = []
  const spin = prompts.spinner()
  spin.start("Fetching board items…")

  try {
    const data = await platformCall<{ items: BoardItem[] }>(
      "GET",
      `/api/v1/board-items?workspaceId=${encodeURIComponent(workspaceId)}`,
    )
    items = data.items ?? []
    spin.stop(`${items.length} item${items.length !== 1 ? "s" : ""} loaded`)
  } catch (e) {
    spin.stop("Failed to fetch items")
    prompts.outro(`Error: ${e instanceof Error ? e.message : String(e)}`)
    process.exit(1)
  }

  if (items.length === 0) {
    prompts.outro("Board is empty.")
    return
  }

  const header = `${"ID".padEnd(10)}  ${"STATUS".padEnd(11)}  ${"PRI".padEnd(4)}  ${"ASSIGNEE".padEnd(14)}  TITLE`
  process.stdout.write(`\n${header}\n${"─".repeat(80)}\n`)

  for (const item of items) {
    const id = item.id.slice(0, 8)
    const status = `${statusDot(item.status)} ${padEnd(item.status.replace("_", " "), 10)}`
    const pri = priorityLabel(item.priority)
    const assignee = padEnd(item.assigneeName ?? item.assigneeId ?? "—", 14)
    const title = item.title.length > 40 ? item.title.slice(0, 37) + "…" : item.title
    process.stdout.write(`${id}  ${status}  ${pri}  ${assignee}  ${title}\n`)
  }

  process.stdout.write("\n")
  prompts.outro("Done")
}

async function boardStatusCommand(workspaceId: string): Promise<void> {
  prompts.intro(`Board status — workspace: ${workspaceId}`)

  const spin = prompts.spinner()
  spin.start("Loading…")

  let items: BoardItem[]
  try {
    const data = await platformCall<{ items: BoardItem[] }>(
      "GET",
      `/api/v1/board-items?workspaceId=${encodeURIComponent(workspaceId)}`,
    )
    items = data.items ?? []
    spin.stop("Loaded")
  } catch (e) {
    spin.stop("Error")
    prompts.outro(`Error: ${e instanceof Error ? e.message : String(e)}`)
    process.exit(1)
  }

  const counts: Record<string, number> = {}
  for (const item of items) {
    counts[item.status] = (counts[item.status] ?? 0) + 1
  }

  const statuses = ["backlog", "todo", "in_progress", "in_review", "done", "blocked"]
  process.stdout.write("\n")
  for (const s of statuses) {
    const n = counts[s] ?? 0
    const bar = "█".repeat(Math.min(n, 30))
    process.stdout.write(`  ${padEnd(s.replace("_", " "), 11)}  ${String(n).padStart(3)}  ${bar}\n`)
  }
  process.stdout.write(`\n  Total: ${items.length}\n\n`)
  prompts.outro("Done")
}

async function boardAssignCommand(itemId: string, assigneeId: string): Promise<void> {
  prompts.intro(`Assigning board item…`)

  const spin = prompts.spinner()
  spin.start(`Assigning ${itemId.slice(0, 8)} → ${assigneeId}`)

  try {
    await platformCall("POST", `/api/v1/cowork-team/board/${encodeURIComponent(itemId)}/assign`, {
      assigneeId,
      assigneeType: "human",
    })
    spin.stop("Assigned")
    prompts.outro(`Item ${itemId.slice(0, 8)} assigned to ${assigneeId}`)
  } catch (e) {
    spin.stop("Failed")
    prompts.outro(`Error: ${e instanceof Error ? e.message : String(e)}`)
    process.exit(1)
  }
}

async function skillsListCommand(workspaceId: string): Promise<void> {
  prompts.intro(`Team skills — workspace: ${workspaceId}`)

  const spin = prompts.spinner()
  spin.start("Fetching skills…")

  let skills: TeamSkill[] = []
  try {
    const data = await platformCall<{ skills: TeamSkill[] }>(
      "GET",
      `/api/v1/team-skills?workspaceId=${encodeURIComponent(workspaceId)}`,
    )
    skills = data.skills ?? []
    spin.stop(`${skills.length} skill${skills.length !== 1 ? "s" : ""} found`)
  } catch (e) {
    spin.stop("Error")
    prompts.outro(`Error: ${e instanceof Error ? e.message : String(e)}`)
    process.exit(1)
  }

  if (skills.length === 0) {
    prompts.outro("No team skills installed.")
    return
  }

  process.stdout.write("\n")
  for (const skill of skills) {
    process.stdout.write(`  ${padEnd(skill.name, 24)}  v${padEnd(skill.version, 8)}  ${skill.description ?? ""}\n`)
  }
  process.stdout.write("\n")
  prompts.outro("Done")
}

// ─── Command Definition ───────────────────────────────────────────────────────

export const CoworkTeamCommand = cmd({
  command: "team",
  describe: "a shared team workspace on the platform: its board (list, status, assign) and skills",
  builder: (yargs) =>
    yargs
      .command(
        "board <subcommand>",
        "Board operations",
        (y) =>
          y
            .command(
              "list <workspaceId>",
              "List all board items",
              (y2) => y2.positional("workspaceId", { type: "string", demandOption: true }),
              async (argv) => { await boardListCommand(argv.workspaceId as string) },
            )
            .command(
              "status <workspaceId>",
              "Show board status summary",
              (y2) => y2.positional("workspaceId", { type: "string", demandOption: true }),
              async (argv) => { await boardStatusCommand(argv.workspaceId as string) },
            )
            .command(
              "assign <itemId> <assigneeId>",
              "Assign a board item",
              (y2) =>
                y2
                  .positional("itemId", { type: "string", demandOption: true })
                  .positional("assigneeId", { type: "string", demandOption: true }),
              async (argv) => {
                await boardAssignCommand(argv.itemId as string, argv.assigneeId as string)
              },
            )
            .demandCommand(1, "Specify a subcommand: list | status | assign"),
        () => {},
      )
      .command(
        "skills <subcommand>",
        "Team skill operations",
        (y) =>
          y
            .command(
              "list <workspaceId>",
              "List installed team skills",
              (y2) => y2.positional("workspaceId", { type: "string", demandOption: true }),
              async (argv) => { await skillsListCommand(argv.workspaceId as string) },
            )
            .demandCommand(1, "Specify a subcommand: list"),
        () => {},
      )
      .demandCommand(1, "Specify a subcommand: board | skills"),
  handler: () => {},
})
