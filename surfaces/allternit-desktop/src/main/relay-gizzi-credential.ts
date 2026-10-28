import { URLS } from './config.js';

/**
 * Relayed requests routed straight to the local gizzi-code (providers,
 * agents, sessions) must not carry this runtime's device token: gizzi reads
 * any non-`alt_` Bearer as a Clerk JWT and answers 401 "Invalid or expired
 * Clerk token" (live 2026-09-30: ai.allternit.com's model picker never got
 * its provider list). Loopback gizzi needs no credential; the relayed user is
 * still named by the X-Allternit-* headers.
 */
export function dropRuntimeCredentialForGizzi(localUrl: string, headers: Headers): void {
  if (!localUrl.startsWith(`${URLS.GIZZI}/`)) return;
  headers.delete('Authorization');
  headers.delete('X-Allternit-Desktop-Access-Token');
}

/** The bare provider list is gizzi's; sub-routes are allternit-api's. */
export function isGizziProviderListPath(requestPath: string): boolean {
  return /^\/api\/v1\/providers\/?(\?|$)/.test(requestPath);
}

/**
 * gizzi-code routes allternit-api does not serve, which the native phone app
 * (Allternit Mobile) reaches over the relay: terminal (pty REST + attach
 * socket + dev-server ports), file browser, cron jobs, routines and loops,
 * and the bus event stream. Before this they fell through to allternit-api
 * and answered 501.
 */
export function isGizziOwnedRelayPath(requestPath: string): boolean {
  return /^\/v1\/(pty|file|cron|automations)(\/|\?|$)/.test(requestPath)
    || /^\/v1\/event(\?|$)/.test(requestPath);
}
