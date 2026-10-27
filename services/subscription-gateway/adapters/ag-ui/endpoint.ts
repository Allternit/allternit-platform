/**
 * Where a user-supplied AG-UI endpoint may point, and a fetch pinned to the address that was checked.
 * Loopback (127.0.0.1, ::1, localhost) over http or https, or https to a host whose every DNS answer is public.
 * The pinned fetch connects by node:http(s) with a fixed `lookup`, so the socket goes to the checked IP while TLS
 * still verifies the certificate for the hostname; a DNS answer that changes between check and connect is never used.
 */
import { lookup as dnsLookup } from 'node:dns/promises';
import { request as httpRequest, type IncomingMessage } from 'node:http';
import { request as httpsRequest } from 'node:https';
import { BlockList, isIP, type LookupFunction } from 'node:net';
import { Readable } from 'node:stream';
import { fail, ok, type AaiResult } from '@allternit/agent-gateway';
import { isObject } from '../_shared/paired-local.js';

export type Endpoint = { url: URL; loopback: boolean; token?: string; headers: Record<string, string> };
export type Resolve = (host: string) => Promise<{ address: string; family: number }[]>;
export const LOOPBACK = new Set(['127.0.0.1', '[::1]', 'localhost']);
const FORBIDDEN_HEADERS = new Set(['host', 'content-length', 'content-type', 'accept', 'connection', 'transfer-encoding', 'te', 'upgrade', 'keep-alive', 'proxy-authorization', 'expect', 'trailer']);

const blocked = new BlockList();
for (const [net, bits] of [['0.0.0.0', 8], ['10.0.0.0', 8], ['100.64.0.0', 10], ['127.0.0.0', 8], ['169.254.0.0', 16], ['172.16.0.0', 12], ['192.0.0.0', 24], ['192.0.2.0', 24], ['192.88.99.0', 24], ['192.168.0.0', 16], ['198.18.0.0', 15], ['198.51.100.0', 24], ['203.0.113.0', 24], ['224.0.0.0', 4], ['240.0.0.0', 4]] as const) blocked.addSubnet(net, bits, 'ipv4');
for (const [net, bits] of [['::', 128], ['::1', 128], ['64:ff9b::', 96], ['64:ff9b:1::', 48], ['100::', 64], ['2001::', 32], ['2001:db8::', 32], ['2002::', 16], ['fc00::', 7], ['fe80::', 10], ['fec0::', 10], ['ff00::', 8]] as const) blocked.addSubnet(net, bits, 'ipv6');

/** True for private, loopback, link-local, CGNAT, ULA, metadata, documentation, multicast and reserved addresses. */
export function isPrivateAddress(address: string): boolean {
  const family = isIP(address);
  if (family === 4) return blocked.check(address, 'ipv4');
  if (family !== 6) return true;
  const mapped = /^::ffff:(\d+\.\d+\.\d+\.\d+)$/i.exec(address) ?? /^::(\d+\.\d+\.\d+\.\d+)$/.exec(address);
  if (mapped) return blocked.check(mapped[1], 'ipv4');
  if (/^::ffff:/i.test(address)) return true; // hex-form IPv4-mapped: refuse rather than decode
  return blocked.check(address, 'ipv6');
}

