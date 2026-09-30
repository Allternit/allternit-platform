import type { AddressFamily, LookupAddress as AxiosLookupAddress } from 'axios'
import { lookup as dnsLookup } from 'dns'
import { promises as dnsPromises } from 'dns'
import { isIP } from 'net'

/**
 * SSRF guard for HTTP hooks.
 *
 * Blocks private, link-local, and other non-routable address ranges to prevent
 * project-configured HTTP hooks from reaching cloud metadata endpoints
 * (169.254.169.254) or internal infrastructure.
 *
 * Loopback (127.0.0.0/8, ::1) is intentionally ALLOWED — local dev policy
 * servers are a primary HTTP hook use case.
 *
 * When a global proxy or the sandbox network proxy is in use, the guard is
 * effectively bypassed for the target host because the proxy performs DNS
 * resolution. The sandbox proxy enforces its own domain allowlist.
 */

/**
 * Returns true if the address is in a range that HTTP hooks should not reach.
 *
 * Blocked IPv4:
 *   0.0.0.0/8        "this" network
 *   10.0.0.0/8       private
 *   100.64.0.0/10    shared address space / CGNAT (some cloud metadata, e.g. Alibaba 100.100.100.200)
 *   169.254.0.0/16   link-local (cloud metadata)
 *   172.16.0.0/12    private
 *   192.168.0.0/16   private
 *
 * Blocked IPv6:
 *   ::               unspecified
 *   fc00::/7         unique local
 *   fe80::/10        link-local
 *   ::ffff:<v4>      mapped IPv4 in a blocked range
 *
 * Allowed (returns false):
 *   127.0.0.0/8      loopback (local dev hooks)
 *   ::1              loopback
 *   everything else
 */
export function isBlockedAddress(address: string): boolean {
  const v = isIP(address)
  if (v === 4) {
    return isBlockedV4(address)
  }
  if (v === 6) {
    return isBlockedV6(address)
  }
  // Not a valid IP literal — let the real DNS path handle it (this function
  // is only called on results from dns.lookup, which always returns valid IPs)
  return false
}

function isBlockedV4(address: string): boolean {
  const parts = address.split('.').map(Number)
  const [a, b] = parts
  if (
    parts.length !== 4 ||
    a === undefined ||
    b === undefined ||
    parts.some(n => Number.isNaN(n))
  ) {
    return false
  }

  // Loopback explicitly allowed
  if (a === 127) return false

  // 0.0.0.0/8
  if (a === 0) return true
  // 10.0.0.0/8
  if (a === 10) return true
  // 169.254.0.0/16 — link-local, cloud metadata
  if (a === 169 && b === 254) return true
  // 172.16.0.0/12
  if (a === 172 && b >= 16 && b <= 31) return true
  // 100.64.0.0/10 — shared address space (RFC 6598, CGNAT). Some cloud
  // providers use this range for metadata endpoints (e.g. Alibaba Cloud at
  // 100.100.100.200).
  if (a === 100 && b >= 64 && b <= 127) return true
  // 192.168.0.0/16
  if (a === 192 && b === 168) return true

  return false
}

function isBlockedV6(address: string): boolean {
  const lower = address.toLowerCase()

  // ::1 loopback explicitly allowed
  if (lower === '::1') return false

  // :: unspecified
  if (lower === '::') return true

  // IPv4-mapped IPv6 (0:0:0:0:0:ffff:X:Y in any representation — ::ffff:a.b.c.d,
  // ::ffff:XXXX:YYYY, expanded, or partially expanded). Extract the embedded
  // IPv4 address and delegate to the v4 check. Without this, hex-form mapped
  // addresses (e.g. ::ffff:a9fe:a9fe = 169.254.169.254) bypass the guard.
  const mappedV4 = extractMappedIPv4(lower)
  if (mappedV4 !== null) {
    return isBlockedV4(mappedV4)
  }

  // fc00::/7 — unique local addresses (fc00:: through fdff::)
  if (lower.startsWith('fc') || lower.startsWith('fd')) {
    return true
  }

  // fe80::/10 — link-local. The /10 means fe80 through febf, but the first
  // hextet is always fe80 in practice (RFC 4291 requires the next 54 bits
  // to be zero). Check both to be safe.
  const firstHextet = lower.split(':')[0]
  if (
    firstHextet &&
    firstHextet.length === 4 &&
    firstHextet >= 'fe80' &&
    firstHextet <= 'febf'
  ) {
    return true
  }

  return false
}

