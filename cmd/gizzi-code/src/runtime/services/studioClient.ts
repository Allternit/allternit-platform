/**
 * Client for the Freeweights Studio host API (/api/v1/studio on the computer's
 * allternit-api). Shapes follow services/studio-spec/openapi/studio-api.yaml.
 */
import { getAllternitApiConfig } from "@/runtime/services/api/allternitApi"

export type JobState = "queued" | "preparing" | "running" | "exporting" | "succeeded" | "failed" | "cancelled"
export const TERMINAL_STATES: JobState[] = ["succeeded", "failed", "cancelled"]

/** The host's verdict for this computer (studio-spec fit object). */
export interface EngineFit {
  ok: boolean
  hardware?: string
  backend?: string
  reason?: string
  warning?: string | null
  methods_unavailable?: Record<string, string>
  outputs_unavailable?: Record<string, string>
}

export interface EngineInfo {
  id: string
  name: string
  license: string
  version: string
  methods: string[]
  hardware: string[]
  fit?: EngineFit
  notes?: string
  install_state: "not_installed" | "installing" | "installed" | "failed"
}
export interface JobOutput { kind: string; path: string; bytes?: number }
export interface Job {
  id: string
  state: JobState
  spec: Record<string, any>
  created_at: string
  updated_at?: string | null
  error?: string | null
  last_seq?: number
  outputs?: JobOutput[]
}
export interface JobEvent { seq: number; at: string; kind: "state" | "log" | "progress" | "output" | "error"; message: string; progress?: number }
export interface EventsPage { state: JobState; events: JobEvent[] }

/** A host error: always {error, message} on the wire. */
export class StudioError extends Error {
  constructor(public status: number, public code: string, message: string) {
    super(message)
  }
}

export interface StudioClient {
  engines(): Promise<EngineInfo[]>
  install(id: string): Promise<EngineInfo>
  createJob(spec: unknown): Promise<Job>
  jobs(): Promise<Job[]>
  job(id: string): Promise<Job>
  events(id: string, after: number): Promise<EventsPage>
  cancel(id: string): Promise<Job>
  outputs(id: string): Promise<JobOutput[]>
  register(id: string, kind: string, name?: string): Promise<{ model_id?: string }>
}

export function createStudioClient(opts?: { base?: string; token?: string }): StudioClient {
  const cfg = opts?.base ? undefined : getAllternitApiConfig()
  const base = (opts?.base ?? cfg!.baseUrl).replace(/\/+$/, "") + "/api/v1/studio"
  const token = opts?.token ?? cfg?.token
  async function call<T>(path: string, init?: { method?: string; body?: unknown }): Promise<T> {
    let res: Response
    try {
      res = await fetch(base + path, {
        method: init?.method ?? "GET",
        headers: {
          "content-type": "application/json",
          ...(token ? { authorization: `Bearer ${token}` } : {}),
        },
        body: init?.body === undefined ? undefined : JSON.stringify(init.body),
      })
    } catch (e) {
      throw new StudioError(0, "unreachable", `Cannot reach the computer's Allternit API at ${base} (${e instanceof Error ? e.message : e}). Set ALLTERNIT_API_URL if it runs elsewhere.`)
    }
    const text = await res.text()
    let data: any = undefined
    try { data = text ? JSON.parse(text) : undefined } catch {}
    if (!res.ok) throw new StudioError(res.status, data?.error ?? `http_${res.status}`, data?.message ?? (text || res.statusText))
    return data as T
  }
  const e = encodeURIComponent
  return {
    engines: async () => (await call<{ engines: EngineInfo[] }>("/engines")).engines,
    install: (id) => call(`/engines/${e(id)}/install`, { method: "POST", body: {} }),
    createJob: (spec) => call("/jobs", { method: "POST", body: spec }),
    jobs: async () => (await call<{ jobs: Job[] }>("/jobs")).jobs,
    job: (id) => call(`/jobs/${e(id)}`),
    events: (id, after) => call(`/jobs/${e(id)}/events?after=${after}`),
    cancel: (id) => call(`/jobs/${e(id)}/cancel`, { method: "POST", body: {} }),
    outputs: async (id) => (await call<{ outputs: JobOutput[] }>(`/jobs/${e(id)}/outputs`)).outputs,
    register: (id, kind, name) => call(`/jobs/${e(id)}/outputs/register`, { method: "POST", body: { kind, ...(name ? { name } : {}) } }),
  }
}

/** Exit code for a finished job: 0 succeeded, 1 failed or cancelled. */
export function exitCodeForState(state: JobState): number {
  return state === "succeeded" ? 0 : 1
}

/** Exit code for an error: 2 when the host refused the spec (422), else 1. */
export function exitCodeForError(err: unknown): number {
  return err instanceof StudioError && err.status === 422 ? 2 : 1
}

/**
 * Follow a job's events. Pages with ?after=<last seq> so no event repeats,
 * stops at a terminal state once the page that reported it is drained, or when
 * `signal` aborts (detach). Returns the last known state.
 */
