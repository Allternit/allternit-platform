import { test } from 'node:test';
import assert from 'node:assert/strict';
import { rewriteLocation } from './location.js';

const ai = new URL('https://ai.allternit.com/__clerk/v1/oauth_callback?state=s&code=c');
const apex = new URL('https://allternit.com/__clerk/v1/oauth_callback?state=s&code=c');

test('relative FAPI redirect keeps the proxy prefix', () => {
  assert.equal(
    rewriteLocation('/v1/oauth_callback?err_code=authorization_invalid#', apex),
    'https://allternit.com/__clerk/v1/oauth_callback?err_code=authorization_invalid',
  );
  assert.equal(
    rewriteLocation('/v1/client/handshake?x=1', ai),
    'https://ai.allternit.com/__clerk/v1/client/handshake?x=1',
  );
});

test('absolute FAPI redirect is proxied on the request origin', () => {
  assert.equal(
    rewriteLocation('https://clerk.allternit.com/v1/verify?token=t', ai),
    'https://ai.allternit.com/__clerk/v1/verify?token=t',
  );
});

test('clerk-js 307 to the configured proxy host stays on the request origin', () => {
  assert.equal(
    rewriteLocation('https://allternit.com/__clerk/npm/@clerk/clerk-js@5.128.0/dist/clerk.browser.js', ai),
    'https://ai.allternit.com/__clerk/npm/@clerk/clerk-js@5.128.0/dist/clerk.browser.js',
  );
});

test('instance sign-in / sign-up URL is an app page, never /__clerk/sign-in', () => {
  assert.equal(
    rewriteLocation('https://allternit.com/sign-in?redirect_url=%2Fshell', ai),
    'https://ai.allternit.com/sign-in?redirect_url=%2Fshell',
  );
  assert.equal(
    rewriteLocation('https://allternit.com/sign-up%20%20%20%20', new URL('https://platform.allternit.com/__clerk/v1/x')),
    'https://platform.allternit.com/sign-up',
  );
  assert.equal(
    rewriteLocation('https://allternit.com/sign-in/factor-one', ai),
    'https://ai.allternit.com/sign-in/factor-one',
  );
});

test('on the apex the sign-in URL is left for the www site to redirect', () => {
  assert.equal(rewriteLocation('https://allternit.com/sign-in', apex), 'https://allternit.com/sign-in');
});

test('other absolute app URLs pass through untouched', () => {
  for (const u of [
    'https://ai.allternit.com/sign-in/sso-callback',
    'https://allternit.com',
    'https://allternit.com/pricing',
    'https://accounts.google.com/o/oauth2/auth?client_id=x',
  ]) {
    assert.equal(rewriteLocation(u, apex), u);
    assert.equal(rewriteLocation(u, ai), u);
  }
});
