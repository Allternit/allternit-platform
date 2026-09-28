import type {
  Approval,
  ApprovalScopeRequest,
  ClaimRequest,
  CompleteRequest,
  CreatePrincipalRequest,
  CreatePrincipalResponse,
  Dag,
  DagNode,
  DelegationRule,
  EnsureWorkerPrincipalResponse,
  IntentEnvelope,
  IntentView,
  Json,
  JobView,
  Lease,
  LeaseGrant,
  MemoryEntry,
  Principal,
  PrincipalId,
  StoreMemoryRequest,
} from "./types"

export class AError extends Error {
  constructor(
    readonly status: number,
    readonly body: unknown,
    message: string,
  ) {
    super(message)
    this.name = "AError"
  }
}

export interface AClientOptions {
  /** API origin, e.g. `http://127.0.0.1:8013` (the `/api/v1` prefix is added). */
  baseUrl: string
  /** Bearer token: a user session or a principal token from provisioning. */
  token?: string | (() => string | undefined | Promise<string | undefined>)
  fetch?: typeof fetch
}

const enc = encodeURIComponent

function qs(params: Record<string, string | number | boolean | undefined | null>): string {
  const p = new URLSearchParams()
  for (const [k, v] of Object.entries(params)) if (v !== undefined && v !== null) p.set(k, String(v))
  const s = p.toString()
  return s ? `?${s}` : ""
}

/**
 * Typed client for the A:// surface. One instance per caller identity:
 * a user (app, CLI) or a worker principal (claims and runs jobs).
 */
export class AClient {
  private readonly base: string
  private readonly doFetch: typeof fetch

  constructor(private readonly options: AClientOptions) {
    this.base = `${options.baseUrl.replace(/\/+$/, "")}/api/v1`
    this.doFetch = options.fetch ?? fetch
  }

