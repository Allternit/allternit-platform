/**
 * Artifacts v2 store client (docs/design/artifacts-v2.md §3, §5).
 *
 * The model tools `artifact_create` / `artifact_update` / `artifact_read`
 * write to the one account-level store in allternit-cloud-api
 * (`/api/v2/artifacts`). When this process has no cloud credential the cloud
 * rejects, or the network fails, the tools still succeed: gizzi mints the id
 * itself (`art_` + ULID) and returns the full payload with `persisted: false`,
 * and the app persists it with an idempotent create under the same id.
 *
 * Base URL: ALLTERNIT_CLOUD_API_URL, else GIZZI_PLATFORM_API_URL (the
 * cloud-api origin pairing already uses), else https://api.allternit.com.
 * Credentials, in order: ALLTERNIT_API_KEY, ALLTERNIT_API_TOKEN, then the
 * `gizzi login` / `gizzi pair` device token. GIZZI_ARTIFACTS_OFFLINE=1 skips
 * the cloud entirely (the app persists every artifact).
 */
import { ulid } from "ulid"
import { CLOUD_URLS } from "@/shared/constants/cloudUrls"
import { Pairing } from "@/runtime/services/pairing/pairing"
import type { MessageV2 } from "@/runtime/session/message-v2"
import { Log } from "@/shared/util/log"

export namespace Artifacts {
  const log = Log.create({ service: "artifacts" })

  /** kind → accepted body formats; the first is the default. Mirrors §1. */
  export const FORMATS = {
    doc: ["application/vnd.allternit.doc+json", "text/markdown"],
    sheet: ["application/vnd.allternit.sheet+json"],
    slides: ["application/vnd.allternit.slides+json"],
    design: ["application/vnd.allternit.design+json", "text/html"],
    dashboard: ["application/vnd.allternit.openui"],
    motion: ["application/vnd.allternit.motion+json"],
    page: ["text/html", "text/markdown"],
    card: ["application/vnd.allternit.openui"],
    diagram: ["text/vnd.mermaid", "image/svg+xml"],
    image: ["text/uri-list"],
    code: ["text/plain"],
  } as const satisfies Record<string, readonly string[]>

  export type Kind = keyof typeof FORMATS
  export const KINDS = Object.keys(FORMATS) as [Kind, ...Kind[]]

  /** What every artifact tool result carries in its metadata (the app renders and persists from it). */
  export interface Payload {
    id: string
    kind: string
    title: string
    icon?: string
    version: number
    body: string
    body_format: string
    meta: Record<string, unknown>
    /** true when the cloud store has this version; false = the app must persist it. */
    persisted: boolean
    /** Offline update: the version this one was written on top of. */
    base_version?: number
  }

  export class StaleVersionError extends Error {
    constructor(
      readonly id: string,
      readonly baseVersion: number,
      readonly currentVersion: number | undefined,
    ) {
      super(
        `Artifact ${id} has changed since version ${baseVersion}` +
          (currentVersion ? ` (it is now at version ${currentVersion})` : "") +
          `. Nothing was saved. Call artifact_read with id "${id}" to get the latest body, apply your change to it, ` +
          `then call artifact_update again with base_version ${currentVersion ?? "set to the version artifact_read returns"}.`,
      )
      this.name = "StaleVersionError"
    }
  }

  export class NotFoundError extends Error {
    constructor(readonly id: string) {
      super(
        `Artifact ${id} was not found, or you don't have access to it. Check the id (it starts with "art_"), ` +
          `or create a new artifact with artifact_create.`,
      )
      this.name = "NotFoundError"
    }
  }

  /** The cloud answered with a request error the model can fix (bad kind/format, too big…). */
  export class RejectedError extends Error {
    constructor(
      readonly status: number,
      message: string,
    ) {
      super(message)
      this.name = "RejectedError"
    }
  }

  export function mintId(): string {
    return `art_${ulid()}`
  }

  export function isKind(kind: string): kind is Kind {
    return Object.prototype.hasOwnProperty.call(FORMATS, kind)
  }

  /** Pick the body format when the model left it out. */
  export function inferFormat(kind: Kind, body: string): string {
    const text = body.trimStart()
    const looksJson = text.startsWith("{") || text.startsWith("[")
    switch (kind) {
      case "doc":
        return looksJson ? FORMATS.doc[0] : "text/markdown"
      case "design":
        return looksJson ? FORMATS.design[0] : "text/html"
      case "page":
        return text.startsWith("<") ? "text/html" : "text/markdown"
      case "diagram":
        return /^(<\?xml|<svg)/i.test(text) ? "image/svg+xml" : "text/vnd.mermaid"
      default:
        return FORMATS[kind][0]
    }
  }

