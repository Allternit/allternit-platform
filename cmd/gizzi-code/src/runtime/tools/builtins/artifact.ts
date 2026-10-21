import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { Artifacts } from "@/runtime/integrations/artifacts"
import { Flag } from "@/runtime/context/flag/flag"
import type { MessageV2 } from "@/runtime/session/message-v2"
import CREATE_DESCRIPTION from "@/runtime/tools/builtins/artifact-create.txt"
import UPDATE_DESCRIPTION from "@/runtime/tools/builtins/artifact-update.txt"
import READ_DESCRIPTION from "@/runtime/tools/builtins/artifact-read.txt"
import SUGGEST_DESCRIPTION from "@/runtime/tools/builtins/artifact-suggest.txt"
import CLAIM_DESCRIPTION from "@/runtime/tools/builtins/artifact-claim.txt"
import RELEASE_DESCRIPTION from "@/runtime/tools/builtins/artifact-release.txt"
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
      // While this session holds a frame of this design, every heartbeat
      // carries it; a heartbeat without the anchor would release the claim.
      const held = claimOf(ctx.sessionID, id)
      if (held) held.touched = Date.now()
      const anchor = input.anchor ?? (held ? { kind: "artboard" as const, artboard: held.artboard } : undefined)
      const [name, cost] = await Promise.all([
        input.name ? Promise.resolve(input.name) : agentName(ctx),
        input.cost_usd !== undefined ? Promise.resolve(input.cost_usd) : sessionCost(ctx.sessionID),
      ])
      return await Artifacts.presence(id, input.state, {
        name,
        session_id: ctx.sessionID || undefined,
        task: input.task,
        anchor,
        cost_usd: cost,
      })
    } catch {
      return false
    }
  })()
}

/** The task line for a heartbeat: the tool's `task`, else the held frame's task, else the tool's default. */
function taskFor(ctx: Tool.Context, id: string, task: string | undefined, fallback: string): string {
  return task?.trim() || claimOf(ctx.sessionID, id)?.task || fallback
}

// ------------------------------------------------------------ agents on one canvas

/**
 * Frames this process's sessions hold (SPEC-agent-canvas-2026-10-20). A claim
 * is an agent presence row whose anchor is the artboard, so it lives only
 * while heartbeats keep coming (45 s TTL on the server). One frame per
 * session per artifact: claiming another moves the claim.
 */
export interface Claim {
  artifactId: string
  /** The artboard's shape id ("shape:…"). */
  artboard: string
  /** The artboard's name, for messages. */
  frame: string
  task: string
  name: string
  sessionID: string
  touched: number
  timer?: ReturnType<typeof setInterval>
}

/** A held claim is re-sent this often, so it outlives long model turns. */
export const CLAIM_REFRESH_MS = 20_000
/** With no artifact tool call on the design for this long, the claim is let go. */
export const CLAIM_IDLE_MS = 3 * 60_000

const claims = new Map<string, Claim>()
const claimKey = (sessionID: string, artifactId: string) => `${sessionID}\u0000${artifactId}`

export function claimOf(sessionID: string, artifactId: string): Claim | undefined {
  return claims.get(claimKey(sessionID, artifactId))
}

function dropClaim(claim: Claim) {
  if (claim.timer) clearInterval(claim.timer)
  const key = claimKey(claim.sessionID, claim.artifactId)
  if (claims.get(key) === claim) claims.delete(key)
}

/** Forget every claim (tests). */
export function resetClaims() {
  for (const claim of [...claims.values()]) dropClaim(claim)
}

function hold(claim: Claim): Claim | undefined {
  const previous = claimOf(claim.sessionID, claim.artifactId)
  if (previous) dropClaim(previous)
  claims.set(claimKey(claim.sessionID, claim.artifactId), claim)
  claim.timer = setInterval(() => {
    if (Date.now() - claim.touched > CLAIM_IDLE_MS) {
      dropClaim(claim)
      void Artifacts.presence(claim.artifactId, "viewing", { name: claim.name, session_id: claim.sessionID || undefined, task: claim.task })
      return
    }
    Artifacts.claim(
      claim.artifactId,
      { name: claim.name, session_id: claim.sessionID || undefined, task: claim.task, anchor: { kind: "artboard", artboard: claim.artboard } },
      claim.frame,
    ).catch((error) => {
      // Someone else took it after a lapse: stop pretending to hold it.
      if (error instanceof Artifacts.ClaimedError) dropClaim(claim)
    })
  }, CLAIM_REFRESH_MS)
  ;(claim.timer as { unref?: () => void }).unref?.()
  return previous && previous.artboard !== claim.artboard ? previous : undefined
}

