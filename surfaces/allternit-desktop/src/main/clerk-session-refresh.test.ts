import { describe, expect, it } from 'vitest';
import {
  clerkFrontendApiHost,
  clerkSessionFromJwtPayload,
  clerkTokenRequest,
  jwtFromClerkTokenResponse,
} from './clerk-session-refresh.js';

const pk = (host: string, kind = 'live') => `pk_${kind}_${Buffer.from(`${host}$`).toString('base64')}`;
const jwt = (payload: Record<string, unknown>) =>
  `eyJhbGciOiJSUzI1NiJ9.${Buffer.from(JSON.stringify(payload)).toString('base64url')}.sig`;
const decode = (token: string) => {
  try {
    return JSON.parse(Buffer.from(token.split('.')[1] ?? '', 'base64url').toString('utf8')) as Record<string, unknown>;
  } catch {
    return null;
  }
};

describe('clerkFrontendApiHost', () => {
  it('decodes the Frontend API host from live and test keys', () => {
    expect(clerkFrontendApiHost(pk('clerk.allternit.com'))).toBe('clerk.allternit.com');
    expect(clerkFrontendApiHost(pk('foo-123.clerk.accounts.dev', 'test'))).toBe('foo-123.clerk.accounts.dev');
  });

  it('refuses a missing key or one that does not decode to a host', () => {
    expect(clerkFrontendApiHost(undefined)).toBeNull();
    expect(clerkFrontendApiHost('pk_live_')).toBeNull();
    expect(clerkFrontendApiHost(`pk_live_${Buffer.from('evil.com/path$').toString('base64')}`)).toBeNull();
  });
});

describe('clerkSessionFromJwtPayload', () => {
  it('reads the session and the v2 organization claim', () => {
    expect(clerkSessionFromJwtPayload({ sid: 'sess_2abc', o: { id: 'org_9', rol: 'admin' } }))
      .toEqual({ sessionId: 'sess_2abc', organizationId: 'org_9' });
  });

  it('reads the v1 org_id claim, or no organization', () => {
    expect(clerkSessionFromJwtPayload({ sid: 'sess_2abc', org_id: 'org_1' })?.organizationId).toBe('org_1');
    expect(clerkSessionFromJwtPayload({ sid: 'sess_2abc' })?.organizationId).toBeNull();
  });

  it('refuses a token without a well-formed session id (nothing to put in the URL)', () => {
    expect(clerkSessionFromJwtPayload(null)).toBeNull();
    expect(clerkSessionFromJwtPayload({ sub: 'user_1' })).toBeNull();
    expect(clerkSessionFromJwtPayload({ sid: 'sess_../../x' })).toBeNull();
  });
});

describe('clerkTokenRequest', () => {
  it('posts to the session tokens endpoint from the auth window origin, with the organization', () => {
    const req = clerkTokenRequest({
      frontendApiHost: 'clerk.allternit.com',
      originHost: 'accounts.allternit.com',
      sessionId: 'sess_2abc',
      organizationId: 'org_9',
    });
    expect(req.url).toBe('https://clerk.allternit.com/v1/client/sessions/sess_2abc/tokens');
    expect(req.method).toBe('POST');
    expect(req.headers.Origin).toBe('https://accounts.allternit.com');
    expect(req.body).toBe('organizationId=org_9');
  });

  it('sends an empty body without an organization', () => {
    expect(clerkTokenRequest({ frontendApiHost: 'h', originHost: null, sessionId: 'sess_1', organizationId: null }).body).toBe('');
  });
});

describe('jwtFromClerkTokenResponse', () => {
  it('takes the jwt for the same session', () => {
    const token = jwt({ sid: 'sess_2abc', exp: 1 });
    expect(jwtFromClerkTokenResponse({ object: 'token', jwt: token }, 'sess_2abc', decode)).toBe(token);
  });

  it('refuses a token for another session or a malformed response', () => {
    expect(jwtFromClerkTokenResponse({ jwt: jwt({ sid: 'sess_other' }) }, 'sess_2abc', decode)).toBeNull();
    expect(jwtFromClerkTokenResponse({ errors: [{ code: 'authentication_invalid' }] }, 'sess_2abc', decode)).toBeNull();
    expect(jwtFromClerkTokenResponse(null, 'sess_2abc', decode)).toBeNull();
    expect(jwtFromClerkTokenResponse({ jwt: 'not-a-jwt' }, 'sess_2abc', decode)).toBeNull();
  });
});