  /** Returns an error message for the model, or undefined when the body/format pair is acceptable. */
  export function validate(kind: Kind, bodyFormat: string, body: string): string | undefined {
    const allowed = FORMATS[kind] as readonly string[]
    if (!allowed.includes(bodyFormat)) {
      return `body_format "${bodyFormat}" isn't valid for kind "${kind}". Use one of: ${allowed.join(", ")}.`
    }
    if (bodyFormat.endsWith("+json")) {
      try {
        JSON.parse(body)
      } catch (error) {
        return `The body must be JSON for ${bodyFormat} (${error instanceof Error ? error.message : "parse error"}). Use the format described in the system prompt.`
      }
    }
    if (bodyFormat === "text/uri-list" && !/^\S+:\/\/\S+|^\//m.test(body.trim())) {
      return `An image artifact's body is the image URL (text/uri-list), not the image data.`
    }
    return undefined
  }

  // ---------------------------------------------------------------- cloud

  export function cloudBase(): string {
    const explicit =
      process.env.ALLTERNIT_CLOUD_API_URL?.trim() || process.env.GIZZI_PLATFORM_API_URL?.trim() || CLOUD_URLS.api
    return explicit.replace(/\/+$/, "")
  }

  async function tokens(): Promise<string[]> {
    if (["1", "true"].includes((process.env.GIZZI_ARTIFACTS_OFFLINE ?? "").toLowerCase())) return []
    const env = [process.env.ALLTERNIT_API_KEY?.trim(), process.env.ALLTERNIT_API_TOKEN?.trim()]
    const stored = await Pairing.load().catch(() => undefined)
    const device = Pairing.tokenUsable(stored) ? stored.deviceToken : undefined
    return [...new Set([...env, device].filter((t): t is string => !!t))]
  }

  const TIMEOUT_MS = 10_000

  /** undefined = no usable cloud (no credential, rejected, missing route, network/server error): go offline. */
  async function cloud(
    method: "GET" | "POST",
    path: string,
    body: unknown,
    abort?: AbortSignal,
  ): Promise<{ status: number; json: any } | undefined> {
    const list = await tokens()
    if (list.length === 0) return undefined
    const signal = abort ? AbortSignal.any([abort, AbortSignal.timeout(TIMEOUT_MS)]) : AbortSignal.timeout(TIMEOUT_MS)
    for (const token of list) {
      let response: Response
      try {
        response = await fetch(`${cloudBase()}${path}`, {
          method,
          headers: {
            Authorization: `Bearer ${token}`,
            Accept: "application/json",
            ...(body === undefined ? {} : { "Content-Type": "application/json" }),
          },
          body: body === undefined ? undefined : JSON.stringify(body),
          signal,
        })
      } catch (error) {
        if (abort?.aborted) throw error
        log.warn("cloud artifacts unreachable; the app will persist", { path, error: String(error) })
        return undefined
      }
      // A stale env token shouldn't hide a good device token: try the next one.
      if (response.status === 401 || response.status === 403) continue
      if (response.status >= 500) {
        log.warn("cloud artifacts error; the app will persist", { path, status: response.status })
        return undefined
      }
      const text = await response.text().catch(() => "")
      let json: any = undefined
      try {
        json = text ? JSON.parse(text) : undefined
      } catch {
        json = { message: text }
      }
      return { status: response.status, json }
    }
    log.info("cloud rejected every artifact credential; the app will persist", { path })
    return undefined
  }

  function errorMessage(json: any, status: number): string {
    return (json && (json.message || json.error || json.detail)) || `HTTP ${status}`
  }

  /** A response that is our route's JSON (not some other server's 404 page). */
  function isArtifact(json: any): boolean {
    return !!json && typeof json === "object" && typeof json.id === "string"
  }

  function fromCloud(json: any, fallback: Partial<Payload>): Payload {
    const v = json.version ?? {}
    return {
      id: json.id,
      kind: json.kind ?? fallback.kind ?? "page",
      title: json.title ?? fallback.title ?? "",
      icon: json.icon ?? fallback.icon,
      version: typeof v.version === "number" ? v.version : (json.current_version ?? fallback.version ?? 1),
      body: typeof v.body === "string" ? v.body : (fallback.body ?? ""),
      body_format: v.body_format ?? fallback.body_format ?? "text/plain",
      meta: v.meta ?? fallback.meta ?? {},
      persisted: true,
    }
  }

  export interface CreateInput {
    kind: Kind
    title: string
    body: string
    body_format: string
    icon?: string
    meta?: Record<string, unknown>
    origin?: Record<string, unknown>
  }

