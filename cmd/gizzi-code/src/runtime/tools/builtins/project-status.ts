import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { apiFetch, getAllternitApiConfigWithDeviceToken } from "@/runtime/services/api/allternitApi"
import { agentName } from "@/runtime/tools/builtins/artifact"
import DESCRIPTION from "@/runtime/tools/builtins/project-status.txt"

/**
 * Project status pages. Talks to allternit-api's
 * GET/PATCH /api/v1/cowork/projects/:id/status and POST .../close on the API
 * that runs this session (ALLTERNIT_API_URL: the local API on Desktop, the
 * cloud API on hosted runtimes), with the shared allternitApi credentials.
 */

const SECTIONS = [
  "goal",
  "status",
  "next_actions",
  "owners",
  "deadlines",
  "decisions",
  "blockers",
  "outcome",
  "lessons",
] as const

const TIMEOUT_MS = 15_000

async function call(method: "GET" | "PATCH" | "POST", path: string, body?: unknown): Promise<any> {
  const config = await getAllternitApiConfigWithDeviceToken()
  const response = await apiFetch(config, path, {
    method,
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(TIMEOUT_MS),
  })
  const text = await response.text().catch(() => "")
  let json: any
  try {
    json = text ? JSON.parse(text) : undefined
  } catch {
    json = { message: text }
  }
  if (!response.ok) {
    const code = json?.error ?? `HTTP ${response.status}`
    if (code === "proof_required") throw new Error("proof_required: a done state needs a proof_url (a link to the evidence).")
    if (code === "project_closed") throw new Error("project_closed: this project is closed. Reopen it first (close with reopen: true).")
    if (code === "not_found") throw new Error("not_found: no such project for this account. Check project_id.")
    if (response.status === 401 || response.status === 403) throw new Error(`Allternit rejected the credentials (HTTP ${response.status}).`)
    throw new Error(`${code}${json?.message ? `: ${json.message}` : ""}`)
  }
  return json
}

function render(status: any): string {
  const lines = [`State: ${status.state} · Lifecycle: ${status.lifecycle} · Revision: ${status.revision}`]
  lines.push(`Next: ${status.next_action || "(none set)"}`)
  for (const key of SECTIONS) {
    const text = status.sections?.[key]?.text
    if (text) lines.push(`${key}: ${text}`)
  }
  for (const entry of (status.log ?? []).slice(0, 3)) {
    lines.push(`log: ${String(entry.at).slice(0, 10)} ${entry.by}: ${entry.note}`)
  }
  return lines.join("\n")
}

export const ProjectStatusTool = Tool.define("project_status", {
  description: DESCRIPTION,
  parameters: z.object({
    action: z.enum(["read", "update", "close"]).default("read").describe("read, update or close"),
    project_id: z
      .string()
      .optional()
      .describe("The project id. Optional when the session's project is known; the status block in your prompt lists it."),
    section: z.enum(SECTIONS).optional().describe("Which section to write (update)."),
    text: z.string().optional().describe("New text for the section (update). Replaces that section only."),
    state: z.enum(["on_track", "at_risk", "blocked", "needs_decision", "done"]).optional().describe("Project state (update). done needs proof_url."),
    next_action: z.string().optional().describe("One line: the next step (update)."),
    note: z.string().optional().describe("The log line for this change (update). Defaults to 'updated <section>'."),
    proof_url: z.string().optional().describe("Link to evidence. Required for state done or done: true."),
    done: z.boolean().optional().describe("Mark the project done (needs proof_url)."),
    outcome: z.string().optional().describe("What the project achieved (close)."),
    lessons: z.string().optional().describe("What to remember next time (close)."),
    reopen: z.boolean().optional().describe("Reopen a closed project (close)."),
  }),
  async execute(params, ctx) {
    const id = (params.project_id?.trim() || process.env.ALLTERNIT_PROJECT_ID?.trim() || "").trim()
    if (!id) throw new Error("Pass project_id (it is listed in the project status block of your prompt).")
    const base = `/api/v1/cowork/projects/${encodeURIComponent(id)}`
    const by = await agentName(ctx)
    if (params.action === "read") {
      const { status } = await call("GET", `${base}/status`)
      return { title: `Project status: ${status.state}`, output: render(status), metadata: { projectId: id, status } }
    }
    if (params.action === "update") {
      const { status } = await call("PATCH", `${base}/status`, {
        by,
        section: params.section,
        text: params.text,
        state: params.state,
        next_action: params.next_action,
        note: params.note,
        proof_url: params.proof_url,
        done: params.done,
      })
      return { title: `Project status updated: ${status.state}`, output: render(status), metadata: { projectId: id, status } }
    }
    const { status, paused_routines } = await call("POST", `${base}/close`, {
      by,
      outcome: params.outcome,
      lessons: params.lessons,
      reopen: params.reopen,
    })
    return {
      title: params.reopen ? "Project reopened" : "Project closed",
      output: `${render(status)}\nPaused routines: ${paused_routines ?? 0}`,
      metadata: { projectId: id, status, paused_routines },
    }
  },
})
