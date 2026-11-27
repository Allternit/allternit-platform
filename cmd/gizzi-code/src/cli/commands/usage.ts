/**
 * Usage command
 *
 * Prints your Allternit Cloud inference usage from
 * GET /api/v1/me/usage/report.
 *
 * Usage:
 *   gizzi usage [--days N] [--group-by day|model|project|client] [--project <id>] [--json]
 */

import type { Argv } from "yargs"
import * as fs from "fs"
import * as os from "os"
import * as path from "path"
import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import { EOL } from "os"
import { cloudInferenceToken, cloudOrigin } from "@/runtime/providers/discovery/allternit-cloud"
import { fetchLimits, freeweightsKey, storageLine } from "@/runtime/services/freeweightsUpload"
import { openEntitlement } from "@/cli/ui/ink-app/services/allternitAccount"
import { CloudContinue } from "@/runtime/session/cloud-continue"
import { Installation } from "@/shared/installation"

type UsageRow = {
  group: string
  requests: number
  prompt_tokens: number
  completion_tokens: number
  cost_usd: number
}

const GROUPS = ["day", "model", "project", "client"] as const

// Same order the other commands use: env / paired device token first, then the
// `gizzi login` session file.
async function resolveToken(): Promise<string | undefined> {
  const direct = await cloudInferenceToken()
  if (direct) return direct
  try {
    const dir = process.env.GIZZI_CONFIG_DIR ?? path.join(os.homedir(), ".config", "gizzi-code")
    const session = JSON.parse(fs.readFileSync(path.join(dir, "session.json"), "utf8"))
    return session?.accessToken || undefined
  } catch {
    return undefined
  }
}

function fmtInt(n: number): string {
  return Math.round(n || 0).toLocaleString("en-US")
}

function fmtCost(n: number): string {
  return `$${(n || 0).toFixed(4)}`
}

type UsageWindow = {
  id: string
  label?: string
  used_usd?: number
  limit_usd?: number | null
  resets_at?: string
}
type UsagePace = { window_id?: string; runs_out_at?: string | null }

function fmtWhen(iso: string): string {
  const t = Date.parse(iso)
  if (Number.isNaN(t)) return iso
  return new Date(t).toLocaleString("en-US", { weekday: "short", hour: "numeric", minute: "2-digit" })
}

/** Pace line plus the 5h / 7d / month rows from GET /api/v1/me/usage (snake_case). Older servers send neither: print nothing. */
export function renderWindows(body: { windows?: UsageWindow[]; pace?: UsagePace }): string[] {
  const out: string[] = []
  const windows = Array.isArray(body.windows) ? body.windows : []
  const resetOf = (id?: string) => windows.find((w) => w.id === id)?.resets_at
  if (body.pace?.runs_out_at) {
    const reset = resetOf(body.pace.window_id)
    out.push(`At this pace you run out ${fmtWhen(body.pace.runs_out_at)}${reset ? `, before ${fmtWhen(reset)}` : ""}.`)
  }
  for (const w of windows) {
    const limit = typeof w.limit_usd === "number" ? `of ${fmtCost(w.limit_usd)}` : "no limit"
    const resets = w.resets_at ? ` · resets ${fmtWhen(w.resets_at)}` : ""
    out.push(`${(w.label ?? w.id).padEnd(8)}${fmtCost(w.used_usd ?? 0)} ${limit}${resets}`)
  }
  if (out.length) out.push("")
  return out
}

async function fetchWindows(token: string): Promise<string[]> {
  try {
    const res = await fetch(`${cloudOrigin()}/api/v1/me/usage`, {
      headers: { Authorization: `Bearer ${token}`, Accept: "application/json", "X-Allternit-Client": `gizzi-code/${Installation.VERSION}` },
      signal: AbortSignal.timeout(8_000),
    })
    if (!res.ok) return []
    return renderWindows((await res.json()) as { windows?: UsageWindow[]; pace?: UsagePace })
  } catch {
    return []
  }
}

function table(rows: string[][], header: string[]): string {
  const all = [header, ...rows]
  const widths = header.map((_, i) => Math.max(...all.map((r) => r[i].length)))
  const line = (r: string[]) =>
    r.map((c, i) => (i === 0 ? c.padEnd(widths[i]) : c.padStart(widths[i]))).join("  ")
  return [line(header), widths.map((w) => "-".repeat(w)).join("  "), ...rows.map(line)].join(EOL)
}