export async function followJob(
  client: Pick<StudioClient, "events">,
  id: string,
  opts: { after?: number; intervalMs?: number; signal?: AbortSignal; onEvent: (ev: JobEvent) => void; follow?: boolean },
): Promise<{ state: JobState; last: number; detached: boolean }> {
  let last = opts.after ?? 0
  let state: JobState = "queued"
  const interval = opts.intervalMs ?? 1000
  for (;;) {
    if (opts.signal?.aborted) return { state, last, detached: true }
    const page = await client.events(id, last)
    state = page.state
    for (const ev of page.events) {
      if (ev.seq <= last) continue
      last = ev.seq
      opts.onEvent(ev)
    }
    if (opts.follow === false || TERMINAL_STATES.includes(state)) {
      // A terminal page may race the final events; drain once more to be sure.
      if (opts.follow !== false && TERMINAL_STATES.includes(state) && page.events.length > 0) continue
      return { state, last, detached: false }
    }
    await new Promise<void>((r) => {
      const t = setTimeout(r, interval)
      opts.signal?.addEventListener("abort", () => { clearTimeout(t); r() }, { once: true })
    })
  }
}

// Fallback for old hosts that send no `fit`. Hosts with `fit` decide for themselves.
const HW_FOR_HOST: Record<string, string> = {
  "darwin:arm64": "apple-silicon",
  "linux:x64": "cuda-linux",
  "linux:arm64": "cuda-linux",
  "win32:x64": "cuda-windows",
}

// Same wording as the web (allternit-ai src/lib/model-lab/studio-jobs.ts).
export const HARDWARE_LABELS: Record<string, string> = {
  "apple-silicon": "Mac with Apple Silicon",
  "cuda-linux": "Linux with NVIDIA",
  "cuda-windows": "Windows with NVIDIA",
  "rocm-linux": "Linux with AMD (ROCm)",
  "xpu-linux": "Linux with Intel GPU",
  "xpu-windows": "Windows with Intel GPU",
  cpu: "CPU only",
}
export const BACKEND_LABELS: Record<string, string> = {
  mlx: "Runs with MLX",
  "torch-mps": "Runs with PyTorch (Metal)",
  "torch-cuda": "Runs with PyTorch (CUDA)",
  "torch-rocm": "Runs with PyTorch (ROCm)",
  "torch-xpu": "Runs with PyTorch (Intel XPU)",
  "torch-cpu": "Runs with PyTorch (CPU)",
}
export const UNTESTED_LABEL = "Not yet tested by Allternit on this hardware"
/** Hardware Allternit has run real jobs on. Empty until a run is recorded. */
export const TESTED_HARDWARE: ReadonlySet<string> = new Set<string>()

export const hardwareLabel = (id: string | undefined): string => (id && HARDWARE_LABELS[id]) || id || "unknown hardware"
export const backendLabel = (id: string | undefined): string | undefined => (id ? BACKEND_LABELS[id] ?? `Runs with ${id}` : undefined)

/** Plain-words lines for an engine card: hardware, backend, untested label, warning, refused methods/outputs. */
export function fitLines(fit: EngineFit | undefined): string[] {
  if (!fit) return []
  const lines: string[] = []
  if (fit.hardware) lines.push(hardwareLabel(fit.hardware))
  if (!fit.ok) {
    if (fit.reason) lines.push(`Refused: ${fit.reason}`)
    return lines
  }
  const b = backendLabel(fit.backend)
  if (b) lines.push(b)
  if (fit.hardware && !TESTED_HARDWARE.has(fit.hardware)) lines.push(UNTESTED_LABEL)
  if (fit.warning) lines.push(`Warning: ${fit.warning}`)
  for (const [m, r] of Object.entries(fit.methods_unavailable ?? {})) lines.push(`Not ${m}: ${r}`)
  for (const [o, r] of Object.entries(fit.outputs_unavailable ?? {})) lines.push(`No ${o} output: ${r}`)
  return lines
}

/** A host refusal in a plain sentence, using the host's reason. */
export function plainRefusal(code: string, message: string): string {
  const m = message.trim().replace(/\.$/, "")
  switch (code) {
    case "hardware_not_supported": return `This computer can't run that engine. ${m}.`
    case "method_not_supported": return `This computer can't train with that method. ${m}.`
    case "backend_not_supported": return `This computer can't use that backend. ${m}.`
    case "output_not_supported": return `This computer can't make that output. ${m}.`
    case "invalid_base": return `The base model is not valid: ${m}. Give exactly one of a repo or a path.`
    case "hardware_unavailable": return `The training device on this computer is not available right now. ${m}.`
    default: return `Refused (${code}): ${message}`
  }
}

/** Does this engine run on this machine? Reason is plain words when it does not. */
export function hardwareFit(e: Pick<EngineInfo, "hardware">, platform = process.platform, arch = process.arch): { fits: boolean; reason?: string } {
  const here = HW_FOR_HOST[`${platform}:${arch}`]
  if (here && e.hardware.includes(here)) return { fits: true }
  return { fits: false, reason: `needs ${e.hardware.map(hardwareLabel).join(" or ")}; this computer is ${platform}/${arch}` }
}

/** Build a job spec v1 from flags (used when --spec is not given). */
export function specFromFlags(f: { engine?: string; base?: string; dataset?: string; method?: string; place?: string; backend?: string; mlx?: boolean }): Record<string, unknown> {
  const dataset: Record<string, string> = { format: "chat" }
  if (f.dataset && /^(~|\.|\/)/.test(f.dataset)) dataset.path = f.dataset
  else if (f.dataset) dataset.repo = f.dataset
  return {
    spec_version: 1,
    engine: f.engine,
    base: { repo: f.base },
    dataset,
    method: f.method ?? "lora",
    resources: { place: f.place ?? "local", ...(f.backend ? { backend: f.backend } : {}) },
    ...(f.mlx ? { outputs: { mlx: true } } : {}),
  }
}