/**
 * Expand `::` and optional trailing dotted-decimal so an IPv6 address is
 * represented as exactly 8 hex groups. Returns null if expansion is not
 * well-formed (the caller has already validated with isIP, so this is
 * defensive).
 */
function expandIPv6Groups(addr: string): number[] | null {
  // Handle trailing dotted-decimal IPv4 (e.g. ::ffff:169.254.169.254).
  // Replace it with its two hex groups so the rest of the expansion is uniform.
  let tailHextets: number[] = []
  if (addr.includes('.')) {
    const lastColon = addr.lastIndexOf(':')
    const v4 = addr.slice(lastColon + 1)
    addr = addr.slice(0, lastColon)
    const octets = v4.split('.').map(Number)
    if (
      octets.length !== 4 ||
      octets.some(n => !Number.isInteger(n) || n < 0 || n > 255)
    ) {
      return null
    }
    tailHextets = [
      (octets[0]! << 8) | octets[1]!,
      (octets[2]! << 8) | octets[3]!,
    ]
  }

  // Expand `::` (at most one) into the right number of zero groups.
  const dbl = addr.indexOf('::')
  let head: string[]
  let tail: string[]
  if (dbl === -1) {
    head = addr.split(':')
    tail = []
  } else {
    const headStr = addr.slice(0, dbl)
    const tailStr = addr.slice(dbl + 2)
    head = headStr === '' ? [] : headStr.split(':')
    tail = tailStr === '' ? [] : tailStr.split(':')
  }

  const target = 8 - tailHextets.length
  const fill = target - head.length - tail.length
  if (fill < 0) return null

  const hex = [...head, ...new Array<string>(fill).fill('0'), ...tail]
  const nums = hex.map(h => parseInt(h, 16))
  if (nums.some(n => Number.isNaN(n) || n < 0 || n > 0xffff)) {
    return null
  }
  nums.push(...tailHextets)
  return nums.length === 8 ? nums : null
}

/**
 * Extract the embedded IPv4 address from an IPv4-mapped IPv6 address
 * (0:0:0:0:0:ffff:X:Y) in any valid representation — compressed, expanded,
 * hex groups, or trailing dotted-decimal. Returns null if the address is
 * not an IPv4-mapped IPv6 address.
 */
function extractMappedIPv4(addr: string): string | null {
  const g = expandIPv6Groups(addr)
  if (!g) return null
  // IPv4-mapped: first 80 bits zero, next 16 bits ffff, last 32 bits = IPv4
  if (
    g[0] === 0 &&
    g[1] === 0 &&
    g[2] === 0 &&
    g[3] === 0 &&
    g[4] === 0 &&
    g[5] === 0xffff
  ) {
    const hi = g[6]!
    const lo = g[7]!
    return `${hi >> 8}.${hi & 0xff}.${lo >> 8}.${lo & 0xff}`
  }
  return null
}

/**
 * A dns.lookup-compatible function that resolves a hostname and rejects
 * addresses in blocked ranges. Used as the `lookup` option in axios request
 * config so that the validated IP is the one the socket connects to — no
 * rebinding window between validation and connection.
 *
 * IP literals in the hostname are validated directly without DNS.
 *
 * Signature matches axios's `lookup` config option (not Node's dns.lookup).
 */
export function ssrfGuardedLookup(
  hostname: string,
  options: object,
  callback: (
    err: Error | null,
    address: AxiosLookupAddress | AxiosLookupAddress[],
    family?: AddressFamily,
  ) => void,
): void {
  const wantsAll = 'all' in options && options.all === true

  // If hostname is already an IP literal, validate it directly. dns.lookup
  // would short-circuit too, but checking here gives a clearer error and
  // avoids any platform-specific lookup behavior for literals.
  const ipVersion = isIP(hostname)
  if (ipVersion !== 0) {
    if (isBlockedAddress(hostname)) {
      callback(ssrfError(hostname, hostname), '')
      return
    }
    const family = ipVersion === 6 ? 6 : 4
    if (wantsAll) {
      callback(null, [{ address: hostname, family }])
    } else {
      callback(null, hostname, family)
    }
    return
  }

  dnsLookup(hostname, { all: true }, (err, addresses) => {
    if (err) {
      callback(err, '')
      return
    }

    for (const { address } of addresses) {
      if (isBlockedAddress(address)) {
        callback(ssrfError(hostname, address), '')
        return
      }
    }

    const first = addresses[0]
    if (!first) {
      callback(
        Object.assign(new Error(`ENOTFOUND ${hostname}`), {
          code: 'ENOTFOUND',
          hostname,
        }),
        '',
      )
      return
    }

    const family = first.family === 6 ? 6 : 4
    if (wantsAll) {
      callback(
        null,
        addresses.map(a => ({
          address: a.address,
          family: a.family === 6 ? 6 : 4,
        })),
      )
    } else {
      callback(null, first.address, family)
    }
  })
}

