import { Identifier } from "@/shared/id/id"
import { Log } from "@/shared/util/log"
import { Bus } from "@/shared/bus"
import { ModelJobs } from "@/runtime/context/config/model-jobs"
import { ReviewChanges } from "./changes"
import { ReviewEvent } from "./events"
import { ReviewStore } from "./store"
import { ReviewVerdict } from "./verdict"

export { ReviewChanges } from "./changes"
export { ReviewEvent } from "./events"
export { ReviewStore } from "./store"
export { ReviewVerdict } from "./verdict"

const log = Log.create({ service: "review" })

/** Independent reviewer: a fresh agent session with no coder context and
 * read-only tools judges the changes and returns a PASS/FAIL/PARTIAL verdict. */
export namespace Reviewer {
  /** Name of the native read-only agent defined in runtime/loop/agent.ts. */
  export const AGENT = "review"

  export type ModelRef = ModelJobs.ModelRef

  export interface RunInput {
    sessionID?: string
    cwd: string
    instruction?: string
    previous?: ReviewVerdict.Record | ReviewVerdict.Shape
    model?: ModelRef
    round?: number
    maxRounds?: number
    abort?: AbortSignal
  }

  /** Tools the reviewer's turns may never use, on top of the agent's own deny rules. */
  export const DISABLED_TOOLS = {
    edit: false,
    write: false,
    multiedit: false,
    apply_patch: false,
    patch: false,
    task: false,
    todowrite: false,
    todoread: false,
    question: false,
  } as const

  export async function collectChanges(input: { sessionID?: string; cwd: string }) {
    return ReviewChanges.collect(input)
  }

  /** Model choice: explicit → `models.jobs.review` → session model → default. */
  export async function resolveModel(input: { sessionID?: string; model?: ModelRef }): Promise<ModelRef> {
    if (input.model) return input.model
    const job = await ModelJobs.jobModel("review")
    if (job) return job
    if (input.sessionID) {
      const { Session } = await import("@/runtime/session")
      const messages = await Session.messages({ sessionID: input.sessionID as any }).catch(() => [])
      for (let i = messages.length - 1; i >= 0; i--) {
        const info = messages[i]!.info as any
        if (info.role === "assistant" && info.providerID && info.modelID) {
          return { providerID: info.providerID, modelID: info.modelID }
        }
        if (info.role === "user" && info.model?.providerID) return info.model
      }
    }
    const { Provider } = await import("@/runtime/providers/provider")
    const fallback = (await Provider.defaultModel()) as ModelRef
    return { providerID: fallback.providerID, modelID: fallback.modelID }
  }

  export function buildPrompt(input: {
    changes: ReviewChanges.Changes
    instruction?: string
    previous?: ReviewVerdict.Shape
  }): string {
    const { changes } = input
    const lines: string[] = [
      "You are an independent code reviewer. You did not write these changes and you have none of the author's context.",
      "Judge them only from the repository as it is now. Use read, grep, glob, list and lsp to inspect code; bash only for read-only commands (git diff/log/show/status, ls, cat).",
      "You cannot edit files. Do not try to fix anything.",
      "",
    ]
    if (input.instruction?.trim()) {
      lines.push(
        "Check this first — the user asked specifically for it:",
        "<instruction>",
        input.instruction.trim(),
        "</instruction>",
        "",
      )
    }
    lines.push(
      `Changed files (${changes.source === "session" ? "edited in the session" : "uncommitted in git"}${changes.truncated ? ", list truncated" : ""}):`,
      ...changes.files.map((f) => `- ${f.path} (${f.status})`),
      "",
      "For each deleted file, search for callers, imports or references that still use it.",
      "Check that each change is correct, complete, consistent with its callers, and does not break existing behaviour.",
      "",
    )
    if (input.previous) {
      lines.push(
        "A previous review round reported the result below. Treat it as a CLAIM to verify, not as fact:",
        "re-check whether each earlier finding is really fixed and whether anything it marked verified still holds.",
        "<previous_verdict>",
        JSON.stringify(
          {
            verdict: input.previous.verdict,
            findings: input.previous.findings,
            unverified: input.previous.unverified,
            verified: input.previous.verified,
          },
          null,
          2,
        ),
        "</previous_verdict>",
        "",
      )
    }
    lines.push(
      "Answer with the StructuredOutput tool:",
      "- PASS: you verified the changes are correct. List what you checked in `verified`.",
      "- FAIL: at least one real problem. Each finding needs `file` (and `line` when known) and a concrete `problem`.",
      "- PARTIAL: no problems found but some things could not be verified. List each in `unverified` with why.",
      "",
      "<diff>",
      changes.diff || "(no textual diff; read the files)",
      "</diff>",
    )
    return lines.join("\n")
  }

