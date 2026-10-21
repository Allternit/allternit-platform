import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Artifacts } from "@/runtime/integrations/artifacts"
import { Flag } from "@/runtime/context/flag/flag"
import type { MessageV2 } from "@/runtime/session/message-v2"
import CREATE_DESCRIPTION from "@/runtime/tools/builtins/artifact-create.txt"
import UPDATE_DESCRIPTION from "@/runtime/tools/builtins/artifact-update.txt"
import READ_DESCRIPTION from "@/runtime/tools/builtins/artifact-read.txt"
import SUGGEST_DESCRIPTION from "@/runtime/tools/builtins/artifact-suggest.txt"
import * as BotTurn from "@/runtime/bots/bot-turn"

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

// ------------------------------------------------------------ agent watch

/** The name used when the session isn't one of the user's bots. */
export const DEFAULT_AGENT_NAME = "Gizzi"

/** Resolve within `ms`, else undefined; never rejects. */
async function within<T>(ms: number, work: () => Promise<T>): Promise<T | undefined> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      work().catch(() => undefined),
      new Promise<undefined>((resolve) => {
        timer = setTimeout(() => resolve(undefined), ms)
      }),
    ])
  } finally {
    if (timer) clearTimeout(timer)
  }
}

/**
 * The running agent's display name: the bot this turn runs as (the `bot`
 * marker allternit-api puts on the turn), else the gizzi bot whose canonical
 * chat this session is, else "Gizzi".
 */
export async function agentName(ctx: Tool.Context): Promise<string> {
  const marked = BotTurn.resolve(ctx.sessionID, ctx.messages ?? [])?.name?.trim()
  if (marked) return marked
  const bot = await within(300, async () => {
    const { findBotByCanonicalSession } = await import("@/runtime/bots/canonical-chat")
    return findBotByCanonicalSession(ctx.sessionID)
  })
  return bot?.title?.trim() || bot?.name?.trim() || DEFAULT_AGENT_NAME
}

/** The session's model cost so far in USD; undefined when nothing was metered (e.g. a subscription CLI brain). */
export async function sessionCost(sessionID: string): Promise<number | undefined> {
  const usage = await within(500, async () => {
    const { SessionUsage } = await import("@/runtime/session/usage")
    return SessionUsage.getSessionUsage(sessionID)
  })
  const cost = usage?.total?.cost
  return typeof cost === "number" && Number.isFinite(cost) && cost > 0 ? Math.round(cost * 10_000) / 10_000 : undefined
}

/**
 * Best-effort agent presence on an artifact: fire and forget. Never throws and
 * never delays the tool (the name and cost lookups run in the background too).
 */
export function heartbeat(
  ctx: Tool.Context,
  id: string,
  input: { state: "viewing" | "editing"; task: string; anchor?: Artifacts.Anchor; name?: string; cost_usd?: number },
): Promise<boolean> {
  return (async () => {
    try {
      const [name, cost] = await Promise.all([
        input.name ? Promise.resolve(input.name) : agentName(ctx),
        input.cost_usd !== undefined ? Promise.resolve(input.cost_usd) : sessionCost(ctx.sessionID),
      ])
      return await Artifacts.presence(id, input.state, {
        name,
        session_id: ctx.sessionID || undefined,
        task: input.task,
        anchor: input.anchor,
        cost_usd: cost,
      })
    } catch {
      return false
    }
  })()
}

const TaskInput = z.string().max(200).optional()