  export async function create(input: CreateInput, abort?: AbortSignal): Promise<Payload> {
    const id = mintId()
    const local: Payload = {
      id,
      kind: input.kind,
      title: input.title,
      icon: input.icon,
      version: 1,
      body: input.body,
      body_format: input.body_format,
      meta: input.meta ?? {},
      persisted: false,
    }
    const res = await cloud(
      "POST",
      "/api/v2/artifacts",
      {
        id,
        kind: input.kind,
        title: input.title,
        icon: input.icon,
        origin: input.origin ?? {},
        body: input.body,
        body_format: input.body_format,
        meta: input.meta ?? {},
        runtime_version: 2,
      },
      abort,
    )
    if (!res) return local
    if (res.status === 200 || res.status === 201) {
      return isArtifact(res.json) ? fromCloud(res.json, local) : local
    }
    // The route isn't deployed on this cloud yet: the app persists.
    if (res.status === 404 || res.status === 405) return local
    throw new RejectedError(res.status, `The artifact store refused it: ${errorMessage(res.json, res.status)}`)
  }

  export interface UpdateInput {
    id: string
    body: string
    base_version: number
    note?: string
    /** From the transcript when known, so an offline result is complete. */
    known?: Payload
  }

  export async function update(input: UpdateInput, abort?: AbortSignal): Promise<Payload> {
    const meta: Record<string, unknown> = { ...(input.known?.meta ?? {}) }
    delete meta.note
    if (input.note) meta.note = input.note
    const local: Payload = {
      id: input.id,
      kind: input.known?.kind ?? "page",
      title: input.known?.title ?? "",
      icon: input.known?.icon,
      version: input.base_version + 1,
      body: input.body,
      body_format: input.known?.body_format ?? "text/plain",
      meta,
      persisted: false,
      base_version: input.base_version,
    }
    if (input.known && !input.known.persisted && input.known.version !== input.base_version) {
      throw new StaleVersionError(input.id, input.base_version, input.known.version)
    }
    const res = await cloud(
      "POST",
      `/api/v2/artifacts/${encodeURIComponent(input.id)}/versions`,
      {
        base_version: input.base_version,
        body: input.body,
        ...(input.known?.body_format ? { body_format: input.known.body_format } : {}),
        meta,
        author: "assistant",
      },
      abort,
    )
    if (!res) return local
    if (res.status === 200 || res.status === 201) {
      return isArtifact(res.json) ? fromCloud(res.json, local) : local
    }
    if (res.status === 409) {
      const current = typeof res.json?.current_version === "number" ? res.json.current_version : undefined
      throw new StaleVersionError(input.id, input.base_version, current)
    }
    if (res.status === 404 || res.status === 405) {
      // Not in the cloud: either the route isn't deployed or the app hasn't
      // persisted this session's artifact yet. If this session made it, the
      // app persists the new version; otherwise it really doesn't exist.
      if (input.known) return local
      throw new NotFoundError(input.id)
    }
    throw new RejectedError(res.status, `The artifact store refused the update: ${errorMessage(res.json, res.status)}`)
  }

  export async function read(
    input: { id: string; version?: number; known?: Payload },
    abort?: AbortSignal,
  ): Promise<Payload> {
    const path = input.version
      ? `/api/v2/artifacts/${encodeURIComponent(input.id)}/versions/${input.version}`
      : `/api/v2/artifacts/${encodeURIComponent(input.id)}`
    const res = await cloud("GET", path, undefined, abort)
    if (res && res.status === 200) {
      if (input.version) {
        // The version route returns just the version; title/kind come from the transcript when known.
        const v = res.json ?? {}
        return {
          id: input.id,
          kind: input.known?.kind ?? "page",
          title: input.known?.title ?? "",
          icon: input.known?.icon,
          version: typeof v.version === "number" ? v.version : input.version,
          body: typeof v.body === "string" ? v.body : "",
          body_format: v.body_format ?? input.known?.body_format ?? "text/plain",
          meta: v.meta ?? {},
          persisted: true,
        }
      }
      if (isArtifact(res.json)) return fromCloud(res.json, {})
    }
    if (res && res.status !== 404 && res.status !== 405 && res.status !== 200) {
      throw new RejectedError(res.status, `The artifact store refused the read: ${errorMessage(res.json, res.status)}`)
    }
    if (input.known && (!input.version || input.version === input.known.version)) return input.known
    throw new NotFoundError(input.id)
  }

  // ----------------------------------------------------------- transcript

  const TOOL_IDS = new Set(["artifact_create", "artifact_update", "artifact_read"])

  /** The newest version of an artifact this session's tool calls produced or read. */
  export function fromTranscript(messages: MessageV2.WithParts[] | undefined, id: string): Payload | undefined {
    let found: Payload | undefined
    for (const message of messages ?? []) {
      for (const part of message.parts ?? []) {
        if (part.type !== "tool" || !TOOL_IDS.has(part.tool) || part.state.status !== "completed") continue
        const artifact = part.state.metadata?.artifact as Payload | undefined
        if (!artifact || artifact.id !== id || typeof artifact.body !== "string") continue
        if (!found || artifact.version >= found.version) found = artifact
      }
    }
    return found
  }
}