  /** Run one review. Returns null when there is nothing to review (no model call). */
  export async function runReview(input: RunInput): Promise<ReviewVerdict.Record | null> {
    const changes = await collectChanges({ sessionID: input.sessionID, cwd: input.cwd })
    if (!changes) return null

    const started = Date.now()
    const round = input.round ?? 1
    const maxRounds = input.maxRounds ?? 1
    Bus.publish(ReviewEvent.Started, { sessionID: input.sessionID, round, maxRounds, files: changes.files.length })

    const model = await resolveModel(input)
    const { Session } = await import("@/runtime/session")
    const { SessionPrompt } = await import("@/runtime/session/prompt")

    // Fresh context: a new session (child of the coder's session when there is
    // one, so it shows up under it) running the read-only `review` agent.
    const reviewer = await Session.create({
      parentID: input.sessionID?.startsWith("ses") ? (input.sessionID as any) : undefined,
      agentID: AGENT,
      title: `Review round ${round} (@${AGENT} subagent)`,
      permission: Object.keys(DISABLED_TOOLS).map((permission) => ({ permission, pattern: "*", action: "deny" as const })),
    })
    const cancel = () => SessionPrompt.cancel(reviewer.id)
    input.abort?.addEventListener("abort", cancel)

    const format = { type: "json_schema" as const, schema: ReviewVerdict.jsonSchema(), retryCount: 2 }
    let cost = 0
    const tokens = { input: 0, output: 0, reasoning: 0 }
    const turn = async (text: string) => {
      const result = await SessionPrompt.prompt({
        messageID: Identifier.ascending("message"),
        sessionID: reviewer.id,
        model,
        agent: AGENT,
        tools: { ...DISABLED_TOOLS },
        format,
        parts: [{ type: "text", text }],
      } as any)
      const info = result.info as any
      cost += Number(info.cost ?? 0)
      tokens.input += Number(info.tokens?.input ?? 0)
      tokens.output += Number(info.tokens?.output ?? 0)
      tokens.reasoning += Number(info.tokens?.reasoning ?? 0)
      if (info.structured !== undefined) return info.structured
      // No StructuredOutput call: fall back to the last text part (parse() accepts JSON text).
      return (result.parts as any[]).findLast((p) => p.type === "text")?.text
    }

    const previous = input.previous
    let parsed: { value: ReviewVerdict.Shape; failed: boolean }
    try {
      parsed = await ReviewVerdict.parseWithRepair(
        await turn(buildPrompt({ changes, instruction: input.instruction, previous })),
        (error) =>
          turn(
            `Your verdict did not match the required format: ${error}\nReturn the corrected verdict with the StructuredOutput tool. FAIL needs at least one finding with a file; PARTIAL needs at least one unverified item.`,
          ),
      )
    } catch (error) {
      if (input.abort?.aborted) throw error
      log.warn("review turn failed", { sessionID: input.sessionID, error })
      parsed = {
        value: ReviewVerdict.partialFromFailure(error instanceof Error ? error.message : String(error)),
        failed: true,
      }
    } finally {
      input.abort?.removeEventListener("abort", cancel)
    }

    const record: ReviewVerdict.Record = {
      ...parsed.value,
      id: Identifier.ascending("message").replace(/^msg_/, "rev_"),
      sessionID: input.sessionID,
      reviewerSessionID: reviewer.id,
      time: { started, finished: Date.now() },
      model: `${model.providerID}/${model.modelID}`,
      source: changes.source,
      files: changes.files,
      instruction: input.instruction?.trim() || undefined,
      round,
      cost,
      tokens,
    }
    await ReviewStore.add(record).catch((error) => log.warn("failed to store verdict", { error }))
    Bus.publish(ReviewEvent.Finished, {
      sessionID: input.sessionID,
      round,
      maxRounds,
      verdict: record.verdict,
      reviewID: record.id,
      findings: record.findings.length,
    })
    return record
  }

  export async function listReviews(sessionID?: string) {
    return ReviewStore.list(sessionID)
  }
}
