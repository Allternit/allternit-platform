/**
 * Approves a terminal's `gizzi login` (allternit://pair?code=…) from the
 * signed-in app. The app's renderer runs on the local UI origin without
 * Clerk, so the web /pair page can't load a session there; instead the main
 * process asks for confirmation in a native dialog and approves with the
 * account's Clerk token. Without a Clerk token, the hosted pairing page opens
 * in the browser, where the account is signed in on the web.
 */

export type PairingRequest = {
  userCode: string;
  name?: string;
  hostname?: string;
  platform?: string;
  capabilities?: string[];
  status?: string;
};

export type PairingApprovalDeps = {
  cloudApiBase: string;
  hostedPairUrl: (code: string) => string;
  getClerkToken: () => Promise<string | null>;
  fetch: (input: string, init?: RequestInit) => Promise<Response>;
  /** Ask the person to approve; resolves true on Approve. */
  confirm: (request: PairingRequest) => Promise<boolean>;
  openBrowser: (url: string) => void;
  notify: (title: string, body: string) => void;
  account?: { email?: string; name?: string };
};

export type PairingApprovalOutcome = 'approved' | 'cancelled' | 'browser' | 'failed';

export async function approvePairing(code: string, deps: PairingApprovalDeps): Promise<PairingApprovalOutcome> {
  const inBrowser = (): PairingApprovalOutcome => {
    deps.openBrowser(deps.hostedPairUrl(code));
    return 'browser';
  };

  const token = await deps.getClerkToken().catch(() => null);
  if (!token) return inBrowser();

  const base = `${deps.cloudApiBase.replace(/\/$/, '')}/api/v1/runtime-pairings/code/${encodeURIComponent(code)}`;
  const auth = { Authorization: `Bearer ${token}` };

  let request: PairingRequest;
  try {
    const response = await deps.fetch(base, { headers: auth });
    if (response.status === 401 || response.status === 403) return inBrowser();
    if (!response.ok) {
      deps.notify('gizzi sign-in', 'This sign-in code is invalid or expired. Run `gizzi login` again.');
      return 'failed';
    }
    request = (await response.json()) as PairingRequest;
  } catch {
    return inBrowser();
  }
  if (request.status === 'approved') {
    deps.notify('gizzi sign-in', 'This terminal is already signed in.');
    return 'approved';
  }

  if (!(await deps.confirm(request))) {
    await deps.fetch(`${base}/deny`, { method: 'POST', headers: auth }).catch(() => undefined);
    return 'cancelled';
  }

  try {
    const response = await deps.fetch(`${base}/approve`, {
      method: 'POST',
      headers: { ...auth, 'Content-Type': 'application/json' },
      body: JSON.stringify({ email: deps.account?.email, name: deps.account?.name }),
    });
    if (response.status === 401 || response.status === 403) return inBrowser();
    if (!response.ok) {
      deps.notify('gizzi sign-in', 'Allternit could not approve this terminal. Try `gizzi login` again.');
      return 'failed';
    }
  } catch {
    return inBrowser();
  }
  deps.notify('gizzi sign-in', 'Approved. The terminal is signing in now.');
  return 'approved';
}

/** Text for the confirmation dialog. */
export function describePairingRequest(request: PairingRequest): string {
  const device = [request.name, request.hostname && request.hostname !== request.name ? request.hostname : null]
    .filter(Boolean)
    .join(' · ');
  return [
    device ? `Device: ${device}` : null,
    `Code: ${request.userCode}`,
    'It will be able to use your Allternit account from the terminal.',
  ].filter(Boolean).join('\n');
}
