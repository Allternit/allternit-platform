// Rebuilds admin.allternit.com when allternit-ai merges to main.
//
// Cloudflare Pages builds the admin site from allternit-platform, so its own Git integration only
// sees platform merges. This Worker receives allternit-ai's GitHub push webhook, checks the
// signature, ignores everything except pushes to main, and calls the Pages deploy hook.
//
// Secrets (wrangler secret put): GITHUB_WEBHOOK_SECRET, PAGES_DEPLOY_HOOK_URL.

const enc = new TextEncoder();

export async function verifySignature(secret, body, header) {
  if (!secret || !header || !header.startsWith('sha256=')) return false;
  const key = await crypto.subtle.importKey('raw', enc.encode(secret), { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
  const mac = new Uint8Array(await crypto.subtle.sign('HMAC', key, enc.encode(body)));
  const expected = 'sha256=' + [...mac].map((b) => b.toString(16).padStart(2, '0')).join('');
  if (expected.length !== header.length) return false;
  let diff = 0;
  for (let i = 0; i < expected.length; i++) diff |= expected.charCodeAt(i) ^ header.charCodeAt(i);
  return diff === 0;
}

export function shouldRebuild(event, payload) {
  if (event === 'ping') return { rebuild: false, status: 200, reason: 'pong' };
  if (event !== 'push') return { rebuild: false, status: 202, reason: `ignored event ${event}` };
  if (payload?.ref !== 'refs/heads/main') return { rebuild: false, status: 202, reason: `ignored ref ${payload?.ref}` };
  if (payload?.repository?.full_name !== 'Allternit/allternit-ai') return { rebuild: false, status: 202, reason: 'ignored repository' };
  return { rebuild: true, status: 200, reason: `rebuild for ${String(payload.after || '').slice(0, 7)}` };
}

export default {
  async fetch(request, env) {
    if (request.method !== 'POST') return new Response('Method not allowed', { status: 405 });
    const body = await request.text();
    if (!(await verifySignature(env.GITHUB_WEBHOOK_SECRET, body, request.headers.get('x-hub-signature-256')))) {
      return new Response('Bad signature', { status: 401 });
    }
    let payload;
    try { payload = JSON.parse(body); } catch { return new Response('Bad JSON', { status: 400 }); }
    const decision = shouldRebuild(request.headers.get('x-github-event'), payload);
    if (decision.rebuild) {
      const res = await fetch(env.PAGES_DEPLOY_HOOK_URL, { method: 'POST' });
      if (!res.ok) return new Response(`Deploy hook failed: ${res.status}`, { status: 502 });
    }
    return new Response(decision.reason, { status: decision.status });
  },
};
