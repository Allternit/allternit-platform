/**
 * `gizzi runtime register --self-hosted` and `gizzi runtime status` for a
 * self-hosted Agents Server (selfhost-v1.yaml): GET /v1/server/info,
 * /v1/environments and /v1/environments/{id}/workers.
 *
 * The API key is checked against the server, then stored in the credential
 * store (see runtime/selfhost/profiles.ts). It is never printed or logged:
 * output shows only its first characters.
 */
import { createProfileStore, keyPrefix, type SelfHostProfile } from "../../../runtime/selfhost/profiles"

type Out = (line: string) => void

interface Deps {
  fetchImpl?: typeof fetch
  store?: ReturnType<typeof createProfileStore>
  out?: Out
  err?: Out
}

interface ServerInfo {
  version: string
  provider: string
  features?: Record<string, boolean>
  auth_modes?: string[]
  public_url?: string | null
  max_tool_result_bytes?: number
}

interface Environment {
  id: string
  name: string
  network: string
  allowed_hosts?: string[]
  worker_count?: number
  queue_depth?: number
}

interface Worker {
  worker_id: string
  last_seen_at: string
  online: boolean
  current_work_id?: string | null
  hostname?: string | null
  version?: string | null
}

class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string | undefined,
    message: string,
  ) {
    super(message)
  }
}

function isLocalHost(hostname: string): boolean {
  const h = hostname.replace(/^\[|\]$/g, "")
  return (
    h === "localhost" ||
    h === "::1" ||
    /^127\./.test(h) ||
    /^10\./.test(h) ||
    /^192\.168\./.test(h) ||
    /^172\.(1[6-9]|2\d|3[01])\./.test(h) ||
    h.endsWith(".local") ||
    h.endsWith(".internal")
  )
}