  async request<T>(method: string, path: string, body?: unknown): Promise<T> {
    const token = typeof this.options.token === "function" ? await this.options.token() : this.options.token
    const res = await this.doFetch(`${this.base}${path}`, {
      method,
      headers: {
        Accept: "application/json",
        ...(body !== undefined ? { "Content-Type": "application/json" } : {}),
        ...(token ? { Authorization: `Bearer ${token}` } : {}),
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    })
    const text = await res.text()
    const data = text ? safeJson(text) : undefined
    if (!res.ok) {
      const msg = (data && typeof data === "object" && "error" in data ? String((data as { error: unknown }).error) : text) || res.statusText
      throw new AError(res.status, data ?? text, `${method} ${path}: ${res.status} ${msg}`)
    }
    return data as T
  }

  readonly principals = {
    list: (workspace?: string) => this.request<Principal[] | { principals: Principal[] }>("GET", `/fabric/transport/principals${qs({ workspace })}`).then(unwrap<Principal>("principals")),
    create: (req: CreatePrincipalRequest) => this.request<CreatePrincipalResponse>("POST", "/fabric/transport/principals", req),
    setCapabilities: (id: PrincipalId, capabilities: string[]) =>
      this.request<Json>("PUT", `/fabric/transport/principals/${enc(id)}/capabilities`, { capabilities }),
    provisionToken: (id: PrincipalId) => this.request<{ token: string; [k: string]: unknown }>("POST", `/fabric/transport/principals/${enc(id)}/provision-token`),
    /** A local worker principal for this machine, created on first use. */
    ensureLocalWorker: (workspace?: string, kind?: string) =>
      this.request<EnsureWorkerPrincipalResponse>("POST", "/fabric/transport/local/ensure-worker-principal", { workspace, kind }),
  }

  readonly intents = {
    submit: (envelope: IntentEnvelope) => this.request<IntentView>("POST", "/fabric/transport/intents", envelope),
    get: (id: string) => this.request<IntentView>("GET", `/fabric/transport/intents/${enc(id)}`),
  }

  /** Worker side: claim a job, keep its lease alive, finish it. */
  readonly jobs = {
    claim: (req: ClaimRequest = {}) => this.request<LeaseGrant | null>("POST", "/fabric/transport/claim", req),
    get: (id: string) => this.request<JobView>("GET", `/fabric/transport/jobs/${enc(id)}`),
    heartbeat: (id: string, lease: Lease, workerTime?: string) =>
      this.request<Json>("POST", `/fabric/transport/jobs/${enc(id)}/heartbeat`, { ...lease, worker_time: workerTime }),
    renew: (id: string, lease: Lease, leaseTtlSecs?: number) =>
      this.request<Json>("POST", `/fabric/transport/jobs/${enc(id)}/renew`, { ...lease, lease_ttl_secs: leaseTtlSecs }),
    complete: (id: string, req: CompleteRequest) => this.request<Json>("POST", `/fabric/transport/jobs/${enc(id)}/complete`, req),
    continueInCloud: (id: string) => this.request<Json>("POST", `/fabric/transport/jobs/${enc(id)}/continue-in-cloud`),
    checkApproval: (id: string, req: ApprovalScopeRequest) =>
      this.request<{ approved: boolean; [k: string]: unknown }>("POST", `/fabric/transport/jobs/${enc(id)}/approvals/check`, req),
    requestApproval: (id: string, req: ApprovalScopeRequest) => this.request<Approval>("POST", `/fabric/transport/jobs/${enc(id)}/approvals/request`, req),
  }

  readonly runs = {
    continueInCloud: (runId: string) => this.request<Json>("POST", `/fabric/transport/runs/${enc(runId)}/continue-in-cloud`),
  }

  readonly approvals = {
    list: (q: { workspace?: string; status?: string } = {}) =>
      this.request<Approval[] | { approvals: Approval[] }>("GET", `/fabric/transport/approvals${qs(q)}`).then(unwrap<Approval>("approvals")),
    get: (id: string) => this.request<Approval>("GET", `/fabric/transport/approvals/${enc(id)}`),
    grant: (id: string) => this.request<Approval>("POST", `/fabric/transport/approvals/${enc(id)}/grant`),
    deny: (id: string) => this.request<Approval>("POST", `/fabric/transport/approvals/${enc(id)}/deny`),
  }

  readonly delegation = {
    list: (workspace?: string) =>
      this.request<DelegationRule[] | { rules: DelegationRule[] }>("GET", `/fabric/transport/delegation-rules${qs({ workspace })}`).then(unwrap<DelegationRule>("rules")),
    upsert: (rule: DelegationRule) => this.request<Json>("POST", "/fabric/transport/delegation-rules", rule),
    remove: (workspace: string, actionType: string) =>
      this.request<Json>("DELETE", `/fabric/transport/delegation-rules/${enc(workspace)}/${enc(actionType)}`),
  }

  /** The canonical DAG: plans, nodes, execution. */
  readonly dags = {
    list: () => this.request<Dag[] | { dags: Dag[] }>("GET", "/dags").then(unwrap<Dag>("dags")),
    nodes: (dagId: string) => this.request<DagNode[] | { nodes: DagNode[] }>("GET", `/dags/${enc(dagId)}/nodes`).then(unwrap<DagNode>("nodes")),
    node: (dagId: string, nodeId: string) => this.request<DagNode>("GET", `/dags/${enc(dagId)}/nodes/${enc(nodeId)}`),
    execute: (dagId: string) => this.request<Json>("POST", `/dags/${enc(dagId)}/execute`),
    render: (dagId: string) => this.request<Json>("GET", `/dags/${enc(dagId)}/render`),
    forSession: (sessionId: string) => this.request<Json>("GET", `/cowork/sessions/${enc(sessionId)}/dag`),
  }

  /** Principal-scoped memory with explicit grants (default-deny). */
  readonly memory = {
    list: (q: { principal?: PrincipalId; bot?: string; owned?: boolean } = {}) =>
      this.request<{ memories: MemoryEntry[] }>("GET", `/cowork/memory${qs(q)}`).then((r) => r.memories),
    store: (req: StoreMemoryRequest) => this.request<{ memory: { id: string } }>("POST", "/cowork/memory", req).then((r) => r.memory.id),
    forget: (id: string) => this.request<void>("DELETE", `/cowork/memory/${enc(id)}`),
    search: (query: string, limit?: number) => this.request<{ memories?: MemoryEntry[]; [k: string]: unknown }>("POST", "/cowork/memory/search", { query, limit }),
  }
}

function safeJson(text: string): unknown {
  try {
    return JSON.parse(text)
  } catch {
    return text
  }
}

function unwrap<T>(key: string) {
  return (r: T[] | Record<string, T[]>): T[] => (Array.isArray(r) ? r : (r?.[key] ?? []))
}