/** tldraw shape ids are "shape:…". The app adds the prefix to model-written ids when it loads a canvas, so changes carry it too. */
export function shapeId(id: string): string {
  const t = id.trim()
  return t.startsWith("shape:") ? t : `shape:${t}`
}
const bareId = (id: string) => id.trim().replace(/^shape:/, "")
const isObj = (v: unknown): v is Record<string, any> => typeof v === "object" && v !== null && !Array.isArray(v)

export interface CanvasDoc {
  artboards: Array<Record<string, any>>
  layers: Artifacts.CanvasLayer[]
}

/** The canvas in a design body, or undefined when the body isn't one (an HTML design, or not JSON). */
export function parseCanvas(body: string, bodyFormat: string): CanvasDoc | undefined {
  if (bodyFormat !== Artifacts.FORMATS.design[0]) return undefined
  if (!body.trim()) return { artboards: [], layers: [] }
  try {
    const doc = JSON.parse(body)
    if (!isObj(doc) || doc.v !== 2 || doc.kind !== "canvas") return undefined
    return {
      artboards: (Array.isArray(doc.artboards) ? doc.artboards : []).filter((a: unknown) => isObj(a) && typeof a.id === "string"),
      layers: (Array.isArray(doc.layers) ? doc.layers : []).filter((l: unknown) => isObj(l) && typeof l.id === "string"),
    }
  } catch {
    return undefined
  }
}

function findArtboard(doc: CanvasDoc, id: string) {
  return doc.artboards.find((a) => bareId(a.id) === bareId(id))
}

function artboardList(doc: CanvasDoc): string {
  return doc.artboards.length
    ? `Its artboards: ${doc.artboards.map((a) => `${a.name || "Untitled"} (${a.id})`).join(", ")}.`
    : "It has no artboards yet."
}

function frameName(board: Record<string, any>): string {
  return typeof board.name === "string" && board.name.trim() ? board.name.trim() : String(board.id)
}

/** An artboard's layers in stacking order, with shape ids. */
function layersOn(doc: CanvasDoc, artboard: string): Artifacts.CanvasLayer[] {
  return doc.layers
    .filter((l) => typeof l.artboard === "string" && bareId(l.artboard) === bareId(artboard))
    .map((l) => ({ ...l, id: shapeId(l.id), artboard: shapeId(artboard) }))
}

/** The artboard's current values for the fields a patch names. */
function framePart(board: Record<string, any>, keys: string[]): Artifacts.FramePatch {
  const out: Record<string, unknown> = {}
  for (const k of keys) {
    if (k === "props") out.props = isObj(board.props) ? board.props : {}
    else if (board[k] !== undefined) out[k] = board[k]
  }
  return out as Artifacts.FramePatch
}

