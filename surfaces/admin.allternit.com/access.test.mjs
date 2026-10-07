import { test } from 'node:test';
import assert from 'node:assert/strict';
import { generateKeyPairSync, createSign } from 'node:crypto';
import { verifyAccessJwt, _clearCertCache } from './_worker.js/access.js';
import worker from './_worker.js/index.js';

const team = 'allternit.cloudflareaccess.com', aud = 'aud-123';
const { privateKey, publicKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
const jwk = { ...publicKey.export({ format: 'jwk' }), kid: 'k1', alg: 'RS256' };
const fetcher = async () => new Response(JSON.stringify({ keys: [jwk] }));
const b64 = (o) => Buffer.from(JSON.stringify(o)).toString('base64url');
function sign(payload, kid = 'k1', key = privateKey) {
  const head = b64({ alg: 'RS256', kid }) + '.' + b64(payload);
  return head + '.' + createSign('RSA-SHA256').update(head).sign(key).toString('base64url');
}
const good = () => ({ aud: [aud], iss: `https://${team}`, exp: Math.floor(Date.now() / 1000) + 600, email: 'owner@example.com' });

test('accepts a valid Access token', async () => {
  _clearCertCache();
  assert.equal((await verifyAccessJwt(sign(good()), { team, aud, fetcher })).email, 'owner@example.com');
});

test('rejects wrong audience, issuer, expiry, key and tampering', async () => {
  _clearCertCache();
  const other = generateKeyPairSync('rsa', { modulusLength: 2048 }).privateKey;
  const cases = [
    sign({ ...good(), aud: ['other'] }),
    sign({ ...good(), iss: 'https://evil.cloudflareaccess.com' }),
    sign({ ...good(), exp: Math.floor(Date.now() / 1000) - 5 }),
    sign(good(), 'k1', other),
    sign(good(), 'unknown-kid'),
    sign(good()).replace(/\.[^.]+\./, '.' + b64({ ...good(), email: 'x@y.z' }) + '.'),
    'not-a-jwt', '', null,
  ];
  for (const t of cases) assert.equal(await verifyAccessJwt(t, { team, aud, fetcher }), null);
});

test('worker fails closed', async () => {
  let served = 0;
  const env = (extra = {}) => ({ ASSETS: { fetch: async () => { served++; return new Response('ok'); } }, ...extra });
  const req = (h = {}) => new Request('https://admin.allternit.com/', { headers: h });
  assert.equal((await worker.fetch(req(), env())).status, 403);
  assert.equal((await worker.fetch(req(), env({ ACCESS_TEAM_DOMAIN: team, ACCESS_AUD: aud }))).status, 403);
  assert.equal((await worker.fetch(req({ 'cf-access-jwt-assertion': 'x.y.z' }), env({ ACCESS_TEAM_DOMAIN: team, ACCESS_AUD: aud }))).status, 403);
  assert.equal(served, 0);
});
