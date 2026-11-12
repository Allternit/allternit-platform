/**
 * Allternit gateway base URL — the single place that decides where gizzi-code
 * points for the Factory mail bridge, task API, and internal MCP routes.
 *
 * Precedence: ALLTERNIT_API_URL ?? Allternit_API_URL ?? GIZZI_GATEWAY_URL,
 * falling back to the local loopback gateway for dev. The fallback flips to
 * the public Backend B URL once it is deployed — change it here and only
 * here.
 *
 * NOTE: port 8013 is the gateway (Factory mail bridge + task API). Port 3001
 * is a DIFFERENT service (allternit-api / desktop-cloud) — never use it here.
 */

const envBase =
  process.env.ALLTERNIT_API_URL ??
  process.env.Allternit_API_URL ??
  process.env.GIZZI_GATEWAY_URL

/** Gateway base with no trailing slash. Loopback fallback flips to the public Backend B URL once deployed. */
export const ALLTERNIT_GATEWAY_BASE = (envBase ?? "http://127.0.0.1:8013").replace(/\/+$/, "")

/**
 * The gateway base read now (env at call time, same precedence and fallback).
 * Callers that run late (HTTP requests) use this so a test or a re-pointed
 * process sees the current env instead of the value frozen at import time.
 */
export function gatewayBase(): string {
  const base = process.env.ALLTERNIT_API_URL ?? process.env.Allternit_API_URL ?? process.env.GIZZI_GATEWAY_URL
  return (base ?? "http://127.0.0.1:8013").replace(/\/+$/, "")
}

/** Join an absolute API path onto the gateway base. */
export function gatewayUrl(path: string): string {
  return `${ALLTERNIT_GATEWAY_BASE}${path.startsWith("/") ? path : `/${path}`}`
}