/** JSON with sorted keys, so two reads of an untouched frame compare equal. */
function stable(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(stable).join(",")}]`
  if (isObj(v)) return `{${Object.keys(v).sort().map((k) => `${JSON.stringify(k)}:${stable(v[k])}`).join(",")}}`
  return JSON.stringify(v) ?? "null"
}

/** True when the artboard (frame and layers) is the same in both canvases. */
export function sameArtboard(a: CanvasDoc, b: CanvasDoc, artboard: string): boolean {
  const ba = findArtboard(a, artboard)
  const bb = findArtboard(b, artboard)
  if (!ba || !bb) return false
  const frame = (x: Record<string, any>) => stable({ ...x, x: undefined, y: undefined, id: bareId(x.id) })
  return frame(ba) === frame(bb) && stable(layersOn(a, artboard)) === stable(layersOn(b, artboard))
}

/** Cap on each of `layers` and `before` (JSON), as the cloud enforces. */
export const MAX_ARTBOARD_JSON = 2 * 1024 * 1024
export const MAX_ARTBOARD_LAYERS = 2000
/** Generated sliders per artboard change, and their JSON, as the cloud enforces (400 bad_controls). */
export const MAX_CONTROLS = 12
export const MAX_CONTROLS_JSON = 64 * 1024

/**
 * Check the model's controls against the change's layers and give each layer
 * reference its shape id. Throws a message the model can act on.
 */
export function buildControls(
  controls: Array<Record<string, any>>,
  layers: Artifacts.CanvasLayer[],
  frame: string,
): Artifacts.Control[] {
  if (controls.length > MAX_CONTROLS) throw new Error(`Offer at most ${MAX_CONTROLS} controls per frame; 2 to 5 is usually right.`)
  const ids = new Set(layers.map((l) => bareId(l.id)))
  const seen = new Set<string>()
  const out = controls.map((c) => {
    const id = String(c.id ?? "").trim()
    if (seen.has(id)) throw new Error(`Two controls on ${frame} use the id "${id}".`)
    seen.add(id)
    const prop = String(c.prop ?? "").trim()
    const layer = c.layer == null || c.layer === "" ? null : String(c.layer)
    if (layer !== null && !ids.has(bareId(layer))) {
      throw new Error(`Control "${id}" points at layer ${layer}, which isn't in ${frame}'s layers. Use one of their ids, or null for the frame.`)
    }
    if (prop.startsWith("frame.") && layer !== null) {
      throw new Error(`Control "${id}" uses ${prop}, which targets the frame, so its layer must be null.`)
    }
    const base = { id, label: String(c.label ?? id).trim(), layer: layer === null ? null : shapeId(layer), prop }
    if (c.type === "range") {
      const { min, max, value, step } = c
      if (!(min < max)) throw new Error(`Control "${id}": min must be less than max.`)
      if (!(value >= min && value <= max)) throw new Error(`Control "${id}": value ${value} is outside ${min} to ${max}.`)
      return { ...base, type: "range" as const, min, max, ...(typeof step === "number" ? { step } : {}), value }
    }
    if (c.type === "select") {
      const options = (c.options ?? []).map(String)
      if (!options.includes(String(c.value))) throw new Error(`Control "${id}": value must be one of ${options.join(", ")}.`)
      return { ...base, type: "select" as const, options, value: String(c.value) }
    }
    return { ...base, type: "color" as const, value: String(c.value).trim() }
  })
  if (JSON.stringify(out).length > MAX_CONTROLS_JSON) throw new Error(`${frame}'s controls are over 64 KB of JSON; offer fewer.`)
  return out
}

/**
 * Turn the model's `{artboard, layers, frame?}` into the cloud's artboard
 * change, reading `before` from the canvas. Throws a message the model can
 * act on when the artboard doesn't exist, a layer belongs to another
 * artboard, or a layer id is taken elsewhere.
 */
