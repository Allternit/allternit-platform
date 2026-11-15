/**
 * Person-only models in the model list (TUI picker, `gizzi models`,
 * Desktop through the engine):
 *   - `chatgpt/<slug>` once ChatGPT is signed in on this computer (models
 *     from `GET https://api.openai.com/v1/models`, cached on the session 6 h)
 *   - `plan/<vendor>/<model>` and `copilot-plan/<model>` from cloud-api's
 *     connected rows (`GET /api/v1/connections?caps=runner`, `models` of an
 *     `on` row; §9.1, §9.5)
 * All of them carry `runtime: "person-plan"`: no tools, never picked
 * automatically, refused anywhere but a person's own send (rules.ts).
 */
import { cloudInferenceToken, cloudOrigin } from "@/runtime/providers/discovery/allternit-cloud"
import type { DiscoveredModel, DiscoveredProvider } from "@/runtime/providers/discovery"
import { isSelfHosted } from "@/shared/selfhost/offline"
import { Installation } from "@/shared/installation"
import { defaultDeps, loadSession, saveSession, usableSession } from "./chatgpt-auth"
import { CHATGPT_PROVIDER, COPILOT_PROVIDER, PLAN_PROVIDER, personToken } from "./rules"
import { fetchChatgptModels } from "./turn"

export const PERSON_PLAN_RUNTIME = "person-plan"
const MODELS_TTL_MS = 6 * 3_600_000
const VENDOR_NAME: Record<string, string> = { claude: "Claude", chatgpt: "ChatGPT", grok: "Grok", kimi: "Kimi" }

type Row = { id?: string; state?: string; models?: unknown; person_only?: boolean }

/** Connected runner/Copilot rows → local `plan` and `copilot-plan` providers. */
export function planProvidersFromRows(rows: Row[]): DiscoveredProvider[] {
  const plan: DiscoveredModel[] = []
  const copilot: DiscoveredModel[] = []
  for (const row of rows) {
    if (row.state !== "on" || !Array.isArray(row.models)) continue
    for (const id of row.models) {
      if (typeof id !== "string") continue
      if (id.startsWith("plan/")) {
        const rest = id.slice("plan/".length)
        const [vendor, ...model] = rest.split("/")
        if (!vendor || model.length === 0) continue
        plan.push({ id: rest, name: `${VENDOR_NAME[vendor] ?? vendor} plan · ${model.join("/")}`, context: 200_000, output: 16_384 })
      } else if (id.startsWith("copilot/")) {
        const model = id.slice("copilot/".length)
        if (model) copilot.push({ id: model, name: `Copilot plan · ${model}`, context: 128_000, output: 16_384 })
      }
    }
  }
  const out: DiscoveredProvider[] = []
  const base = { auth_type: "bearer" as const, source: "subscription" as const, base_url: `${cloudOrigin()}/v1`, options: { runtime: PERSON_PLAN_RUNTIME } }
  if (plan.length) out.push({ id: PLAN_PROVIDER, name: "Your plans on Allternit", ...base, models: plan.map((m, rank) => ({ ...m, rank })) })
  if (copilot.length) out.push({ id: COPILOT_PROVIDER, name: "GitHub Copilot plan", ...base, models: copilot.map((m, rank) => ({ ...m, rank })) })
  return out
}

async function cloudPlanProviders(): Promise<DiscoveredProvider[]> {
  const token = personToken() ?? (await cloudInferenceToken())
  if (!token) return []
  try {
    const res = await fetch(`${cloudOrigin()}/api/v1/connections?caps=runner`, {
      headers: { authorization: `Bearer ${token}`, accept: "application/json", "x-allternit-client": `gizzi-code/${Installation.VERSION}` },
      signal: AbortSignal.timeout(3_000),
    })
    if (!res.ok) return []
    const body = (await res.json()) as { connections?: Row[] }
    return planProvidersFromRows(body.connections ?? [])
  } catch {
    return []
  }
}

async function chatgptProvider(): Promise<DiscoveredProvider[]> {
  const deps = defaultDeps()
  const stored = await loadSession(deps).catch(() => undefined)
  if (!stored) return []
  let models = stored.models ?? []
  if (!models.length || !stored.models_at || Date.now() - stored.models_at > MODELS_TTL_MS) {
    try {
      const session = await usableSession(deps)
      const fresh = await fetchChatgptModels(session.access_token)
      if (fresh.length) {
        models = fresh
        await saveSession({ ...session, models: fresh, models_at: Date.now() }, deps)
      }
    } catch {
      // Offline or expired: keep the last list; the turn itself reports the sign-in problem.
    }
  }
  if (!models.length) return []
  return [
    {
      id: CHATGPT_PROVIDER,
      name: stored.plan ? `${stored.plan} (signed in here)` : "ChatGPT (signed in here)",
      auth_type: "bearer",
      source: "subscription",
      base_url: "https://api.openai.com/v1",
      options: { runtime: PERSON_PLAN_RUNTIME },
      models: models.map((m, rank) => ({ id: m.slug, name: m.name, context: 200_000, output: 32_768, rank })),
    },
  ]
}

export async function discoverPersonPlans(): Promise<DiscoveredProvider[]> {
  if (isSelfHosted()) return []
  const [chatgpt, cloud] = await Promise.all([chatgptProvider().catch(() => []), cloudPlanProviders()])
  return [...chatgpt, ...cloud]
}
