// Cloudflare Access check for admin.allternit.com. Fails closed.
//
// Every request must carry a Cf-Access-Jwt-Assertion header signed by our Access team and
// issued for this application. Without ACCESS_TEAM_DOMAIN and ACCESS_AUD configured, or
// without a valid token, nothing is served. This guards the *.pages.dev address too.

const certCache = new Map(); // team domain -> { keys, at }
const b64url = (s) => Uint8Array.from(atob(s.replace(/-/g, '+').replace(/_/g, '/').padEnd(Math.ceil(s.length / 4) * 4, '=')), (c) => c.charCodeAt(0));
const json = (bytes) => JSON.parse(new TextDecoder().decode(bytes));

async function certs(team, fetcher) {
  const hit = certCache.get(team);
  if (hit && Date.now() - hit.at < 3600_000) return hit.keys;
  const res = await fetcher(`https://${team}/cdn-cgi/access/certs`);
  if (!res.ok) throw new Error(`certs ${res.status}`);
  const keys = (await res.json()).keys || [];
  certCache.set(team, { keys, at: Date.now() });
  return keys;
}

export async function verifyAccessJwt(token, { team, aud, now = Date.now(), fetcher = fetch }) {
  if (!token || !team || !aud) return null;
  const parts = token.split('.');
  if (parts.length !== 3) return null;
  let header, payload;
  try { header = json(b64url(parts[0])); payload = json(b64url(parts[1])); } catch { return null; }
  if (header.alg !== 'RS256') return null;
  const jwk = (await certs(team, fetcher)).find((k) => k.kid === header.kid);
  if (!jwk) return null;
  const key = await crypto.subtle.importKey('jwk', jwk, { name: 'RSASSA-PKCS1-v1_5', hash: 'SHA-256' }, false, ['verify']);
  const ok = await crypto.subtle.verify('RSASSA-PKCS1-v1_5', key, b64url(parts[2]), new TextEncoder().encode(parts[0] + '.' + parts[1]));
  if (!ok) return null;
  const auds = Array.isArray(payload.aud) ? payload.aud : [payload.aud];
  if (!auds.includes(aud)) return null;
  if (payload.iss !== `https://${team}`) return null;
  if (typeof payload.exp !== 'number' || payload.exp * 1000 <= now) return null;
  return payload;
}

export function _clearCertCache() { certCache.clear(); }