export function buildArtboardChange(
  doc: CanvasDoc,
  input: {
    artboard: string
    /** Left out = the frame's layers as they are now (a controls-only change). */
    layers?: Array<Record<string, any>>
    frame?: Artifacts.FramePatch
    controls?: Array<Record<string, any>>
  },
): Extract<Artifacts.Change, { kind: "artboard" }> {
  const board = findArtboard(doc, input.artboard)
  if (!board) throw new Error(`There's no artboard "${input.artboard}" in this design. ${artboardList(doc)}`)
  const artboard = shapeId(board.id)
  const name = frameName(board)
  const hasFrame = !!input.frame && Object.keys(input.frame).length > 0
  if (!input.layers && !hasFrame && !input.controls?.length) {
    throw new Error(`The change on ${name} has no layers, frame patch or controls; send at least one.`)
  }
  const elsewhere = new Set(
    doc.layers.filter((l) => !(typeof l.artboard === "string" && bareId(l.artboard) === bareId(artboard))).map((l) => bareId(l.id)),
  )
  const seen = new Set<string>()
  const source: Array<Record<string, any>> = input.layers ?? layersOn(doc, artboard)
  const layers = source.map((layer) => {
    const id = String(layer.id ?? "")
    if (layer.artboard != null && bareId(String(layer.artboard)) !== bareId(artboard)) {
      throw new Error(
        `Layer ${id} is on artboard ${layer.artboard}, not ${name} (${artboard}). One change holds one artboard's layers; suggest the other artboard as its own change.`,
      )
    }
    if (seen.has(bareId(id))) throw new Error(`The layer id ${id} is used twice in ${name}'s layers.`)
    seen.add(bareId(id))
    if (elsewhere.has(bareId(id))) throw new Error(`The layer id ${id} is already used on another artboard; give the layer a new id.`)
    return { ...layer, id: shapeId(id), artboard, props: isObj(layer.props) ? layer.props : {} } as Artifacts.CanvasLayer
  })
  if (layers.length > MAX_ARTBOARD_LAYERS) throw new Error(`An artboard change holds at most ${MAX_ARTBOARD_LAYERS} layers.`)
  if (JSON.stringify(layers).length > MAX_ARTBOARD_JSON) throw new Error(`${name}'s layers are over 2 MB of JSON; simplify the frame.`)
  const frame = hasFrame ? input.frame : undefined
  const controls = input.controls?.length ? buildControls(input.controls, layers, name) : undefined
  return {
    kind: "artboard",
    artboard,
    name,
    ...(frame ? { frame } : {}),
    layers,
    before: { ...(frame ? { frame: framePart(board, Object.keys(frame)) } : {}), layers: layersOn(doc, artboard) },
    ...(controls ? { controls } : {}),
  }
}

const TaskInput = z.string().max(200).optional()