/** Parses the sealed credential: JSON {"url","token"?,"headers"?}. */
export function parseCredential(raw: string | undefined): AaiResult<Endpoint> {
  if (!raw?.trim()) return fail('AUTH_REQUIRED', 'Connect an AG-UI agent by entering its run URL (and token, if it needs one).');
  let value: unknown; try { value = JSON.parse(raw); } catch { return fail('AUTH_REQUIRED', 'The AG-UI credential must be JSON with a url and an optional token.'); }
  if (!isObject(value) || typeof value.url !== 'string') return fail('AUTH_REQUIRED', 'The AG-UI credential must include the agent run URL.');
  let url: URL; try { url = new URL(value.url.trim()); } catch { return fail('AUTH_REQUIRED', 'The AG-UI agent URL is not a valid URL.'); }
  if (url.username || url.password || url.hash) return fail('POLICY_DENIED', 'The AG-UI agent URL must not contain a username, password or #fragment.');
  const loopback = LOOPBACK.has(url.hostname);
  if (!(url.protocol === 'https:' || (url.protocol === 'http:' && loopback))) return fail('POLICY_DENIED', 'The AG-UI agent URL must use https, or http only on this computer (127.0.0.1, ::1 or localhost).');
  const token = value.token === undefined || value.token === null || value.token === '' ? undefined : value.token;
  if (token !== undefined && (typeof token !== 'string' || /[\r\n]/.test(token) || token.length > 8192)) return fail('AUTH_REQUIRED', 'The AG-UI token must be a single-line string.');
  const headers: Record<string, string> = {};
  if (value.headers !== undefined) {
    if (!isObject(value.headers) || Object.keys(value.headers).length > 20) return fail('AUTH_REQUIRED', 'AG-UI headers must be an object of up to 20 string values.');
    for (const [k, v] of Object.entries(value.headers)) {
      if (!/^[A-Za-z0-9!#$%&'*+.^_`|~-]{1,64}$/.test(k) || FORBIDDEN_HEADERS.has(k.toLowerCase()) || (token && k.toLowerCase() === 'authorization') || typeof v !== 'string' || /[\r\n]/.test(v) || v.length > 8192) return fail('AUTH_REQUIRED', `The AG-UI header "${k.slice(0, 64)}" is not allowed.`);
      headers[k] = v;
    }
  }
  return ok({ url, loopback, ...(token ? { token: token.trim() } : {}), headers });
}

const systemResolve: Resolve = (host) => dnsLookup(host, { all: true, verbatim: true });

/**
 * The fetch to use for this endpoint: loopback → plain fetch; public https → resolve, refuse if any answer is not
 * public, then pin the connection to the first answer. `override` (tests) replaces the transport after the same checks.
 */
export async function endpointFetch(endpoint: Endpoint, resolve: Resolve = systemResolve, override?: typeof fetch): Promise<AaiResult<typeof fetch>> {
  if (endpoint.loopback) return ok(override ?? fetch);
  const host = endpoint.url.hostname.replace(/^\[|\]$/g, '');
  let answers: { address: string; family: number }[];
  if (isIP(host)) answers = [{ address: host, family: isIP(host) }];
  else { try { answers = await resolve(host); } catch { return fail('VENDOR_UNAVAILABLE', `The AG-UI host ${host} could not be resolved.`); } }
  if (!answers.length) return fail('VENDOR_UNAVAILABLE', `The AG-UI host ${host} has no address.`);
  if (answers.some((a) => isPrivateAddress(a.address))) return fail('POLICY_DENIED', 'The AG-UI agent URL points to a private, local-network or reserved address. Use a public https host, or 127.0.0.1 for an agent on this computer.');
  return ok(override ?? pinnedFetch(answers[0].address, answers[0].family === 6 ? 6 : 4));
}

/** A fetch whose sockets always connect to `address`; redirects are never followed (a 3xx is returned as-is). */
export function pinnedFetch(address: string, family: 4 | 6): typeof fetch {
  const lookup = ((_host: string, options: { all?: boolean }, cb: (...args: unknown[]) => void) => options?.all ? cb(null, [{ address, family }]) : cb(null, address, family)) as unknown as LookupFunction;
  return ((input: string | URL | Request, init?: RequestInit) => new Promise<Response>((resolve, reject) => {
    const url = new URL(input instanceof Request ? input.url : String(input));
    const headers: Record<string, string> = {}; new Headers(init?.headers).forEach((v, k) => { headers[k] = v; });
    const req = (url.protocol === 'https:' ? httpsRequest : httpRequest)(url, { method: init?.method ?? 'GET', headers, lookup, signal: init?.signal ?? undefined }, (res: IncomingMessage) => {
      const status = res.statusCode ?? 0;
      const out = new Headers(); for (const [k, v] of Object.entries(res.headers)) if (v !== undefined) out.set(k, Array.isArray(v) ? v.join(', ') : v);
      const empty = status === 204 || status === 205 || status === 304;
      if (empty) res.resume();
      try { resolve(new Response(empty ? null : Readable.toWeb(res) as ReadableStream<Uint8Array>, { status: status >= 200 && status <= 599 ? status : 502, headers: out })); } catch (e) { res.destroy(); reject(e); }
    });
    req.on('error', reject);
    req.end(typeof init?.body === 'string' ? init.body : undefined);
  })) as typeof fetch;
}
