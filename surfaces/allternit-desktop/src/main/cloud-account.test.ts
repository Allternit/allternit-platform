import { describe, expect, it } from 'vitest';
import { cloudAccountMismatch } from './cloud-account.js';

describe('cloudAccountMismatch', () => {
  const paired = { userId: 'user_a', userEmail: 'a@example.com' };

  it('accepts the account the device is paired to', () => {
    expect(cloudAccountMismatch(paired, 'user_a')).toBeNull();
  });

  it('refuses another account and names the paired one', () => {
    expect(cloudAccountMismatch(paired, 'user_b')).toBe('That is a different account. Sign in as a@example.com.');
  });

  it('does not show a synthetic device email', () => {
    const synthetic = { userId: 'user_a', userEmail: 'user_a@users.allternit.local' };
    expect(cloudAccountMismatch(synthetic, 'user_b')).toBe('That is a different account. Sign in as the account this device is paired to.');
  });

  it('refuses a sign-in without an account', () => {
    expect(cloudAccountMismatch(paired, '')).toBe('Sign-in did not return an account.');
  });
});
