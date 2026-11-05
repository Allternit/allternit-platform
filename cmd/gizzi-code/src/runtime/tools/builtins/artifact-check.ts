import z from "zod/v4"
import path from "path"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Artifacts } from "@/runtime/integrations/artifacts"
import { PaneRender } from "@/runtime/integrations/pane-render"
import { transcript } from "@/runtime/tools/builtins/artifact"
import type { MessageV2 } from "@/runtime/session/message-v2"
import DESCRIPTION from "@/runtime/tools/builtins/artifact-check.txt"

/**
 * artifact_check: the agent looks at what it made. The app showing the
 * session renders the artifact at each width (lib/artifacts/check in
 * allternit-ai) and replies with pictures and problems; with a reference
 * picture it also says how alike they look. The check also stops a refine
 * loop that has stopped improving (Craft Anything's "a builder that can't
 * make progress stops").
 */

export const DEFAULT_WIDTHS = [390, 768, 1440]
export const CHECK_TIMEOUT_MS = 120_000
const MAX_REFERENCE_BYTES = 8 * 1024 * 1024
/** Checks in a row with no better likeness and no fewer problems before the check says stop. */
export const STALL_CHECKS = 3
/** Checks of one artifact in one session before the check says stop. */
export const MAX_CHECKS = 8

const IMAGE_EXT: Record<string, string> = {
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".webp": "image/webp",
  ".gif": "image/gif",
}

/** A reference picture as a data URL, or an error the model can act on. */
export async function resolveReference(
  reference: string,
  messages: MessageV2.WithParts[],
  abort?: AbortSignal,
): Promise<string> {
  const ref = reference.trim()
  if (ref === "attachment") {
    for (let i = messages.length - 1; i >= 0; i--) {
      const m = messages[i]
      if (m.info.role !== "user") continue
      for (let j = m.parts.length - 1; j >= 0; j--) {
        const p = m.parts[j]
        if (p.type === "file" && p.mime.startsWith("image/")) return resolveReference(p.url, [], abort)
      }
    }
    throw new Error("The user hasn't attached a picture in this chat. Ask them for one, or pass an https link or a file path.")
  }
  if (ref.startsWith("data:image/")) return ref
  if (/^https?:\/\//i.test(ref)) {
    const res = await fetch(ref, { signal: abort ?? AbortSignal.timeout(20_000) })
    if (!res.ok) throw new Error(`The reference link answered ${res.status}.`)
    const mime = (res.headers.get("content-type") ?? "").split(";")[0].trim()
    if (!mime.startsWith("image/")) throw new Error(`The reference link is ${mime || "not a picture"}; pass a link to a picture (png, jpg, webp).`)
    const bytes = Buffer.from(await res.arrayBuffer())
    if (bytes.length > MAX_REFERENCE_BYTES) throw new Error("The reference picture is over 8 MB.")
    return `data:${mime};base64,${bytes.toString("base64")}`
  }
  const file = ref.startsWith("file://") ? new URL(ref).pathname : ref
  const mime = IMAGE_EXT[path.extname(file).toLowerCase()]
  if (!mime) throw new Error("The reference must be a png, jpg, webp or gif picture, \"attachment\", or an https link.")
  const f = Bun.file(file)
  if (!(await f.exists())) throw new Error(`No file at ${file}.`)
  if (f.size > MAX_REFERENCE_BYTES) throw new Error("The reference picture is over 8 MB.")
  return `data:${mime};base64,${Buffer.from(await f.arrayBuffer()).toString("base64")}`
}

interface Round {
  problems: number
  similarity?: number
  flat: number
  count: number
}

/** Per session + artifact: the last check, how many checks in a row didn't improve, and how many in all. */
const rounds = new Map<string, Round>()

/** Problems a viewer would notice (blank screens, sideways scroll on narrow screens, errors, mismatched regions). */
export function countProblems(checks: PaneRender.CheckWidth[]): number {
  return checks.reduce(
    (n, c) => n + (c.blank ? 1 : 0) + (c.overflowX > 4 ? 1 : 0) + c.errors.length + (c.mismatch?.length ?? 0),
    0,
  )
}

/**
 * Record one check and say whether refining has stalled. A check counts as
 * no progress when the likeness didn't rise by a point and the problems
 * didn't drop below the best so far (an unchanged body counts too).
 */
export function recordRound(
  key: string,
  problems: number,
  similarity: number | undefined,
): { stalled: boolean; capped: boolean; count: number } {
  const prev = rounds.get(key)
  const better =
    !prev ||
    problems < prev.problems ||
    (similarity !== undefined && (prev.similarity === undefined || similarity > prev.similarity + 0.01))
  const flat = prev && !better ? prev.flat + 1 : 0
  const count = (prev?.count ?? 0) + 1
  const best = prev?.similarity !== undefined && (similarity === undefined || prev.similarity > similarity) ? prev.similarity : similarity
  rounds.set(key, { problems: Math.min(problems, prev?.problems ?? problems), similarity: best, flat, count })
  const done = problems === 0 && (similarity === undefined || similarity >= 0.9)
  return { stalled: !done && flat >= STALL_CHECKS, capped: !done && count >= MAX_CHECKS, count }
}

/** Test hook. */
export function resetCheckRounds(): void {
  rounds.clear()
}

