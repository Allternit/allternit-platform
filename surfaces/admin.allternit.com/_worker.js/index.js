// admin.allternit.com: Pages advanced-mode worker. Every request must carry a valid Cloudflare
// Access token for this application; otherwise nothing is served. Fails closed while
// ACCESS_TEAM_DOMAIN / ACCESS_AUD are unset, and guards the *.pages.dev address as well.
import { verifyAccessJwt } from './access.js';

const deny = (msg) => new Response(msg, { status: 403, headers: { 'content-type': 'text/plain', 'cache-control': 'no-store', 'x-robots-tag': 'noindex' } });

export default {
  async fetch(request, env) {
    if (!env.ACCESS_TEAM_DOMAIN || !env.ACCESS_AUD) return deny('This admin site is locked until Cloudflare Access is configured.');
    const token = request.headers.get('cf-access-jwt-assertion');
    const who = await verifyAccessJwt(token, { team: env.ACCESS_TEAM_DOMAIN, aud: env.ACCESS_AUD }).catch(() => null);
    if (!who) return deny('Sign in through Cloudflare Access to view this page.');
    return env.ASSETS.fetch(request);
  },
};
