import { describe, expect, it } from 'vitest';
import { kernelApproveRequestId, KERNEL_RESOLVE_PATH, mintPresenceProof } from './presence.js';

const SECRET = '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef';
const body = (v: unknown) => new TextEncoder().encode(JSON.stringify(v)).buffer as ArrayBuffer;

describe('kernel approval presence proof (CONTRACT.md 12.3)', () => {
  it('matches allternit-api (same vector as authority/presence.rs)', () => {
    expect(mintPresenceProof(SECRET, 'apr_1', 1_800_000_060)).toBe(
      'v1.1800000060.f92933da31497666592fd04d78854e07ae0cc0951d689671ae0ee62650baabcb',
    );
  });

  it('gates only an approve sent to the resolve route', () => {
    expect(kernelApproveRequestId('POST', KERNEL_RESOLVE_PATH, body({ request_id: 'apr_1', decision: 'approve' }))).toBe('apr_1');
    expect(kernelApproveRequestId('POST', KERNEL_RESOLVE_PATH, body({ request_id: 'apr_1', decision: 'deny' }))).toBeNull();
    expect(kernelApproveRequestId('POST', '/api/v1/kernel/approvals/request', body({ request_id: 'apr_1', decision: 'approve' }))).toBeNull();
    expect(kernelApproveRequestId('GET', KERNEL_RESOLVE_PATH, undefined)).toBeNull();
    expect(kernelApproveRequestId('POST', KERNEL_RESOLVE_PATH, new TextEncoder().encode('not json').buffer as ArrayBuffer)).toBeNull();
  });
});
