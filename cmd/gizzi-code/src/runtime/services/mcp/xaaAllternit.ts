/**
 * XAA IdP mode "allternit": Allternit is the identity provider.
 *
 * When gizzi is signed in to Allternit (ALLTERNIT_API_URL + a token, the same
 * runtime credentials the pin check uses), an org admin may have turned on
 * managed sign-in for an MCP server. Then gizzi skips the browser:
 *
 *   1. POST {ALLTERNIT_API_URL}/api/v1/managed-auth/id-jag { server_url }
 *      The local allternit-api asks cloud-api (with this computer's device
 *      credential) for an ID-JAG signed by the org's Allternit issuer, and
 *      returns it with the vendor client credentials. 404 = not configured.
 *   2. RFC 7523 JWT-bearer grant at the vendor AS (`exchangeJwtAuthGrant`).
 *
 * On by default when signed in; GIZZI_DISABLE_ALLTERNIT_XAA=1 turns it off.
 * GIZZI_ENABLE_XAA still gates only the generic external-IdP mode.
 * No silent fallback: a configured server whose managed sign-in fails throws;
 * only "not configured" lets the normal OAuth flow run.
 */

import { isEnvTruthy } from '../../../shared/utils/envUtils.js'
import { logMCPDebug } from '../../../shared/utils/log.js'
import { runtimeCredentials } from './pins.js'
import { exchangeJwtAuthGrant, type XaaResult } from './xaa.js'

const REQUEST_TIMEOUT_MS = 20_000
/** How long a "not configured" answer is remembered per server URL. */
const NOT_CONFIGURED_TTL_MS = 5 * 60_000
/** How long "can't tell right now" (runtime or cloud unreachable) is remembered. */
const UNKNOWN_TTL_MS = 60_000

export type AllternitIdJag = {
  id_jag: string
  token_endpoint: string
  client_id: string
  client_secret?: string | null
  expires_in?: number | null
  connector_id: string
  org_id: string
  as_issuer: string
  resource: string
  scopes?: string[]
}

type Creds = { base: string; token: string } | null

export function isAllternitXaaEnabled(creds: Creds = runtimeCredentials()): boolean {
  return creds !== null && !isEnvTruthy(process.env.GIZZI_DISABLE_ALLTERNIT_XAA)
}

const notConfigured = new Map<string, number>()

function serverKey(url: string): string {
  try {
    return new URL(url).href.replace(/\/$/, '')
  } catch {
    return url.replace(/\/$/, '')
  }
}

function isLoopback(url: URL): boolean {
  const host = url.hostname.replace(/^\[|\]$/g, '')
  return host === 'localhost' || host === '::1' || host.startsWith('127.')
}

/**
 * The ID-JAG for `serverUrl`, or `null` when Allternit has no managed config for
 * it (the caller uses its normal sign-in). Throws a plain error otherwise.
 */
export async function fetchAllternitIdJag(
  serverUrl: string,
  opts: { creds?: Creds; fetchFn?: typeof fetch; now?: () => number } = {},
): Promise<AllternitIdJag | null> {
  const creds = opts.creds === undefined ? runtimeCredentials() : opts.creds
  if (!creds) return null
  const now = (opts.now ?? Date.now)()
  const key = serverKey(serverUrl)
  const until = notConfigured.get(key)
  if (until !== undefined && until > now) return null

  let res: Response
  try {
    res = await (opts.fetchFn ?? fetch)(`${creds.base}/api/v1/managed-auth/id-jag`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', authorization: `Bearer ${creds.token}` },
      body: JSON.stringify({ server_url: serverUrl }),
      signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    })
  } catch {
    // The runtime isn't answering: managed or not can't be known, so nothing
    // managed is attempted and the normal sign-in stays available.
    notConfigured.set(key, now + UNKNOWN_TTL_MS)
    return null
  }
  let body: any = null
  try {
    body = await res.json()
  } catch {}
  // 404: not managed. 503: can't tell right now (not paired, cloud down).
  // Older runtimes without the endpoint also answer 404.
  if (res.status === 404 || res.status === 503) {
    notConfigured.set(key, now + (res.status === 404 ? NOT_CONFIGURED_TTL_MS : UNKNOWN_TTL_MS))
    return null
  }
  if (!res.ok) {
    const message = body?.error?.message || `Allternit managed sign-in failed (HTTP ${res.status}).`
    throw new Error(`Allternit managed sign-in: ${message}`)
  }
  if (!body?.id_jag || !body?.token_endpoint || !body?.client_id) {
    throw new Error('Allternit managed sign-in: the Allternit app sent an incomplete answer.')
  }
  return body as AllternitIdJag
}

/**
 * Managed sign-in for one MCP server: ID-JAG from Allternit, then the JWT-bearer
 * grant. `null` when not configured. Refuses a non-HTTPS token endpoint (the
 * assertion and client secret would travel in the clear), loopback excepted.
 */
export async function performAllternitXaa(
  serverUrl: string,
  serverName: string,
  opts: { creds?: Creds; fetchFn?: typeof fetch } = {},
): Promise<XaaResult | null> {
  const jag = await fetchAllternitIdJag(serverUrl, opts)
  if (!jag) return null
  const endpoint = new URL(jag.token_endpoint)
  if (endpoint.protocol !== 'https:' && !isLoopback(endpoint)) {
    throw new Error(`Allternit managed sign-in: refusing non-HTTPS token endpoint ${jag.token_endpoint}`)
  }
  logMCPDebug(serverName, `XAA(allternit): ID-JAG obtained for ${jag.connector_id} (org ${jag.org_id})`)
  const tokens = await exchangeJwtAuthGrant({
    tokenEndpoint: jag.token_endpoint,
    assertion: jag.id_jag,
    clientId: jag.client_id,
    clientSecret: jag.client_secret || undefined,
    scope: jag.scopes?.length ? jag.scopes.join(' ') : undefined,
    fetchFn: opts.fetchFn,
  })
  logMCPDebug(serverName, 'XAA(allternit): access_token obtained')
  return { ...tokens, authorizationServerUrl: jag.as_issuer || endpoint.origin }
}

/** Test seam: forget remembered "not configured" answers. */
export function resetAllternitXaaCache(): void {
  notConfigured.clear()
}
