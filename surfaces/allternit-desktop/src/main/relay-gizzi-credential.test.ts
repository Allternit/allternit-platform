import { describe, expect, it } from 'vitest';

import { dropRuntimeCredentialForGizzi } from './relay-gizzi-credential.js';
import { URLS } from './config.js';

describe('dropRuntimeCredentialForGizzi', () => {
  const relayed = () =>
    new Headers({
      Authorization: 'Bearer device-token',
      'X-Allternit-Desktop-Access-Token': 'device-token',
      'X-Allternit-User-Id': 'user_1',
    });

  it('strips the device token for relayed gizzi routes (providers, sessions)', () => {
    for (const path of ['/v1/provider', '/v1/session/abc', '/v1/agent/list']) {
      const headers = relayed();
      dropRuntimeCredentialForGizzi(`${URLS.GIZZI}${path}`, headers);
      expect(headers.get('Authorization')).toBeNull();
      expect(headers.get('X-Allternit-Desktop-Access-Token')).toBeNull();
      expect(headers.get('X-Allternit-User-Id')).toBe('user_1');
    }
  });

  it('keeps it for allternit-api, which authenticates the relay with it', () => {
    const headers = relayed();
    dropRuntimeCredentialForGizzi(`${URLS.API}/api/v1/sessions`, headers);
    expect(headers.get('Authorization')).toBe('Bearer device-token');
    expect(headers.get('X-Allternit-Desktop-Access-Token')).toBe('device-token');
  });
});