const ChangeInput = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("text"),
    block: z.number().int().min(0).nullish().describe("0-based block index; null or left out searches the whole doc"),
    find: z.string().min(1).max(20_000).describe("The exact text to replace (its first occurrence)"),
    replace: z.string().max(20_000).describe("The new text"),
    reason: z.string().max(500).optional().describe("One line the user sees"),
  }),
  z.object({
    kind: z.literal("block"),
    op: z.enum(["replace", "insert_after", "delete"]),
    block: z.number().int().min(0).describe("0-based block index"),
    markdown: z.string().max(20_000).optional().describe("New content as markdown; ignored for delete"),
    reason: z.string().max(500).optional().describe("One line the user sees"),
  }),
  z.object({
    kind: z.literal("cell"),
    sheet: z.string().max(200).nullish().describe("Sheet name or id; null or left out = the first sheet"),
    address: z.string().min(1).max(20).describe("A1-style cell address, e.g. B4"),
    value: z.string().max(20_000).describe("The new value; starting with = makes it a formula"),
    reason: z.string().max(500).optional().describe("One line the user sees"),
  }),
])

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
          `The user sees it as a card that opens beside the chat; mention it by its title, don't link the id. Use artifact_update with base_version ${artifact.version} to revise it.`,
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
    task: TaskInput.describe("Short line shown on your cursor while you work, e.g. 'Saving the outline'"),
  }),
  async execute(params, ctx) {
    void heartbeat(ctx, params.id, { state: "editing", task: params.task?.trim() || "Saving" })
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
    task: TaskInput.describe("Short line shown to the user while you work in it, e.g. 'Checking the dates'"),
  }),
  async execute(params, ctx) {
    void heartbeat(ctx, params.id, { state: "viewing", task: params.task?.trim() || "Reading" })
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

export const ArtifactListTool = Tool.define("artifact_list", {
  description:
    "List the user's artifacts (their own and ones shared with them), newest first: id, kind, title, version, who can open it. " +
    "Use it to find an artifact the user mentions by name before artifact_read or artifact_update.",
  parameters: z.object({
    q: z.string().max(200).optional().describe("Words in the title"),
    kind: z.enum(Artifacts.KINDS).optional().describe("Only this kind"),
    scope: z.enum(["all", "mine", "shared"]).optional().describe("all (default), mine, or shared with me"),
    limit: z.number().int().min(1).max(50).optional().describe("How many (default 20)"),
    cursor: z.string().optional().describe("next_cursor from a previous call, for more"),
  }),
  async execute(params, ctx) {
    const { items, next_cursor } = await Artifacts.list({ ...params, limit: params.limit ?? 20 }, ctx.abort)
    const lines = items.map(
      (a) => `${a.id}  ${a.kind}  "${a.title}"  v${a.current_version ?? "?"}  ${a.visibility ?? ""}${a.my_access && a.my_access !== "owner" ? ` (${a.my_access})` : ""}  ${a.updated_at ?? ""}`,
    )
    return {
      title: `${items.length} artifact${items.length === 1 ? "" : "s"}`,
      output: (lines.length ? lines.join("\n") : "No artifacts match.") + (next_cursor ? `\nMore: cursor ${next_cursor}` : ""),
      metadata: { ok: true, items, next_cursor, truncated: false },
    }
  },
})

export const ArtifactDeleteTool = Tool.define("artifact_delete", {
  description:
    "Permanently delete an artifact the user owns: every version, its comments and its share links. There is no trash. " +
    "Only when the user asked to delete that artifact; the user approves each delete.",
  parameters: z.object({ id: z.string().min(1).describe("The artifact id (art_…)") }),
  async execute(params, ctx) {
    await ctx.ask({ permission: "artifact_delete", patterns: [params.id], always: [], metadata: { id: params.id } })
    await Artifacts.remove(params.id, ctx.abort)
    return { title: `Deleted ${params.id}`, output: `Deleted ${params.id} permanently.`, metadata: { ok: true, truncated: false } }
  },
})

export const ArtifactShareTool = Tool.define("artifact_share", {
  description:
    "Read or change who can open an artifact the user owns. Without visibility it returns the current settings. " +
    "visibility: private (only me), people (the listed people), org (everyone in their organization), link (anyone with the link; " +
    "not allowed for artifacts that call AI or connectors, and an org admin may turn it off). Levels: view, comment, edit (Docs: view or edit). " +
    "Changing sharing is outward-facing: only when the user asked; the user approves each change.",
  parameters: z.object({
    id: z.string().min(1).describe("The artifact id (art_…)"),
    visibility: z.enum(["private", "people", "org", "link"]).optional(),
    people: z
      .array(z.object({ email: z.string().email(), level: z.enum(["view", "comment", "edit"]) }))
      .max(50)
      .optional()
      .describe("For people: who, and what they can do. Replaces the current list."),
  }),
  async execute(params, ctx) {
    if (!params.visibility) {
      const current = await Artifacts.sharing(params.id, ctx.abort)
      return { title: `Sharing for ${params.id}`, output: JSON.stringify(current, null, 2), metadata: { ok: true, sharing: current, truncated: false } }
    }
    await ctx.ask({
      permission: "artifact_share",
      patterns: [params.id],
      always: [],
      metadata: { id: params.id, visibility: params.visibility, people: params.people ?? [] },
    })
    const shares = (params.people ?? []).map((p) => ({ principal_type: "email", principal_id: p.email.toLowerCase(), level: p.level }))
    const updated = await Artifacts.setSharing(params.id, { visibility: params.visibility, shares }, ctx.abort)
    return {
      title: `Shared ${params.id}`,
      output: `Sharing is now ${params.visibility}${shares.length ? ` with ${shares.map((s) => `${s.principal_id} (${s.level})`).join(", ")}` : ""}.`,
      metadata: { ok: true, sharing: updated, truncated: false },
    }
  },
})

export const ArtifactCommentTool = Tool.define("artifact_comment", {
  description:
    "Read an artifact's comment threads, or post a comment or a reply (parent_id). Use it to answer a comment that mentions you, " +
    "or to leave a note where the user asked. Leave body out to read.",
  parameters: z.object({
    id: z.string().min(1).describe("The artifact id (art_…)"),
    body: z.string().min(1).max(10_000).optional().describe("The comment text"),
    parent_id: z.string().optional().describe("Reply to this comment id"),
    task: TaskInput.describe("Short line shown to the user while you work, e.g. 'Asking about the dates'"),
  }),
  async execute(params, ctx) {
    if (!params.body) {
      void heartbeat(ctx, params.id, { state: "viewing", task: params.task?.trim() || "Reading comments" })
      const items = await Artifacts.comments(params.id, ctx.abort)
      return { title: `${items.length} comments`, output: JSON.stringify(items, null, 2).slice(0, MAX_MODEL_BODY), metadata: { ok: true, items, comment: undefined as unknown, truncated: false } }
    }
    const name = await agentName(ctx)
    void heartbeat(ctx, params.id, { state: "viewing", task: params.task?.trim() || "Asking a question", name })
    const posted = await Artifacts.comment(
      params.id,
      { body: params.body, parent_id: params.parent_id, agent_name: name },
      ctx.abort,
    )
    return { title: "Comment posted", output: `Posted${params.parent_id ? " a reply" : " a comment"} on ${params.id}.`, metadata: { ok: true, items: [] as any[], comment: posted as unknown, truncated: false } }
  },
})

/** The exact change shape the cloud and the app expect (nullable locations are sent as null). */
export function normalizeChange(change: z.infer<typeof ChangeInput>): Artifacts.Change {
  switch (change.kind) {
    case "text":
      return { kind: "text", block: change.block ?? null, find: change.find, replace: change.replace }
    case "block":
      return change.op === "delete"
        ? { kind: "block", op: "delete", block: change.block }
        : { kind: "block", op: change.op, block: change.block, markdown: change.markdown ?? "" }
    case "cell":
      return { kind: "cell", sheet: change.sheet ?? null, address: change.address.trim().toUpperCase(), value: change.value }
  }
}

export const ArtifactSuggestTool = Tool.define("artifact_suggest", {
  description: SUGGEST_DESCRIPTION,
  parameters: z.object({
    id: z.string().min(1).describe("The artifact id (art_…); a doc or a sheet"),
    base_version: z.number().int().min(1).describe("The version these changes are based on"),
    changes: z.array(ChangeInput).min(1).max(50).describe("The changes, in order: text, block or cell"),
    task: TaskInput.describe("Short line shown on your cursor while you work, e.g. 'Fixing the dates'"),
  }),
  async execute(params, ctx) {
    const [name, cost] = await Promise.all([agentName(ctx), sessionCost(ctx.sessionID)])
    const changes = params.changes.map((item) => ({
      change: normalizeChange(item),
      ...(item.reason?.trim() ? { reason: item.reason.trim() } : {}),
    }))
    void heartbeat(ctx, params.id, {
      state: "editing",
      task: params.task?.trim() || "Suggesting edits",
      anchor: Artifacts.anchorOf(changes[0]?.change),
      name,
      cost_usd: cost,
    })
    const items = await Artifacts.suggest(
      params.id,
      {
        base_version: params.base_version,
        agent_name: name,
        ...(ctx.sessionID ? { session_id: ctx.sessionID } : {}),
        ...(cost !== undefined ? { cost_usd: cost } : {}),
        changes,
      },
      ctx.abort,
    )
    const ids = items.map((s) => s.id)
    return {
      title: `${ids.length} suggestion${ids.length === 1 ? "" : "s"}`,
      output: [
        JSON.stringify({ id: params.id, base_version: params.base_version, suggestion_ids: ids }),
        `Left ${ids.length} suggestion${ids.length === 1 ? "" : "s"} on ${params.id} as ${name}. Nothing is changed yet: ` +
          `the user reviews each one and accepts or rejects it. Tell them you left suggestions to review; don't repeat them.`,
      ].join("\n"),
      metadata: { ok: true, artifact_id: params.id, agent_name: name, suggestions: items, truncated: false },
    }
  },
})

export const ArtifactStorageTool = Tool.define("artifact_storage", {
  description:
    "Read or write a page artifact's saved data (what the page stores with window.allternit.storage). scope personal = only this user; " +
    "shared = everyone who opens it. Actions: list (optional prefix), get, set (value is a string, often JSON), delete. " +
    "Up to 20 MB of text per artifact.",
  parameters: z.object({
    id: z.string().min(1).describe("The artifact id (art_…)"),
    action: z.enum(["list", "get", "set", "delete"]),
    scope: z.enum(["personal", "shared"]).default("personal"),
    key: z.string().min(1).max(200).optional(),
    value: z.string().optional(),
    prefix: z.string().max(200).optional(),
  }),
  async execute(params, ctx) {
    const { id, action, scope, key } = params
    if (action !== "list" && !key) throw new Error("key is required for get, set and delete")
    const result =
      action === "list"
        ? await Artifacts.storageList(id, scope, params.prefix, ctx.abort)
        : action === "get"
          ? await Artifacts.storageGet(id, scope, key!, ctx.abort)
          : action === "set"
            ? await Artifacts.storageSet(id, scope, key!, params.value ?? "", ctx.abort)
            : (await Artifacts.storageDelete(id, scope, key!, ctx.abort), { deleted: key })
    return { title: `Storage ${action}`, output: JSON.stringify(result, null, 2).slice(0, MAX_MODEL_BODY), metadata: { ok: true, truncated: false } }
  },
})

export const ARTIFACT_TOOLS = [
  ArtifactCreateTool,
  ArtifactUpdateTool,
  ArtifactSuggestTool,
  ArtifactReadTool,
  ArtifactListTool,
  ArtifactDeleteTool,
  ArtifactShareTool,
  ArtifactCommentTool,
  ArtifactStorageTool,
]