const ControlBase = {
  id: z.string().min(1).max(64).describe("Short id, unique in this change, e.g. radius"),
  label: z.string().min(1).max(200).describe("What the user sees, e.g. 'Corner radius'"),
  layer: z
    .string()
    .max(200)
    .nullable()
    .describe("A layer id in this change's layers, or null for the artboard itself"),
  prop: z
    .string()
    .min(1)
    .max(200)
    .describe("A key in the layer's props (radius, fill, ...), or x, y, w, h; frame.<key> targets the artboard's props (layer null)"),
}
const Control = z.discriminatedUnion("type", [
  z.object({
    ...ControlBase,
    type: z.literal("range"),
    min: z.number(),
    max: z.number(),
    step: z.number().positive().optional(),
    value: z.number().describe("The value in your layers now, between min and max"),
  }),
  z.object({ ...ControlBase, type: z.literal("color"), value: z.string().min(1).max(200).describe("A colour, e.g. #6366f1") }),
  z.object({
    ...ControlBase,
    type: z.literal("select"),
    options: z.array(z.string().min(1).max(200)).min(1).max(50),
    value: z.string().describe("One of options"),
  }),
])

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
  z.object({
    kind: z.literal("artboard"),
    artboard: z.string().min(1).max(200).describe("The artboard id from the design you read, e.g. shape:hero"),
    layers: z
      .array(
        z
          .object({
            id: z.string().min(1).max(200),
            type: z.string().min(1).max(100).describe("text, geo, design-component, design-uiblock, design-html, ..."),
            name: z.string().max(200).optional(),
            artboard: z.string().max(200).nullish().describe("This change's artboard; filled in when left out"),
            x: z.number().optional(),
            y: z.number().optional(),
            w: z.number().optional(),
            h: z.number().optional(),
            rotation: z.number().optional(),
            props: z.record(z.string(), z.unknown()).optional(),
          })
          .catchall(z.unknown()),
      )
      .max(2000)
      .optional()
      .describe(
        "The artboard's full new layer list, bottom first. Keep the ids of layers you keep; new layers get new ids. " +
          "Left out = the frame's layers as they are now (send controls to let the user tune them).",
      ),
    frame: z
      .object({
        fill: z.string().max(200).optional(),
        w: z.number().min(1).max(100_000).optional(),
        h: z.number().min(1).max(100_000).optional(),
        props: z.record(z.string(), z.unknown()).optional(),
      })
      .optional()
      .describe("Optional artboard patch (fill, size, props); left out = unchanged"),
    controls: z
      .array(Control)
      .max(12)
      .optional()
      .describe(
        "Sliders the user moves before keeping the change, when they are tuning something (spacing, radius, colour, density): " +
          "offer 2-5 instead of guessing one value",
      ),
    reason: z.string().max(500).optional().describe("One line the user sees, e.g. 'Made the eyebrow and heading dark'"),
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
    void heartbeat(ctx, params.id, { state: "editing", task: taskFor(ctx, params.id, params.task, "Saving") })
    const known = Artifacts.fromTranscript(await transcript(ctx), params.id)
    if (known && Artifacts.isKind(known.kind)) {
      const invalid = Artifacts.validate(known.kind, known.body_format, params.body)
      if (invalid) throw new Error(invalid)
    }
    if (!known || known.kind === "design") {
      // A whole-canvas save would overwrite frames other agents are changing.
      const others = ((await Artifacts.pendingSuggestions(params.id, ctx.abort)) ?? []).filter(
        (s) => s.change?.kind === "artboard" && s.session_id !== ctx.sessionID,
      )
      if (others.length) {
        const who = [...new Set(others.map((s) => `${s.agent_name} on ${(s.change as { name?: string; artboard: string }).name || (s.change as { artboard: string }).artboard}`))]
        throw new Error(
          `This design has ${others.length} frame change${others.length === 1 ? "" : "s"} from other agents waiting for review (${who.join(", ")}). ` +
            `artifact_update replaces the whole canvas and would overwrite them. Change one frame instead: artifact_claim the artboard, ` +
            `artifact_suggest an artboard change for it, then artifact_release.`,
        )
      }
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
    void heartbeat(ctx, params.id, { state: "viewing", task: taskFor(ctx, params.id, params.task, "Reading") })
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

/** The cloud keeps an agent's comment anchor up to this size. */
export const MAX_AGENT_ANCHOR_JSON = 4 * 1024

/**
 * The pin for an agent's comment on a Design canvas: the anchor the model
 * gave (EditorAnchor JSON), with `artboard` and `shapeId` resolved to the
 * frame's shape id. With only `artboard`, the pin is the frame itself
 * (`shapeId === artboard`). Throws when the artboard or layer isn't there.
 */
export function pinAnchor(
  doc: CanvasDoc,
  input: { anchor?: Record<string, unknown>; artboard?: string },
): Artifacts.CommentAnchor {
  const given = input.anchor ?? {}
  const boardRef = input.artboard ?? (typeof given.artboard === "string" ? given.artboard : undefined)
  const shapeRef = typeof given.shapeId === "string" && given.shapeId.trim() ? given.shapeId : undefined
  const layer = shapeRef ? doc.layers.find((l) => bareId(l.id) === bareId(shapeRef)) : undefined
  const board = boardRef
    ? findArtboard(doc, boardRef)
    : layer && typeof layer.artboard === "string"
      ? findArtboard(doc, layer.artboard)
      : shapeRef
        ? findArtboard(doc, shapeRef)
        : undefined
  if (boardRef && !board) throw new Error(`There's no artboard "${boardRef}" in this design. ${artboardList(doc)}`)
  if (shapeRef && !layer && !(board && bareId(board.id) === bareId(shapeRef))) {
    throw new Error(`There's no layer "${shapeRef}" in this design; pin the comment to a layer or artboard id from it.`)
  }
  if (!board) throw new Error(`Say which frame the comment is about: pass artboard (an artboard id). ${artboardList(doc)}`)
  if (layer && typeof layer.artboard === "string" && bareId(layer.artboard) !== bareId(board.id)) {
    throw new Error(`Layer ${shapeRef} is on artboard ${layer.artboard}, not ${frameName(board)} (${shapeId(board.id)}).`)
  }
  const label = typeof given.label === "string" && given.label.trim() ? given.label.trim() : layer?.name ?? frameName(board)
  return {
    ...given,
    kind: "element",
    shapeId: shapeId(layer ? layer.id : board.id),
    artboard: shapeId(board.id),
    label: String(label).slice(0, 200),
  }
}

export const ArtifactCommentTool = Tool.define("artifact_comment", {
  description:
    "Read an artifact's comment threads, or post a comment or a reply (parent_id). Use it to answer a comment that mentions you, " +
    "to leave a note where the user asked, or to ask the user a question about one frame of a design: pass artboard (and anchor " +
    "for a layer in it) and the question shows as a numbered pin on that frame, under your name. Leave body out to read.",
  parameters: z.object({
    id: z.string().min(1).describe("The artifact id (art_…)"),
    body: z.string().min(1).max(10_000).optional().describe("The comment text"),
    parent_id: z.string().optional().describe("Reply to this comment id"),
    anchor: z
      .object({ kind: z.enum(["text", "cell", "slide", "element"]) })
      .catchall(z.unknown())
      .optional()
      .describe(
        "Where the comment points. On a design: {kind:'element', shapeId:<layer id>, label?}; the frame comes from artboard. " +
          "Other kinds: {kind:'text', text}, {kind:'cell', sheet?, from:{row,col}, to:{row,col}}, {kind:'slide', slide}",
      ),
    artboard: z
      .string()
      .min(1)
      .max(200)
      .optional()
      .describe("Design canvases: the artboard id the question is about, e.g. shape:hero; pins the comment to that frame"),
    task: TaskInput.describe("Short line shown to the user while you work, e.g. 'Asking about the dates'"),
  }),
  async execute(params, ctx) {
    if (!params.body) {
      void heartbeat(ctx, params.id, { state: "viewing", task: taskFor(ctx, params.id, params.task, "Reading comments") })
      const items = await Artifacts.comments(params.id, ctx.abort)
      return { title: `${items.length} comments`, output: JSON.stringify(items, null, 2).slice(0, MAX_MODEL_BODY), metadata: { ok: true, items, comment: undefined as unknown, truncated: false } }
    }
    let anchor: Artifacts.CommentAnchor | undefined = params.anchor as Artifacts.CommentAnchor | undefined
    let frame: string | undefined
    if (!params.parent_id && (params.artboard || anchor?.kind === "element")) {
      // A pin on a Design canvas: resolve the frame (and layer) from the design.
      const known = Artifacts.fromTranscript(await transcript(ctx), params.id)
      const current = await Artifacts.read({ id: params.id, known }, ctx.abort)
      const doc = parseCanvas(current.body, current.body_format)
      if (doc) {
        anchor = pinAnchor(doc, { anchor: params.anchor as Record<string, unknown> | undefined, artboard: params.artboard })
        frame = String(findArtboard(doc, String(anchor.artboard))?.name ?? anchor.artboard)
      } else if (params.artboard) {
        throw new Error(
          current.kind === "design"
            ? `${params.id} is an HTML design, not a canvas, so it has no artboards to pin to. Leave artboard out.`
            : `artboard is for design canvases; ${params.id} is a ${current.kind}. Leave it out.`,
        )
      }
    }
    if (anchor && JSON.stringify(anchor).length > MAX_AGENT_ANCHOR_JSON) {
      throw new Error("The comment's anchor is over 4 KB of JSON; keep it to the ids and a short label.")
    }
    const name = await agentName(ctx)
    void heartbeat(ctx, params.id, { state: "viewing", task: taskFor(ctx, params.id, params.task, "Asking a question"), name })
    const posted = await Artifacts.comment(
      params.id,
      { body: params.body, parent_id: params.parent_id, ...(anchor ? { anchor } : {}), agent_name: name },
      ctx.abort,
    )
    return {
      title: "Comment posted",
      output:
        `Posted${params.parent_id ? " a reply" : " a comment"} on ${params.id}` +
        (frame ? `, pinned to ${frame}. The user sees a numbered pin on the frame and answers in the thread.` : "."),
      metadata: { ok: true, items: [] as any[], comment: posted as unknown, truncated: false },
    }
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
    case "artboard":
      throw new Error("An artboard change is built from the canvas (buildArtboardChange)")
  }
}

export const ArtifactSuggestTool = Tool.define("artifact_suggest", {
  description: SUGGEST_DESCRIPTION,
  parameters: z.object({
    id: z.string().min(1).describe("The artifact id (art_…); a doc, a sheet or a design canvas"),
    base_version: z.number().int().min(1).describe("The version these changes are based on"),
    changes: z
      .array(ChangeInput)
      .min(1)
      .max(50)
      .describe("The changes, in order: text, block or cell for docs and sheets; artboard (one per frame) for designs"),
    task: TaskInput.describe("Short line shown on your cursor while you work, e.g. 'Fixing the dates'"),
  }),
  async execute(params, ctx) {
    const [name, cost] = await Promise.all([agentName(ctx), sessionCost(ctx.sessionID)])
    let baseVersion = params.base_version
    const built = new Map<number, Artifacts.Change>()
    if (params.changes.some((c) => c.kind === "artboard")) {
      // Read the canvas now: `before` comes from it, and a frame someone else
      // changed since base_version must be read again, not overwritten.
      const known = Artifacts.fromTranscript(await transcript(ctx), params.id)
      const current = await Artifacts.read({ id: params.id, known }, ctx.abort)
      const doc = parseCanvas(current.body, current.body_format)
      if (current.kind !== "design" || !doc) {
        throw new Error(
          current.kind !== "design"
            ? `Artboard changes are for design artifacts; ${params.id} is a ${current.kind}.`
            : `${params.id} is an HTML design, not a canvas, so it has no artboards. The user can open it and choose Convert to canvas; until then use artifact_update.`,
        )
      }
      let baseDoc: CanvasDoc | undefined = doc
      if (current.version !== params.base_version) {
        const base = await Artifacts.read({ id: params.id, version: params.base_version }, ctx.abort).catch(() => undefined)
        baseDoc = base ? parseCanvas(base.body, base.body_format === "text/plain" ? current.body_format : base.body_format) : undefined
      }
      params.changes.forEach((item, i) => {
        if (item.kind !== "artboard") return
        const change = buildArtboardChange(doc, item)
        if (!baseDoc || !sameArtboard(baseDoc, doc, change.artboard)) {
          throw new Error(
            `${change.name} changed since version ${params.base_version} (the design is at version ${current.version}). ` +
              `Nothing was suggested. Call artifact_read again and rebuild ${change.name}'s layers from the latest body.`,
          )
        }
        built.set(i, change)
      })
      // Other frames may have moved on; this frame is as the agent read it.
      baseVersion = current.version
    }
    const changes = params.changes.map((item, i) => ({
      change: built.get(i) ?? normalizeChange(item),
      ...(item.reason?.trim() ? { reason: item.reason.trim() } : {}),
    }))
    const held = claimOf(ctx.sessionID, params.id)
    const onHeld = held && changes.some((c) => c.change.kind === "artboard" && c.change.artboard === held.artboard)
    void heartbeat(ctx, params.id, {
      state: "editing",
      task: taskFor(ctx, params.id, params.task, "Suggesting edits"),
      anchor: onHeld ? { kind: "artboard", artboard: held!.artboard } : Artifacts.anchorOf(changes[0]?.change),
      name,
      cost_usd: cost,
    })
    const items = await Artifacts.suggest(
      params.id,
      {
        base_version: baseVersion,
        agent_name: name,
        ...(ctx.sessionID ? { session_id: ctx.sessionID } : {}),
        ...(cost !== undefined ? { cost_usd: cost } : {}),
        changes,
      },
      ctx.abort,
    )
    const ids = items.map((s) => s.id)
    const frames = [...built.values()].map((c) => (c as { name?: string }).name).filter(Boolean)
    return {
      title: `${ids.length} suggestion${ids.length === 1 ? "" : "s"}`,
      output: [
        JSON.stringify({ id: params.id, base_version: baseVersion, suggestion_ids: ids }),
        frames.length
          ? `Left ${frames.length === 1 ? "a change" : `${frames.length} changes`} on ${frames.join(", ")} as ${name}. The canvas shows the new ` +
            `version "In review"; the user keeps or undoes it. Release the frame with artifact_release when you're done with it. ` +
            `Tell them what you changed in a sentence; don't list the layers.`
          : `Left ${ids.length} suggestion${ids.length === 1 ? "" : "s"} on ${params.id} as ${name}. Nothing is changed yet: ` +
            `the user reviews each one and accepts or rejects it. Tell them you left suggestions to review; don't repeat them.`,
      ].join("\n"),
      metadata: { ok: true, artifact_id: params.id, agent_name: name, suggestions: items, truncated: false },
    }
  },
})

export const ArtifactClaimTool = Tool.define("artifact_claim", {
  description: CLAIM_DESCRIPTION,
  parameters: z.object({
    artifact_id: z.string().min(1).describe("The design artifact id (art_…)"),
    artboard: z.string().min(1).max(200).describe("The artboard id from the design you read, e.g. shape:hero"),
    task: TaskInput.describe("Short line on your tag while you work on the frame, e.g. 'Dark restyle'"),
  }),
  async execute(params, ctx) {
    const id = params.artifact_id
    const known = Artifacts.fromTranscript(await transcript(ctx), id)
    const current = await Artifacts.read({ id, known }, ctx.abort)
    const doc = parseCanvas(current.body, current.body_format)
    if (current.kind !== "design" || !doc) {
      throw new Error(`artifact_claim works on design canvases; ${id} is ${current.kind === "design" ? "an HTML design" : `a ${current.kind}`}.`)
    }
    const board = findArtboard(doc, params.artboard)
    if (!board) throw new Error(`There's no artboard "${params.artboard}" in this design. ${artboardList(doc)}`)
    const frame = frameName(board)
    const artboard = shapeId(board.id)
    const task = params.task?.trim() || `Working on ${frame}`
    const [name, cost] = await Promise.all([agentName(ctx), sessionCost(ctx.sessionID)])
    await Artifacts.claim(
      id,
      { name, session_id: ctx.sessionID || undefined, task, anchor: { kind: "artboard", artboard }, cost_usd: cost },
      frame,
      ctx.abort,
    )
    const moved = hold({ artifactId: id, artboard, frame, task, name, sessionID: ctx.sessionID, touched: Date.now() })
    return {
      title: `Claimed ${frame}`,
      output: [
        JSON.stringify({ artifact_id: id, artboard, name: frame, version: current.version }),
        `You hold ${frame} (${artboard}) on ${id}${moved ? `; ${moved.frame} is released` : ""}. Other agents can't change it while you work. ` +
          `Next: artifact_suggest one artboard change for it with base_version ${current.version}, then artifact_release.`,
      ].join("\n"),
      metadata: { ok: true, artifact_id: id, artboard, frame, truncated: false },
    }
  },
})

export const ArtifactReleaseTool = Tool.define("artifact_release", {
  description: RELEASE_DESCRIPTION,
  parameters: z.object({
    artifact_id: z.string().min(1).describe("The design artifact id (art_…)"),
    artboard: z.string().max(200).optional().describe("The artboard you hold; left out = whichever you hold"),
  }),
  async execute(params, ctx) {
    const id = params.artifact_id
    const held = claimOf(ctx.sessionID, id)
    if (held && params.artboard && bareId(params.artboard) !== bareId(held.artboard)) {
      return {
        title: "Nothing released",
        output: `You don't hold ${params.artboard} on ${id}; you hold ${held.frame} (${held.artboard}). Nothing was released.`,
        metadata: { ok: true, artifact_id: id, released: false, truncated: false },
      }
    }
    if (held) dropClaim(held)
    const name = held?.name ?? (await agentName(ctx))
    // A heartbeat without an anchor clears the claim; the agent stays present until its TTL.
    await Artifacts.presence(id, "viewing", { name, session_id: ctx.sessionID || undefined, task: "Done" })
    const frame = held?.frame ?? params.artboard ?? "the frame"
    return {
      title: `Released ${frame}`,
      output: `Released ${frame} on ${id}. Other agents can work on it now.`,
      metadata: { ok: true, artifact_id: id, released: true, truncated: false },
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
  ArtifactClaimTool,
  ArtifactReleaseTool,
  ArtifactReadTool,
  ArtifactListTool,
  ArtifactDeleteTool,
  ArtifactShareTool,
  ArtifactCommentTool,
  ArtifactStorageTool,
]