/** Normalized server URL. http:// is accepted only for loopback and private hosts. */
export function normalizeServerUrl(raw: string): string {
  let url: URL
  try {
    url = new URL(raw.trim())
  } catch {
    throw new Error(`Not a URL: ${raw}`)
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") throw new Error(`The server URL must be https:// (got ${url.protocol})`)
  if (url.protocol === "http:" && !isLocalHost(url.hostname)) {
    throw new Error(
      `http:// is only allowed for a local or private address. Use https:// for ${url.hostname}, so the API key is not sent in clear text.`,
    )
  }
  return url.toString().replace(/\/+$/, "")
}

function client(base: string, apiKey: string | undefined, fetchImpl: typeof fetch) {
  return async function get<T>(path: string): Promise<T> {
    const headers: Record<string, string> = { Accept: "application/json" }
    if (apiKey) headers.Authorization = `Bearer ${apiKey}`
    let res: Response
    try {
      res = await fetchImpl(`${base}${path}`, { headers, signal: AbortSignal.timeout(15_000) })
    } catch (error) {
      throw new ApiError(0, "unreachable", `Could not reach ${base} (${(error as Error).message})`)
    }
    if (!res.ok) {
      const text = await res.text().catch(() => "")
      let code: string | undefined
      let message = text.slice(0, 200) || res.statusText
      try {
        const body = JSON.parse(text) as { error?: { code?: string; message?: string } }
        code = body.error?.code
        message = body.error?.message ?? message
      } catch {
        // not JSON
      }
      throw new ApiError(res.status, code, message)
    }
    return (await res.json()) as T
  }
}

async function listEnvironments(get: ReturnType<typeof client>): Promise<Environment[]> {
  const out: Environment[] = []
  let after: string | null | undefined
  for (let page = 0; page < 20; page++) {
    const q = after ? `?limit=100&after=${encodeURIComponent(after)}` : "?limit=100"
    const body = await get<{ data: Environment[]; has_more?: boolean; next_cursor?: string | null }>(`/v1/environments${q}`)
    out.push(...(body.data ?? []))
    if (!body.has_more || !body.next_cursor) break
    after = body.next_cursor
  }
  return out
}

export async function selfHostedRegister(
  input: { server?: string; apiKey?: string; name?: string },
  deps: Deps = {},
): Promise<number> {
  const out = deps.out ?? ((l) => process.stdout.write(l + "\n"))
  const err = deps.err ?? ((l) => process.stderr.write(l + "\n"))
  const fetchImpl = deps.fetchImpl ?? fetch
  const store = deps.store ?? createProfileStore()

  const apiKey = (input.apiKey ?? process.env.ALLTERNIT_SERVER_API_KEY ?? "").trim()
  if (!input.server) {
    err("--server <url> is required with --self-hosted")
    return 1
  }
  if (!apiKey) {
    err("--api-key <ask_...> (or ALLTERNIT_SERVER_API_KEY) is required with --self-hosted")
    return 1
  }
  if (!apiKey.startsWith("ask_")) {
    err("--api-key must be a server API key (ask_...). Environment keys (aek_...) are for workers.")
    return 1
  }
  let base: string
  try {
    base = normalizeServerUrl(input.server)
  } catch (error) {
    err((error as Error).message)
    return 1
  }

  let info: ServerInfo
  try {
    info = await client(base, undefined, fetchImpl)<ServerInfo>("/v1/server/info")
  } catch (error) {
    err(`This does not look like an Agents Server: ${(error as Error).message}`)
    return 1
  }
  try {
    await client(base, apiKey, fetchImpl)("/v1/environments?limit=1")
  } catch (error) {
    if (error instanceof ApiError && (error.status === 401 || error.status === 403)) {
      err(`The server refused the API key (${keyPrefix(apiKey)}): HTTP ${error.status}${error.code ? ` ${error.code}` : ""}.`)
    } else {
      err(`Could not check the API key: ${(error as Error).message}`)
    }
    return 1
  }

  const name = (input.name ?? new URL(base).host).trim()
  const profile = await store.save({ name, url: base, apiKey })
  out(`Saved self-hosted server "${profile.name}"`)
  out(`  URL:      ${profile.url}`)
  out(`  Version:  ${info.version}`)
  out(`  Provider: ${info.provider}`)
  out(`  API key:  ${profile.keyPrefix} (in the credential store, not in config)`)
  out("Run `gizzi runtime status` to see environments, workers and queue depth.")
  return 0
}

export interface SelfHostedStatus {
  profile: Pick<SelfHostProfile, "name" | "url">
  server: ServerInfo
  environments: Array<Environment & { workers: Worker[]; workers_online: number }>
}

export async function collectSelfHostedStatus(
  input: { server?: string },
  deps: Deps = {},
): Promise<SelfHostedStatus | { error: string }> {
  const store = deps.store ?? createProfileStore()
  const fetchImpl = deps.fetchImpl ?? fetch
  const profile = await store.get(input.server)
  if (!profile) {
    return {
      error: input.server
        ? `No saved self-hosted server named ${input.server}. Run \`gizzi runtime register --self-hosted --server <url> --api-key <ask_...>\`.`
        : "No self-hosted server is saved. Run `gizzi runtime register --self-hosted --server <url> --api-key <ask_...>`.",
    }
  }
  const apiKey = await store.apiKey(profile)
  if (!apiKey) return { error: `The API key for "${profile.name}" is missing from the credential store. Register it again.` }
  const get = client(profile.url, apiKey, fetchImpl)
  try {
    const server = await client(profile.url, undefined, fetchImpl)<ServerInfo>("/v1/server/info")
    const envs = await listEnvironments(get)
    const environments = await Promise.all(
      envs.map(async (env) => {
        const workers = await get<{ data: Worker[] }>(`/v1/environments/${encodeURIComponent(env.id)}/workers`)
          .then((b) => b.data ?? [])
          .catch(() => [] as Worker[])
        return { ...env, workers, workers_online: workers.filter((w) => w.online).length }
      }),
    )
    return { profile: { name: profile.name, url: profile.url }, server, environments }
  } catch (error) {
    if (error instanceof ApiError && (error.status === 401 || error.status === 403)) {
      return { error: `The server refused the saved API key for "${profile.name}" (HTTP ${error.status}). Register a new key.` }
    }
    return { error: (error as Error).message }
  }
}

export async function selfHostedStatus(input: { server?: string; json?: boolean }, deps: Deps = {}): Promise<number> {
  const out = deps.out ?? ((l) => process.stdout.write(l + "\n"))
  const err = deps.err ?? ((l) => process.stderr.write(l + "\n"))
  const status = await collectSelfHostedStatus(input, deps)
  if ("error" in status) {
    err(status.error)
    return 1
  }
  if (input.json) {
    out(JSON.stringify(status, null, 2))
    return 0
  }
  const { server, environments } = status
  out("")
  out(`Self-hosted server: ${status.profile.name} (${status.profile.url})`)
  out(`  Version:   ${server.version}`)
  out(`  Provider:  ${server.provider}`)
  if (server.auth_modes?.length) out(`  Sign-in:   ${server.auth_modes.join(", ")}`)
  if (server.features) {
    const on = Object.entries(server.features)
      .filter(([, v]) => v)
      .map(([k]) => k)
    out(`  Features:  ${on.length ? on.join(", ") : "none"}`)
  }
  const totalOnline = environments.reduce((n, e) => n + e.workers_online, 0)
  const totalQueue = environments.reduce((n, e) => n + (e.queue_depth ?? 0), 0)
  out(`  Workers online: ${totalOnline}   Queue depth: ${totalQueue}`)
  out("")
  if (environments.length === 0) {
    out("No environments yet. Create one with POST /v1/environments.")
    out("")
    return 0
  }
  out(`Environments (${environments.length}):`)
  out(`  ${"NAME".padEnd(24)} ${"ID".padEnd(30)} ${"NETWORK".padEnd(8)} ${"WORKERS".padEnd(8)} QUEUE`)
  for (const env of environments) {
    out(
      `  ${env.name.slice(0, 24).padEnd(24)} ${env.id.slice(0, 30).padEnd(30)} ${String(env.network).padEnd(8)} ` +
        `${`${env.workers_online}/${env.workers.length}`.padEnd(8)} ${env.queue_depth ?? 0}`,
    )
    for (const w of env.workers) {
      const mark = w.online ? "●" : "○"
      const busy = w.current_work_id ? ` working on ${w.current_work_id}` : ""
      out(`      ${mark} ${w.worker_id}${w.version ? ` (${w.version})` : ""}  last seen ${w.last_seen_at}${busy}`)
    }
  }
  out("")
  return 0
}
