/**
 * Renewing the cloud (Clerk) sign-in of a paired device: only the account the
 * device is paired to may be used, or cloud calls would act for someone else.
 */

export type CloudReconnectResult = { ok: true } | { ok: false; error: string };

const SYNTHETIC_EMAIL_SUFFIX = '@users.allternit.local';

/** Why a renewed Clerk sign-in can't be used for this device, or null when it can. */
export function cloudAccountMismatch(paired: { userId: string; userEmail?: string }, signedInUserId: string): string | null {
  if (!signedInUserId) return 'Sign-in did not return an account.';
  if (signedInUserId === paired.userId) return null;
  const email = paired.userEmail;
  const who = email && email.includes('@') && !email.endsWith(SYNTHETIC_EMAIL_SUFFIX) ? email : 'the account this device is paired to';
  return `That is a different account. Sign in as ${who}.`;
}
