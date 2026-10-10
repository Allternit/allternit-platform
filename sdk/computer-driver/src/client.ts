// Plain client for the Allternit hosted computer API (/v1/computers).
// Standard fetch only; every adapter in this package goes through it.

export const DEFAULT_BASE_URL = "https://api.allternit.com"

export type ToolsetName = "computer" | "browser"
export type CoordinateSpace = "pixels" | "normalized_1000"
export type ComputerStatus = "provisioning" | "running" | "stopped" | "starting" | "stopping" | "error" | "deleted"

export interface Computer {
  id: string
  object: "computer"
  name: string | null
  status: ComputerStatus
  account_id: string | null
  key_id: string
  created_at: string
  started_at: string | null
  metadata: Record<string, unknown>
}

export interface Page<T> {
  data: T[]
  has_more: boolean
  next_cursor: string | null
}

export type ContentBlock =
  | { type: "text"; text: string }
  | { type: "image"; media_type: string; data: string }

export interface Screen {
  width: number
  height: number
  scale: number
  frame_width: number
  frame_height: number
}

export interface ToolsetResult {
  is_error: boolean
  content: ContentBlock[]
  browser_state?: unknown
  screen?: Screen
  error?: unknown
}

export interface ToolsetCall {
  toolset: ToolsetName
  member: string
  input?: Record<string, unknown>
  run_id?: string
  turn_id?: string
  call_index?: number
  model_frame?: { width: number; height: number }
  coordinate_space?: CoordinateSpace
  approval_grant?: string
  browser_session_id?: string
  /** Off-by-default members (file_upload, read_console, read_network, javascript_exec) to allow for this call. */
  enable?: string[]
}

export interface Approval {
  id: string
  action_hash: string
  member: string
  toolset: ToolsetName
  risk: string
  confirmation_class: string
  approve_url: string
}

export interface ComputerEvent {
  id: string
  type: "computer.action"
  ts: string
  data: Record<string, unknown>
}

export interface ApiErrorBody {
  type: string
  code: string
  message: string
  param: string | null
}

export class AllternitApiError extends Error {
  readonly status: number
  readonly type: string
  readonly code: string
  readonly param: string | null
  readonly body: unknown
  constructor(status: number, err: Partial<ApiErrorBody>, body: unknown) {
    super(err.message ?? `Allternit API error ${status}`)
    this.name = "AllternitApiError"
    this.status = status
    this.type = err.type ?? "api_error"
    this.code = err.code ?? "unknown"
    this.param = err.param ?? null
    this.body = body
  }
}

/** 409 approval_required: the call was held until someone approves it. */
export class ApprovalRequiredError extends AllternitApiError {
  readonly approval: Approval
  readonly result: ToolsetResult
  constructor(err: Partial<ApiErrorBody>, approval: Approval, result: ToolsetResult, body: unknown) {
    super(409, err, body)
    this.name = "ApprovalRequiredError"
    this.approval = approval
    this.result = result
  }
}

/** 423 computer_busy / computer_controlled_elsewhere: someone else holds the computer right now; retry shortly or request_human. */
export class ComputerBusyError extends AllternitApiError {
  constructor(status: number, err: Partial<ApiErrorBody>, body: unknown) {
    super(status, err, body)
    this.name = "ComputerBusyError"
  }
}

/** 409 sandbox_required: the call only runs on a sandbox (cloud/bot) computer. */
export class SandboxRequiredError extends AllternitApiError {
  constructor(status: number, err: Partial<ApiErrorBody>, body: unknown) {
    super(status, err, body)
    this.name = "SandboxRequiredError"
  }
}

/** 409 computer_conflict: the call conflicts with another subtask or lease on the computer. */
export class ComputerConflictError extends AllternitApiError {
  constructor(status: number, err: Partial<ApiErrorBody>, body: unknown) {
    super(status, err, body)
    this.name = "ComputerConflictError"
  }
}

/** Maps a toolset error body onto the typed error for its code, if one exists. */
function typedApiError(status: number, err: Partial<ApiErrorBody>, body: unknown): AllternitApiError {
  switch (err.code) {
    case "computer_busy":
    case "computer_controlled_elsewhere":
      return new ComputerBusyError(status, err, body)
    case "sandbox_required":
      return new SandboxRequiredError(status, err, body)
    case "computer_conflict":
      return new ComputerConflictError(status, err, body)
    default:
      return new AllternitApiError(status, err, body)
  }
}

export interface ClientOptions {
  /** Project key (alt_live_… / alt_test_…). Defaults to ALLTERNIT_API_KEY. */
  apiKey?: string
  baseUrl?: string
  fetch?: typeof fetch
}

export class AllternitComputers {
  readonly baseUrl: string
  readonly #apiKey: string
  readonly #fetch: typeof fetch

