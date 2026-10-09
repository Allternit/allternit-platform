import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Artifacts } from "@/runtime/integrations/artifacts"
import { Flag } from "@/runtime/context/flag/flag"
import type { MessageV2 } from "@/runtime/session/message-v2"
import CREATE_DESCRIPTION from "@/runtime/tools/builtins/artifact-create.txt"
import UPDATE_DESCRIPTION from "@/runtime/tools/builtins/artifact-update.txt"
import READ_DESCRIPTION from "@/runtime/tools/builtins/artifact-read.txt"

/**
 * Artifacts v2 model tools (docs/design/artifacts-v2.md §5). Every result's
 * metadata carries `artifact` (Artifacts.Payload: id, kind, title,
 * body_format, version, full body, persisted) so the app renders the card
 * and, when `persisted` is false, saves it to the cloud store under the same
 * id. The model-visible output keeps the body out (create/update) or caps it
 * (read); the first line is a JSON summary so a CLI driving these over the
 * bridge (no metadata) still hands the app the id and version.
 */

/** Cap on the body shown to the model by artifact_read; the metadata keeps all of it. */
export const MAX_MODEL_BODY = 60_000

const Meta = z.record(z.string(), z.unknown())

function summary(a: Artifacts.Payload): string {
  return JSON.stringify({
    id: a.id,
    version: a.version,
    kind: a.kind,
    title: a.title,
    body_format: a.body_format,
    persisted: a.persisted,
  })
}

/** This session's earlier artifact tool results (the bridge context has none, so load them). */
async function transcript(ctx: Tool.Context): Promise<MessageV2.WithParts[]> {
  if (ctx.messages?.length) return ctx.messages
  try {
    const { Session } = await import("@/runtime/session")
    return await Session.messages({ sessionID: ctx.sessionID })
  } catch {
    return []
  }
}

export const ArtifactCreateTool = Tool.define("artifact_create", {
  description: CREATE_DESCRIPTION,
  parameters: z.object({
    kind: z.enum(Artifacts.KINDS).describe("What the artifact is: " + Artifacts.KINDS.join(", ")),
    title: z.string().min(1).max(200).describe("Short name shown on the card, e.g. 'Q3 launch plan'"),
    body: z.string().min(1).describe("The complete content, in body_format"),
    body_format: z.string().optional().describe("MIME type of the body; inferred from kind when left out"),
    icon: z.string().max(40).optional().describe("One generic word for the icon, e.g. chart, calendar, code"),
    meta: Meta.optional().describe("Extra fields, e.g. {language, filename} for code"),
  }),
  async execute(params, ctx) {
    const kind = params.kind
    const bodyFormat = params.body_format?.trim() || Artifacts.inferFormat(kind, params.body)
    const invalid = Artifacts.validate(kind, bodyFormat, params.body)
    if (invalid) throw new Error(invalid)

    const artifact = await Artifacts.create(
      {
        kind,
        title: params.title.trim(),
        body: params.body,
        body_format: bodyFormat,
        icon: params.icon?.trim() || undefined,
        meta: params.meta,
        origin: {
          surface: "gizzi",
          client: Flag.GIZZI_CLIENT || undefined,
          session_id: ctx.sessionID,
          message_id: ctx.messageID,
          ...(process.env.GIZZI_COMPUTER_ID ? { computer_id: process.env.GIZZI_COMPUTER_ID } : {}),
        },
      },
      ctx.abort,
    )
    return {
      title: artifact.title,
      output: [
        summary(artifact),
        `Created ${artifact.kind} artifact "${artifact.title}" (id ${artifact.id}, version ${artifact.version}). ` +
          `The user sees it as a card that opens beside the chat. Use artifact_update with base_version ${artifact.version} to revise it.`,
      ].join("\n"),
      metadata: { ok: true, artifact, truncated: false },
    }
  },
})

export const ArtifactUpdateTool = Tool.define("artifact_update", {
  description: UPDATE_DESCRIPTION,
  parameters: z.object({
    id: z.string().min(1).describe("The artifact id (art_…)"),
    body: z.string().min(1).describe("The complete new content, in the artifact's body format"),
    base_version: z.number().int().min(1).describe("The version this change is based on"),
    note: z.string().max(500).optional().describe("One line on what changed"),
  }),
  async execute(params, ctx) {
    const known = Artifacts.fromTranscript(await transcript(ctx), params.id)
    if (known && Artifacts.isKind(known.kind)) {
      const invalid = Artifacts.validate(known.kind, known.body_format, params.body)
      if (invalid) throw new Error(invalid)
    }
    const artifact = await Artifacts.update(
      { id: params.id, body: params.body, base_version: params.base_version, note: params.note, known },
      ctx.abort,
    )
    return {
      title: artifact.title || params.id,
      output: [
        summary(artifact),
        `Saved version ${artifact.version} of ${artifact.title ? `"${artifact.title}"` : params.id}. ` +
          `Use base_version ${artifact.version} for the next update.`,
      ].join("\n"),
      metadata: { ok: true, artifact, truncated: false },
    }
  },
})

export const ArtifactReadTool = Tool.define("artifact_read", {
  description: READ_DESCRIPTION,
  parameters: z.object({
    id: z.string().min(1).describe("The artifact id (art_…)"),
    version: z.number().int().min(1).optional().describe("A specific version; latest when left out"),
  }),
  async execute(params, ctx) {
    const known = Artifacts.fromTranscript(await transcript(ctx), params.id)
    const artifact = await Artifacts.read({ id: params.id, version: params.version, known }, ctx.abort)
    const body =
      artifact.body.length > MAX_MODEL_BODY
        ? `${artifact.body.slice(0, MAX_MODEL_BODY)}\n…(truncated: ${artifact.body.length} characters in all)`
        : artifact.body
    return {
      title: artifact.title || params.id,
      output: [summary(artifact), "", body].join("\n"),
      metadata: { ok: true, artifact, truncated: artifact.body.length > MAX_MODEL_BODY },
    }
  },
})

export const ARTIFACT_TOOLS = [ArtifactCreateTool, ArtifactUpdateTool, ArtifactReadTool]
