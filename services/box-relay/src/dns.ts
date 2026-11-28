// IPv4 lookup over DNS-over-HTTPS, so the relay can check every address a name
// resolves to (rebinding guard) and then connect to that exact address. The
// guest does its own TLS with the hostname as SNI, so connecting by address is
// transparent to it, and it closes the gap between "checked" and "connected".

export type Resolver = (hostname: string) => Promise<string[]>;

const DOH_URL = "https://cloudflare-dns.com/dns-query";
const MIN_TTL_S = 30;
const MAX_TTL_S = 300;

interface DohAnswer {
  type?: number;
  data?: string;
  TTL?: number;
}

export function makeDohResolver(
  fetchFn: typeof fetch,
  now: () => number = Date.now,
): Resolver {
  const cache = new Map<string, { ips: string[]; until: number }>();
  return async (hostname) => {
    const key = hostname.toLowerCase();
    const hit = cache.get(key);
    if (hit && hit.until > now()) return hit.ips;

    const url = `${DOH_URL}?name=${encodeURIComponent(key)}&type=A`;
    const res = await fetchFn(url, { headers: { accept: "application/dns-json" } });
    if (!res.ok) return [];
    const body = (await res.json()) as { Status?: number; Answer?: DohAnswer[] };
    if (body.Status !== 0 || !Array.isArray(body.Answer)) return [];

    const answers = body.Answer.filter(
      (a) => a.type === 1 && typeof a.data === "string" && /^\d{1,3}(\.\d{1,3}){3}$/.test(a.data),
    );
    const ips = answers.map((a) => a.data as string);
    if (ips.length > 0) {
      const ttl = Math.min(
        MAX_TTL_S,
        Math.max(MIN_TTL_S, Math.min(...answers.map((a) => a.TTL ?? MIN_TTL_S))),
      );
      cache.set(key, { ips, until: now() + ttl * 1000 });
    }
    return ips;
  };
}