function ssrfError(hostname: string, address: string): NodeJS.ErrnoException {
  const err = new Error(
    `HTTP hook blocked: ${hostname} resolves to ${address} (private/link-local address). Loopback (127.0.0.1, ::1) is allowed for local dev.`,
  )
  return Object.assign(err, {
    code: 'ERR_HTTP_HOOK_BLOCKED_ADDRESS',
    hostname,
    address,
  })
}

// ─── Shared fetch-level egress policy (MCP remote, web fetch, web proxy) ─────

export type EgressOptions = {
  /**
   * Allow loopback (127.0.0.0/8, ::1, localhost). Only for destinations the
   * user configured themselves (local MCP servers, local hook servers) —
   * never for URLs an agent or a web page chose. Cloud metadata, link-local,
   * CGNAT and private ranges are refused whatever this is set to.
   */
  allowLoopback?: boolean
}

const MAX_EGRESS_REDIRECTS = 5
const REDIRECT_STATUSES = new Set([301, 302, 303, 307, 308])

export function isLoopbackAddress(address: string): boolean {
  const v = isIP(address)
  if (v === 4) return address.startsWith('127.')
  if (v === 6) {
    const lower = address.toLowerCase()
    if (lower === '::1') return true
    const mapped = extractMappedIPv4(lower)
    return mapped !== null && mapped.startsWith('127.')
  }
  return false
}

function isAllowedAddress(address: string, opts: EgressOptions): boolean {
  if (isLoopbackAddress(address)) return opts.allowLoopback === true
  // :: and 0.0.0.0 are caught by isBlockedAddress
  return !isBlockedAddress(address)
}

/**
 * True when `hostname` may be dialed: every literal/resolved address must be
 * allowed. Fails closed on resolution errors.
 */
export async function isEgressHostAllowed(
  hostname: string,
  opts: EgressOptions = {},
): Promise<boolean> {
  const host = hostname.replace(/^\[|\]$/g, '')
  if (isIP(host) !== 0) return isAllowedAddress(host, opts)
  const lower = host.replace(/\.$/, '').toLowerCase()
  if (
    (lower === 'localhost' || lower.endsWith('.localhost')) &&
    opts.allowLoopback !== true
  ) {
    return false
  }
  try {
    const records = await dnsPromises.lookup(host, { all: true })
    if (records.length === 0) return false
    return records.every(r => isAllowedAddress(r.address, opts))
  } catch {
    return false
  }
}

export class EgressBlockedError extends Error {
  code = 'ERR_EGRESS_BLOCKED'
  constructor(public hostname: string) {
    super(`egress blocked: ${hostname} is a private, link-local or metadata address`)
  }
}

/** Throws EgressBlockedError unless `url` is http(s) to an allowed host. */
export async function assertEgressAllowed(
  url: string | URL,
  opts: EgressOptions = {},
): Promise<URL> {
  const u = typeof url === 'string' ? new URL(url) : url
  if (u.protocol !== 'http:' && u.protocol !== 'https:') {
    throw new EgressBlockedError(u.protocol)
  }
  if (!(await isEgressHostAllowed(u.hostname, opts))) {
    throw new EgressBlockedError(u.hostname)
  }
  return u
}

/**
 * fetch() that vets the destination before every hop. Redirects are followed
 * manually so a public host cannot bounce the request to a metadata or
 * private address. Drop-in for the MCP SDK transports' `fetch` option.
 */
export function createGuardedFetch(opts: EgressOptions = {}) {
  return async (input: string | URL | Request, init?: RequestInit): Promise<Response> => {
    let current: string | URL =
      input instanceof Request ? input.url : input
    let reqInit: RequestInit | undefined = init
    for (let hop = 0; hop <= MAX_EGRESS_REDIRECTS; hop++) {
      const u = await assertEgressAllowed(current, opts)
      const res = await fetch(input instanceof Request && hop === 0 ? input : u, {
        ...reqInit,
        redirect: 'manual',
      })
      if (!REDIRECT_STATUSES.has(res.status)) return res
      const location = res.headers.get('location')
      if (!location) return res
      current = new URL(location, u)
      if (res.status === 303) reqInit = { ...reqInit, method: 'GET', body: undefined }
    }
    throw new Error('egress: too many redirects')
  }
}
