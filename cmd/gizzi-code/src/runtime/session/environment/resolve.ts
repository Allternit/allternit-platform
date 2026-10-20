/**
 * Fetch a code environment, secret values included, from allternit-cloud-api.
 *
 * `GET {cloud}/api/v1/code-environments/:id/resolve` is the one route that
 * returns secret values, and it only answers the runtime-device token of a
 * device owned by the environment's user. That token is the one `gizzi pair`
 * / `gizzi login` stores (Pairing); hosted runtimes may instead be started
 * with ALLTERNIT_API_KEY / ALLTERNIT_API_TOKEN. Values are never logged.
 *
 * Base URL: ALLTERNIT_CLOUD_API_URL, else GIZZI_PLATFORM_API_URL, else the
 * production control plane — the same resolution the artifacts client uses.
 */
import z from "zod/v4"
import { CLOUD_URLS } from "@/shared/constants/cloudUrls"
import { Pairing } from "@/runtime/services/pairing/pairing"

export namespace EnvironmentResolve {
  export const Repo = z.object({ url: z.string().min(1), branch: z.string().optional() })
  export type Repo = z.infer<typeof Repo>

  export const Resolved = z.object({
    id: z.string(),
    name: z.string().default(""),
    network_access: z.enum(["full", "limited", "none"]).default("full"),
    allowed_hosts: z.array(z.string()).default([]),
    secrets: z
      .array(z.object({ name: z.string(), hosts: z.array(z.string()).default([]) }).passthrough())
      .default([]),
    setup_script: z.string().default(""),
    env_vars: z.string().default(""),
    repos: z.array(Repo).default([]),
    archived: z.boolean().optional(),
    secret_values: z.record(z.string(), z.string()).default({}),
  })
  export type Resolved = z.infer<typeof Resolved>

  /** A failure the session shows to the user as-is. */
  export class ResolveError extends Error {
    constructor(message: string) {
      super(message)
      this.name = "EnvironmentResolveError"
    }
  }

  export function cloudBase(): string {
    const explicit =
      process.env.ALLTERNIT_CLOUD_API_URL?.trim() || process.env.GIZZI_PLATFORM_API_URL?.trim() || CLOUD_URLS.api
    return explicit.replace(/\/+$/, "")
  }

  /** Runtime credentials, device token first (the resolve route's own auth). */
  async function tokens(): Promise<string[]> {
    const stored = await Pairing.load().catch(() => undefined)
    const device = Pairing.tokenUsable(stored) ? stored.deviceToken : undefined
    const env = [process.env.ALLTERNIT_API_KEY?.trim(), process.env.ALLTERNIT_API_TOKEN?.trim()]
    return [...new Set([device, ...env].filter((t): t is string => !!t))]
  }

  export async function hasCredential(): Promise<boolean> {
    return (await tokens()).length > 0
  }

  const TIMEOUT_MS = 20_000

  export async function resolve(id: string, abort?: AbortSignal): Promise<Resolved> {
    const list = await tokens()
    if (list.length === 0) {
      throw new ResolveError(
        "This runtime isn't signed in to Allternit, so it can't load the session's cloud environment. Pair it with `gizzi login` and try again.",
      )
    }
    const url = `${cloudBase()}/api/v1/code-environments/${encodeURIComponent(id)}/resolve`
    let last: { status: number; message: string } | undefined
    for (const token of list) {
      const signal = abort ? AbortSignal.any([abort, AbortSignal.timeout(TIMEOUT_MS)]) : AbortSignal.timeout(TIMEOUT_MS)
      let response: Response
      try {
        response = await fetch(url, {
          headers: { Authorization: `Bearer ${token}`, Accept: "application/json" },
          signal,
        })
      } catch (error) {
        if (abort?.aborted) throw error
        throw new ResolveError(
          `Couldn't reach Allternit to load the cloud environment (${error instanceof Error ? error.message : String(error)}).`,
        )
      }
      if (response.ok) {
        const raw = (await response.json().catch(() => undefined)) as any
        const body = raw && typeof raw === "object" && raw.environment && !raw.id ? { ...raw.environment, secret_values: raw.secret_values ?? raw.environment.secret_values } : raw
        const parsed = Resolved.safeParse(body)
        if (!parsed.success) throw new ResolveError("Allternit returned an environment this runtime couldn't read.")
        return parsed.data
      }
      const text = await response.text().catch(() => "")
      let message = text
      try {
        const j = JSON.parse(text) as { message?: string; error?: string }
        message = j.message || j.error || text
      } catch {}
      last = { status: response.status, message }
      // A rejected env token shouldn't hide a good device token (or vice versa).
      if (response.status === 401 || response.status === 403) continue
      break
    }
    if (!last) throw new ResolveError("Couldn't load the cloud environment.")
    if (last.status === 404) throw new ResolveError("The session's cloud environment no longer exists (it was deleted or isn't yours).")
    if (last.status === 401 || last.status === 403)
      throw new ResolveError(
        "Allternit refused this runtime's credentials for the cloud environment. Only a runtime paired to the environment's owner can load it.",
      )
    throw new ResolveError(`Couldn't load the cloud environment (HTTP ${last.status}${last.message ? `: ${last.message.slice(0, 200)}` : ""}).`)
  }
}
