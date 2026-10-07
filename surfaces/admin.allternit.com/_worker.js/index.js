// admin.allternit.com: Pages advanced-mode worker. Every request must carry a valid Cloudflare
// Access token for this application; otherwise nothing is served. Fails closed while the Access
// team domain / application AUD are unset, and guards the *.pages.dev address as well.
// Values come from ../access.json (copied in as config.js at build time); env vars override.
import { verifyAccessJwt } from './access.js';
import config from './config.js';

const deny = (msg) => new Response(msg, { status: 403, headers: { 'content-type': 'text/plain', 'cache-control': 'no-store', 'x-robots-tag': 'noindex' } });

export default {
  async fetch(request, env) {
    const team = env.ACCESS_TEAM_DOMAIN || config.teamDomain, aud = env.ACCESS_AUD || config.aud;
    if (!team || !aud) return deny('This admin site is locked until Cloudflare Access is configured.');
    const token = request.headers.get('cf-access-jwt-assertion');
    const who = await verifyAccessJwt(token, { team, aud }).catch(() => null);
    if (!who) return deny('Sign in through Cloudflare Access to view this page.');
    return env.ASSETS.fetch(request);
  },
};
