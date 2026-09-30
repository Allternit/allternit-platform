/**
 * Sign in with ChatGPT — gizzi's side of the Desktop token broker.
 *
 * Allternit Desktop owns the ChatGPT OAuth session (OS-keychain storage,
 * refresh, revocation). It hands gizzi a loopback broker URL + secret in the
 * environment; gizzi asks it for a short-lived access token per request and
 * never sees the refresh token. Absent env (web app, CLI, flag off) means
 * SIWC is simply unavailable and the subscription lane behaves as before.
 */

export const SIWC_PROVIDER_ID = "subs-chatgpt"
export const SIWC_FABRIC_PROVIDER = "chatgpt"
export const SIWC_RESPONSES_URL = "https://api.openai.com/v1/responses"

function brokerURL(): string | undefined {
  const url = (process.env.ALLTERNIT_SIWC_BROKER_URL || "").trim()
  return url ? url.replace(/\/+$/, "") : undefined
}

function brokerSecret(): string | undefined {
  return process.env.ALLTERNIT_SIWC_BROKER_TOKEN?.trim() || undefined
}

/** True only under Desktop, which sets the broker env; says nothing about sign-in. */
export function siwcConfigured(): boolean {
  return Boolean(brokerURL() && brokerSecret())
}

async function brokerGet<T>(path: string, signal?: AbortSignal): Promise<T | undefined> {
  const url = brokerURL()
  const secret = brokerSecret()
  if (!url || !secret) return undefined
  try {
    const res = await fetch(`${url}${path}`, {
      headers: { Authorization: `Bearer ${secret}` },
      signal: signal ?? AbortSignal.timeout(8000),
    })
    if (!res.ok) return undefined // 409: flag off, signed out, or plan usage not granted
    return (await res.json()) as T
  } catch {
    return undefined
  }
}

/** A current access token for the signed-in ChatGPT account, or undefined. */
export async function siwcToken(signal?: AbortSignal): Promise<string | undefined> {
  const body = await brokerGet<{ access_token?: string }>("/v1/token", signal)
  return body?.access_token || undefined
}

export interface SiwcModelInfo {
  slug: string
  display_name: string
}

/** The account's live model catalog (`GET /v1/models`, visibility=list). */
export async function siwcModels(): Promise<SiwcModelInfo[]> {
  const body = await brokerGet<{ models?: SiwcModelInfo[] }>("/v1/models")
  return Array.isArray(body?.models) ? body!.models : []
}