  constructor(opts: ClientOptions = {}) {
    const env = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env
    const key = opts.apiKey ?? env?.ALLTERNIT_API_KEY
    if (!key) throw new Error("AllternitComputers: apiKey is required (or set ALLTERNIT_API_KEY).")
    this.#apiKey = key
    this.baseUrl = (opts.baseUrl ?? env?.ALLTERNIT_BASE_URL ?? DEFAULT_BASE_URL).replace(/\/+$/, "")
    this.#fetch = opts.fetch ?? globalThis.fetch.bind(globalThis)
  }

  create(body: { name?: string; account_id?: string; metadata?: Record<string, unknown> } = {}): Promise<Computer> {
    return this.#req("POST", "/v1/computers", body)
  }
  list(q: { limit?: number; after?: string } = {}): Promise<Page<Computer>> {
    return this.#req("GET", `/v1/computers${qs(q)}`)
  }
  get(id: string): Promise<Computer> {
    return this.#req("GET", `/v1/computers/${enc(id)}`)
  }
  start(id: string): Promise<Computer> {
    return this.#req("POST", `/v1/computers/${enc(id)}/start`, {})
  }
  stop(id: string): Promise<Computer> {
    return this.#req("POST", `/v1/computers/${enc(id)}/stop`, {})
  }
  delete(id: string): Promise<Computer> {
    return this.#req("DELETE", `/v1/computers/${enc(id)}`)
  }
  /** Run one toolset member. Action failures resolve with is_error:true; a held call throws ApprovalRequiredError. */
  toolset(id: string, call: ToolsetCall): Promise<ToolsetResult> {
    return this.#req("POST", `/v1/computers/${enc(id)}/toolset`, call)
  }
  /**
   * Run one toolset member, answering a 409 approval_required hold. When the
   * server holds the call and `onApproval` returns true, the approval is
   * granted (POST /approvals/{id}) and the same call resent with the
   * single-use `approval_grant`. When `onApproval` is missing or returns
   * false, the held result resolves (is_error:true) so the model sees it.
   */
  async toolsetWithApproval(id: string, call: ToolsetCall, onApproval?: (approval: Approval) => boolean | Promise<boolean>): Promise<ToolsetResult> {
    try {
      return await this.toolset(id, call)
    } catch (e) {
      if (!(e instanceof ApprovalRequiredError)) throw e
      if (!onApproval || !(await onApproval(e.approval))) return e.result
      await this.approve(id, e.approval.id)
      return this.toolset(id, { ...call, approval_grant: e.approval.id })
    }
  }
  schema(id: string, toolset: ToolsetName = "computer"): Promise<Record<string, unknown>> {
    return this.#req("GET", `/v1/computers/${enc(id)}/toolset/schema${qs({ toolset })}`)
  }
  events(id: string, q: { after?: string; limit?: number } = {}): Promise<Page<ComputerEvent>> {
    return this.#req("GET", `/v1/computers/${enc(id)}/events${qs(q)}`)
  }
  approve(id: string, approvalId: string): Promise<{ approval_id: string; approved: true; approval_grant: string }> {
    return this.#req("POST", `/v1/computers/${enc(id)}/approvals/${enc(approvalId)}`, {})
  }

  async #req<T>(method: string, path: string, body?: unknown): Promise<T> {
    const headers: Record<string, string> = { Authorization: `Bearer ${this.#apiKey}`, Accept: "application/json" }
    if (body !== undefined) headers["Content-Type"] = "application/json"
    if (method === "POST") headers["Idempotency-Key"] = crypto.randomUUID()
    const res = await this.#fetch(this.baseUrl + path, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    })
    const text = await res.text()
    let json: any = undefined
    try {
      json = text ? JSON.parse(text) : undefined
    } catch {
      json = { error: { message: text } }
    }
    if (res.ok) return json as T
    const err: Partial<ApiErrorBody> = json?.error ?? {}
    if (res.status === 409 && err.code === "approval_required" && json?.approval) {
      throw new ApprovalRequiredError(err, json.approval, json.result ?? { is_error: true, content: [] }, json)
    }
    throw typedApiError(res.status, err, json)
  }
}

/** Text of a result, joined. */
export function resultText(r: ToolsetResult): string {
  return r.content.flatMap((b) => (b.type === "text" ? [b.text] : [])).join("\n")
}

/** First image of a result, if any. */
export function resultImage(r: ToolsetResult): { media_type: string; data: string } | undefined {
  const img = r.content.find((b) => b.type === "image")
  return img && img.type === "image" ? { media_type: img.media_type, data: img.data } : undefined
}

function enc(s: string): string {
  return encodeURIComponent(s)
}

function qs(q: Record<string, string | number | undefined>): string {
  const p = Object.entries(q).filter(([, v]) => v !== undefined && v !== "")
  return p.length ? "?" + new URLSearchParams(p.map(([k, v]) => [k, String(v)])).toString() : ""
}