function fail(message: string): never {
  UI.error(message)
  process.exit(1)
}

export const UsageCommand = cmd({
  command: "usage",
  describe: "show your Allternit Cloud inference usage",
  builder: (yargs: Argv) =>
    yargs
      .option("days", { type: "number", default: 30, describe: "number of days to report" })
      .option("group-by", { type: "string", choices: [...GROUPS], default: "day", describe: "how to group rows" })
      .option("project", { type: "string", describe: "only this project id" })
      .option("json", { type: "boolean", default: false, describe: "print the raw rows as JSON" }),
  async handler(args) {
    const days = Number(args.days)
    if (!Number.isFinite(days) || days < 1 || days > 366) fail("--days must be a number from 1 to 366.")

    const token = await resolveToken()
    if (!token) fail("You are not signed in to Allternit Cloud. Run `gizzi login` first.")

    const to = new Date()
    const from = new Date(to.getTime() - days * 86_400_000)
    const qs = new URLSearchParams({
      from: from.toISOString(),
      to: to.toISOString(),
      group_by: String(args["group-by"] ?? "day"),
    })
    if (args.project) qs.set("project_id", String(args.project))

    let res: Response
    try {
      res = await fetch(`${cloudOrigin()}/api/v1/me/usage/report?${qs}`, {
        headers: {
          Authorization: `Bearer ${token}`,
          Accept: "application/json",
          "X-Allternit-Client": `gizzi-code/${Installation.VERSION}`,
        },
        signal: AbortSignal.timeout(15_000),
      })
    } catch (error) {
      fail(`Could not reach Allternit Cloud: ${error instanceof Error ? error.message : String(error)}`)
    }

    if (res.status === 401 || res.status === 403)
      fail("Allternit Cloud did not accept your credentials. Run `gizzi login` again.")
    if (res.status === 404 || res.status === 501) fail("Usage reporting isn't available on this server yet.")
    if (!res.ok) {
      let detail = ""
      try {
        const body = (await res.json()) as { error?: { message?: string } | string; message?: string }
        detail = (typeof body.error === "object" ? body.error?.message : undefined) || body.message || (typeof body.error === "string" ? body.error : "") || ""
      } catch {}
      fail(`Usage report failed (HTTP ${res.status})${detail ? `: ${detail}` : ""}`)
    }

    const body = (await res.json().catch(() => ({}))) as { rows?: UsageRow[] }
    const rows = body.rows ?? []

    // Storage line only (no perks): Freeweights upload storage, from the public limits route.
    const windowLines = args.json ? [] : await fetchWindows(token)
    if (windowLines.length) UI.println(windowLines.join(EOL))
    const storage = args.json ? undefined : storageLine(await fetchLimits(freeweightsKey()))
    const openEnt = args.json ? undefined : await openEntitlement().catch(() => undefined)
    const openLine = openEnt ? CloudContinue.openUsageLine(openEnt) : undefined

    if (args.json) {
      UI.println(JSON.stringify(rows, null, 2))
      return
    }
    if (rows.length === 0) {
      UI.println(`No usage in the last ${days} day${days === 1 ? "" : "s"}.`)
      if (openLine) UI.println(openLine)
      if (storage) UI.println(storage)
      return
    }

    const total = rows.reduce(
      (t, r) => ({
        requests: t.requests + (r.requests || 0),
        prompt: t.prompt + (r.prompt_tokens || 0),
        completion: t.completion + (r.completion_tokens || 0),
        cost: t.cost + (r.cost_usd || 0),
      }),
      { requests: 0, prompt: 0, completion: 0, cost: 0 },
    )
    const out = rows.map((r) => [
      String(r.group ?? "-"),
      fmtInt(r.requests),
      fmtInt(r.prompt_tokens),
      fmtInt(r.completion_tokens),
      fmtCost(r.cost_usd),
    ])
    out.push(["Total", fmtInt(total.requests), fmtInt(total.prompt), fmtInt(total.completion), fmtCost(total.cost)])
    UI.println(table(out, [String(args["group-by"] ?? "day"), "requests", "prompt tokens", "completion tokens", "cost (USD)"]))
    if (openLine) UI.println(EOL + openLine)
    if (storage) UI.println((openLine ? "" : EOL) + storage)
  },
})