interface CheckMeta {
  ok: boolean
  artifact_id?: string
  version?: number
  problems?: number
  similarity?: number
  stalled?: boolean
  checks?: Array<Omit<PaneRender.CheckWidth, "image">>
}
const meta = (m: CheckMeta): CheckMeta => m

const pct = (v: number) => `${Math.round(v * 100)}%`

export function describeWidth(c: PaneRender.CheckWidth): string {
  const issues: string[] = []
  if (c.blank) issues.push("the first screen is blank (one flat colour)")
  if (c.overflowX > 4) issues.push(`scrolls sideways by ${c.overflowX}px`)
  for (const e of c.errors) issues.push(`error: ${e}`)
  const like =
    c.similarity !== undefined
      ? ` Likeness to the reference: ${pct(c.similarity)}${c.mismatch?.length ? `; differs at ${c.mismatch.join("; ")}` : ""}.`
      : ""
  return `- ${c.width}×${c.height}: ${issues.length ? issues.join("; ") : "no problems found"}.${like}`
}

export const ArtifactCheckTool = Tool.define("artifact_check", {
  description: DESCRIPTION,
  parameters: z.object({
    id: z.string().min(1).describe("The artifact id (art_…)"),
    version: z.number().int().min(1).optional().describe("A specific version; latest when left out"),
    widths: z
      .array(z.number().int().min(240).max(3840))
      .min(1)
      .max(4)
      .optional()
      .describe("Window widths in px; default 390, 768, 1440"),
    reference: z
      .string()
      .optional()
      .describe('What it should look like: "attachment", an https link to a picture, or a picture file path'),
  }),
  async execute(params, ctx) {
    const messages = await transcript(ctx)
    const known = Artifacts.fromTranscript(messages, params.id)
    const artifact = await Artifacts.read({ id: params.id, version: params.version, known }, ctx.abort)
    const name = artifact.title ? `"${artifact.title}"` : params.id
    let reference: string | undefined
    if (params.reference?.trim()) {
      try {
        reference = await resolveReference(params.reference, messages, ctx.abort)
      } catch (error) {
        return {
          title: `Check: ${artifact.title || params.id} — no reference`,
          output: `The check didn't run: ${error instanceof Error ? error.message : String(error)}`,
          metadata: meta({ ok: false }),
        }
      }
    }
    const widths = params.widths?.length ? params.widths : DEFAULT_WIDTHS
    const result = await PaneRender.request(
      {
        sessionID: ctx.sessionID,
        kind: "check",
        format: "html",
        code: artifact.body,
        width: Math.max(...widths),
        height: 900,
        title: artifact.title || params.id,
        artifactKind: artifact.kind,
        bodyFormat: artifact.body_format,
        widths,
        reference,
        toolCall: ctx.callID ? { messageID: ctx.messageID, callID: ctx.callID } : undefined,
      },
      { abort: ctx.abort, timeoutMs: CHECK_TIMEOUT_MS },
    )
    if (!result.ok || !result.checks?.length) {
      return {
        title: `Check: ${artifact.title || params.id} — didn't run`,
        output: `The check didn't run: ${result.error ?? "no result came back"}`,
        metadata: meta({ ok: false }),
      }
    }

    const checks = result.checks
    const problems = countProblems(checks)
    const compared = checks.find((c) => c.similarity !== undefined)
    const round = recordRound(`${ctx.sessionID}:${params.id}`, problems, compared?.similarity)

    const lines = [
      `Checked ${name} version ${artifact.version} at ${checks.map((c) => c.width).join(", ")}px. Pictures of the first screen at each width are attached, in that order: look at them.`,
      ...checks.map(describeWidth),
    ]
    if (result.error) lines.push(`Note: ${result.error}`)
    if (round.stalled || round.capped) {
      lines.push(
        round.capped
          ? `Stop refining: this is check ${round.count} of this artifact in this session.`
          : `Stop refining: the last ${STALL_CHECKS} checks found no better likeness and no fewer problems.`,
        "Tell the user what still doesn't match, show what you have, and ask how they want to go on. Don't call artifact_update for this again until they answer.",
      )
    } else if (problems === 0) {
      lines.push(compared ? "No problems found. If the pictures match the reference closely enough, you're done." : "No problems found.")
    } else {
      lines.push("Fix these with artifact_update, then check again.")
    }

    const summaryBits = [problems ? `${problems} problem${problems === 1 ? "" : "s"}` : "no problems"]
    if (compared?.similarity !== undefined) summaryBits.push(`${pct(compared.similarity)} like the reference`)
    if (round.stalled || round.capped) summaryBits.push("stopped")

    return {
      title: `Check: ${artifact.title || params.id} — ${summaryBits.join(" · ")}`,
      output: lines.join("\n"),
      metadata: meta({
        ok: true,
        artifact_id: params.id,
        version: artifact.version,
        problems,
        similarity: compared?.similarity,
        stalled: round.stalled || round.capped,
        checks: checks.map(({ image: _image, ...rest }) => rest),
      }),
      attachments: checks
        .filter((c) => c.image)
        .map((c) => {
          const mime = c.image!.slice(5, c.image!.indexOf(";")) || "image/jpeg"
          return {
            type: "file" as const,
            mime,
            url: c.image!,
            filename: `${artifact.title || params.id} ${c.width}px.${mime === "image/png" ? "png" : "jpg"}`,
          }
        }),
    }
  },
})
