// Destination rules for the box relay (CONTRACT-colibri-unkernel.md section 9).
//
// The Wisp CONNECT packet carries a hostname, so the relay can check the name
// before any TCP connection is made. The guest page has a copy of this list,
// but that copy is advice only: the page can open its own WebSocket. This file
// is the real control.

/** Exact hostnames the relay will connect to. Port 443 only. */
export const ALLOWED_HOSTS: ReadonlySet<string> = new Set([
  "dl-cdn.alpinelinux.org",
  "registry.npmjs.org",
  "pypi.org",
  "files.pythonhosted.org",
  "crates.io",
  "index.crates.io",
  "static.crates.io",
  "proxy.golang.org",
  "sum.golang.org",
  "github.com",
  "api.github.com",
  "codeload.github.com",
  "raw.githubusercontent.com",
  "objects.githubusercontent.com",
  "release-assets.githubusercontent.com",
]);

export const ALLOWED_PORTS: ReadonlySet<number> = new Set([443]);

export type DestinationRefusal = "port" | "ip_literal" | "not_listed";

/**
 * True for anything that looks like an IP address instead of a name: IPv6
 * (any colon or bracket) and IPv4 in every form a resolver might accept
 * (dotted, short `127.1`, hex `0x7f.1`, a single integer `2130706433`).
 */
export function isIpLiteral(host: string): boolean {
  if (host.includes(":") || host.includes("[") || host.includes("]")) return true;
  const labels = host.split(".");
  if (labels.length === 0 || labels.length > 4) return false;
  return labels.every((label) => /^(0x[0-9a-f]*|[0-9]+)$/i.test(label));
}

/**
 * Checks a Wisp CONNECT destination. Returns null when allowed, otherwise the
 * reason it was refused. The match is exact (case-insensitive, as DNS is): no
 * wildcards, no suffix match, no trailing dot.
 */
export function checkDestination(host: string, port: number): DestinationRefusal | null {
  if (isIpLiteral(host)) return "ip_literal";
  if (!ALLOWED_HOSTS.has(host.toLowerCase())) return "not_listed";
  if (!ALLOWED_PORTS.has(port)) return "port";
  return null;
}

/**
 * Rebinding guard, applied to every address a name resolves to. Refuses
 * private, loopback, link-local, carrier-grade NAT, documentation, multicast,
 * reserved, and the 198.18.0.0/15 range (the guest maps names to placeholder
 * addresses there). Only IPv4 is resolved and used; anything that does not
 * parse as IPv4 is treated as blocked.
 */
export function isBlockedAddress(ip: string): boolean {
  const m = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(ip);
  if (!m) return true;
  const o = m.slice(1).map(Number);
  if (o.some((n) => n > 255)) return true;
  const [a, b, c] = o as [number, number, number, number];
  if (a === 0) return true; // 0.0.0.0/8
  if (a === 10) return true; // 10.0.0.0/8
  if (a === 100 && b >= 64 && b <= 127) return true; // 100.64.0.0/10
  if (a === 127) return true; // loopback
  if (a === 169 && b === 254) return true; // link-local
  if (a === 172 && b >= 16 && b <= 31) return true; // 172.16.0.0/12
  if (a === 192 && b === 0 && (c === 0 || c === 2)) return true; // 192.0.0.0/24, 192.0.2.0/24
  if (a === 192 && b === 168) return true; // 192.168.0.0/16
  if (a === 198 && (b === 18 || b === 19)) return true; // 198.18.0.0/15
  if (a === 198 && b === 51 && c === 100) return true; // 198.51.100.0/24
  if (a === 203 && b === 0 && c === 113) return true; // 203.0.113.0/24
  if (a >= 224) return true; // multicast, reserved, broadcast
  return false;
}

/** A hostname safe to write to logs, or a placeholder. Never a URL or payload. */
export function loggableHost(host: string): string {
  return /^[a-z0-9.-]{1,253}$/i.test(host) ? host.toLowerCase() : "(invalid)";
}
