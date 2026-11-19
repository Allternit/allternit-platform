/**
 * Pairing window state: which phase the sign-in-to-pair flow is in, which of
 * the three steps is done, and the plain-language card a failure shows.
 *
 * Pure (no electron imports) so the step mapping and the error copy are
 * unit-tested. Raw HTTP and IPC text never reaches the window: it goes to the
 * log, and the person sees one of the cards below.
 */

export type PairingPhase = 'welcome' | 'signing-in' | 'approving' | 'paired' | 'error';

/** The three rows of the pairing card, in order. */
export const PAIRING_STEPS = [
  'Sign in to your Allternit account',
  'Approve this computer',
  'Start Allternit',
] as const;

export type PairingAction = 'retry' | 'sign-in-again' | 'status' | 'devices';

export interface PairingErrorView {
  /** The step that failed: 0 = sign-in, 1 = approve. */
  step: 0 | 1;
  title: string;
  body: string;
  primary: 'retry' | 'sign-in-again';
  secondary?: PairingAction;
}

export interface PairingState {
  phase: PairingPhase;
  /** Account email, shown once paired. */
  email?: string;
  error?: PairingErrorView;
}

export const PAIRING_ACTION_LABELS: Record<PairingAction, string> = {
  retry: 'Try again',
  'sign-in-again': 'Sign in again',
  status: 'Check Allternit status',
  devices: 'Manage your computers',
};

/**
 * Index of the step in progress: rows before it show a check, this row a
 * spinner, rows after it an empty dot. -1 = nothing started, 3 = all done.
 */
export function pairingStepAt(state: PairingState): number {
  switch (state.phase) {
    case 'welcome': return -1;
    case 'signing-in': return 0;
    case 'approving': return 1;
    case 'paired': return PAIRING_STEPS.length;
    case 'error': return state.error?.step ?? 1;
  }
}

export type PairingFailureKind =
  | 'cancelled'
  | 'expired'
  | 'auth'
  | 'sign-in'
  | 'server'
  | 'limit'
  | 'revoked'
  | 'network'
  | 'unknown';

/**
 * A pairing failure that already knows its kind. `message` is for the log;
 * the window shows the card for `kind`.
 */
export class PairingFailure extends Error {
  constructor(
    readonly kind: PairingFailureKind,
    message: string,
    readonly status?: number,
  ) {
    super(message);
    this.name = 'PairingFailure';
  }
}

const LIMIT_PATTERN = /machine limit|computer limit|runtime limit|MACHINE_LIMIT|RUNTIME_LIMIT/i;

/**
 * Kind of a failed cloud call during pairing (create, lookup, approve,
 * exchange). `serverText` is the response's `code`/`message`/`error`, used
 * only to spot the plan's computer limit.
 */
export function pairingFailureKindForStatus(status: number, serverText = ''): PairingFailureKind {
  if (LIMIT_PATTERN.test(serverText)) return 'limit';
  if (status === 404 || status === 410 || /TOKEN_EXPIRED/.test(serverText)) return 'expired';
  if (status === 401 || status === 403) return 'auth';
  if (status >= 500) return 'server';
  return 'unknown';
}

/** Kind of any error that ended a pairing attempt. */
export function classifyPairingError(error: unknown): { kind: PairingFailureKind; status?: number } {
  if (error instanceof PairingFailure) return { kind: error.kind, status: error.status };
  // fetch() rejects with TypeError when the network is down or DNS fails.
  if (error instanceof TypeError) return { kind: 'network' };
  const message = error instanceof Error ? error.message : typeof error === 'string' ? error : '';
  if (/window was closed|interrupted by sign-out|replaced by a visible sign-in/i.test(message)) return { kind: 'cancelled' };
  if (LIMIT_PATTERN.test(message)) return { kind: 'limit' };
  if (/revoked/i.test(message)) return { kind: 'revoked' };
  if (/expired|timed out/i.test(message)) return { kind: 'expired' };
  if (/fetch failed|ENOTFOUND|ECONNREFUSED|ECONNRESET|ETIMEDOUT|network/i.test(message)) return { kind: 'network' };
  return { kind: 'unknown' };
}

/**
 * The card a failure shows, or null when the person cancelled (the window
 * goes back to Welcome with no error). `failedStep` is the step that was in
 * progress when it failed.
 */
export function pairingErrorView(error: unknown, failedStep: 0 | 1 = 1): PairingErrorView | null {
  const { kind, status } = classifyPairingError(error);
  switch (kind) {
    case 'cancelled':
      return null;
    case 'server':
      return {
        step: 1,
        title: "Couldn't approve this computer",
        body: status
          ? `Allternit had a problem on its side (error ${status}). You did nothing wrong. Try again in a minute.`
          : 'Allternit had a problem on its side. You did nothing wrong. Try again in a minute.',
        primary: 'retry',
        secondary: 'status',
      };
    case 'expired':
      return {
        step: failedStep,
        title: 'This pairing request expired',
        body: 'It ran out before pairing finished. Try again to start a new one.',
        primary: 'retry',
      };
    case 'auth':
      return {
        step: 0,
        title: "Couldn't confirm your sign-in",
        body: 'Allternit could not confirm your account. Sign in again to pair this computer.',
        primary: 'sign-in-again',
        secondary: 'retry',
      };
    case 'sign-in':
      return {
        step: 0,
        title: "Sign-in didn't finish",
        body: 'The sign-in window had a problem. Try again.',
        primary: 'retry',
        secondary: 'status',
      };
    case 'limit':
      return {
        step: 1,
        title: "Your plan's computer limit is reached",
        body: 'Remove a computer you no longer use, then try again.',
        primary: 'retry',
        secondary: 'devices',
      };
    case 'revoked':
      return {
        step: 1,
        title: 'This computer was removed',
        body: 'It was removed from your Allternit account. Pair it again to keep using it.',
        primary: 'retry',
      };
    case 'network':
      return {
        step: failedStep,
        title: "Can't reach Allternit",
        body: "Check this computer's internet connection, then try again.",
        primary: 'retry',
        secondary: 'status',
      };
    case 'unknown':
      return {
        step: failedStep,
        title: "Couldn't pair this computer",
        body: 'Something went wrong while pairing. Try again. If it keeps happening, check Allternit status.',
        primary: 'retry',
        secondary: 'status',
      };
  }
}

/** Display name of the OS for the pairing card's subtitle. */
export function platformLabel(platform: NodeJS.Platform): string {
  if (platform === 'darwin') return 'macOS';
  if (platform === 'win32') return 'Windows';
  if (platform === 'linux') return 'Linux';
  return platform;
}
