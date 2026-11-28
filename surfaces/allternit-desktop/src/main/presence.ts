/**
 * User presence for kernel approvals on Desktop (CONTRACT.md section 12.3).
 *
 * The D16 window proof (human-proof.ts) says a request came through the app
 * window. On Desktop the model's shell runs as the same OS user, so before an
 * AllternitOS approval is APPROVED, Electron main also asks the OS to confirm
 * a person is at the computer:
 *
 * - macOS: Touch ID (`systemPreferences.promptTouchID`).
 * - Windows: Windows Hello (WinRT `UserConsentVerifier`, through Windows
 *   PowerShell, since Electron has no API for it).
 * - Anywhere neither is set up (Linux, a Mac without Touch ID, a PC without
 *   Hello): the approve is refused with a clear message. Never a silent pass.
 *   The person can approve from the web or phone app, which use their Clerk
 *   session.
 *
 * On success main attaches `X-Allternit-Presence-Proof` (minted below with the
 * per-launch human-proof secret, see backend-manager.ts) and allternit-api
 * checks it (`cmd/allternit-api/src/authority/presence.rs`). Deny needs no
 * presence.
 */
import { execFile } from 'child_process';
import * as crypto from 'crypto';

export const PRESENCE_PROOF_HEADER = 'X-Allternit-Presence-Proof';
export const KERNEL_RESOLVE_PATH = '/api/v1/kernel/approvals/resolve';
/** Proof lifetime; allternit-api refuses anything over 120 s. */
export const PRESENCE_TTL_SECS = 60;
const DOMAIN = 'allternit.presence.v1';

export type PresenceResult =
  | { ok: true; method: 'touch-id' | 'windows-hello' }
  | { ok: false; reason: 'unavailable' | 'cancelled' | 'busy'; detail: string };

/** `v1.<expires>.<hex HMAC-SHA256>` bound to one approval request. */
export function mintPresenceProof(secret: string, requestId: string, expiresUnix: number): string {
  const mac = crypto
    .createHmac('sha256', Buffer.from(secret, 'utf8'))
    .update(`${DOMAIN}\n${requestId}\napprove\n${expiresUnix}`)
    .digest('hex');
  return `v1.${expiresUnix}.${mac}`;
}

/**
 * The approval request id when this is an APPROVE sent to the kernel's resolve
 * route, else null (deny, other routes, unreadable bodies are not gated here;
 * allternit-api refuses a Desktop approve that arrives without a proof).
 */
export function kernelApproveRequestId(method: string, pathname: string, body: ArrayBuffer | undefined): string | null {
  if (method.toUpperCase() !== 'POST' || pathname !== KERNEL_RESOLVE_PATH || !body) return null;
  try {
    const parsed = JSON.parse(Buffer.from(body).toString('utf8')) as { request_id?: unknown; decision?: unknown };
    return parsed.decision === 'approve' && typeof parsed.request_id === 'string' && parsed.request_id.length > 0
      ? parsed.request_id
      : null;
  } catch {
    return null;
  }
}

const WINDOWS_HELLO_SCRIPT = `
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$asTask = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation\`1' })[0]
function Await($op, $type) { $t = $asTask.MakeGenericMethod($type).Invoke($null, @($op)); $t.Wait() | Out-Null; $t.Result }
$v = [Windows.Security.Credentials.UI.UserConsentVerifier, Windows.Security.Credentials.UI, ContentType = WindowsRuntime]
$avail = Await ($v::CheckAvailabilityAsync()) ([Windows.Security.Credentials.UI.UserConsentVerifierAvailability])
if ("$avail" -ne 'Available') { Write-Output "unavailable:$avail"; exit 0 }
$r = Await ($v::RequestVerificationAsync($env:ALLTERNIT_PRESENCE_REASON)) ([Windows.Security.Credentials.UI.UserConsentVerificationResult])
Write-Output "result:$r"
`;

function windowsHello(reason: string): Promise<PresenceResult> {
  return new Promise((resolve) => {
    execFile(
      'powershell.exe',
      ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-Command', WINDOWS_HELLO_SCRIPT],
      { windowsHide: true, timeout: 120_000, env: { ...process.env, ALLTERNIT_PRESENCE_REASON: reason } },
      (error, stdout) => {
        const out = String(stdout ?? '').trim();
        if (out === 'result:Verified') return resolve({ ok: true, method: 'windows-hello' });
        if (out.startsWith('result:')) return resolve({ ok: false, reason: 'cancelled', detail: 'Windows Hello did not confirm it was you.' });
        resolve({
          ok: false,
          reason: 'unavailable',
          detail: error
            ? 'Windows Hello could not be started on this PC.'
            : 'Windows Hello is not set up on this PC.',
        });
      },
    );
  });
}

let inflight = false;

/** Ask the OS to confirm a person is here. One prompt at a time. */
export async function checkUserPresence(reason: string): Promise<PresenceResult> {
  if (inflight) return { ok: false, reason: 'busy', detail: 'Another approval is waiting for confirmation.' };
  inflight = true;
  try {
    if (process.platform === 'darwin') {
      const { systemPreferences } = await import('electron');
      if (!systemPreferences.canPromptTouchID()) {
        return { ok: false, reason: 'unavailable', detail: 'Touch ID is not available on this Mac.' };
      }
      try {
        await systemPreferences.promptTouchID(reason);
        return { ok: true, method: 'touch-id' };
      } catch {
        return { ok: false, reason: 'cancelled', detail: 'Touch ID did not confirm it was you.' };
      }
    }
    if (process.platform === 'win32') return await windowsHello(reason);
    return { ok: false, reason: 'unavailable', detail: 'This computer has no Touch ID or Windows Hello.' };
  } finally {
    inflight = false;
  }
}

/** The refusal the window gets when presence fails (same envelope as allternit-api's kernel errors). */
export function presenceRefusal(result: Extract<PresenceResult, { ok: false }>): Response {
  const message = `${result.detail} Approving on Allternit Desktop needs Touch ID or Windows Hello; you can approve from the Allternit web or phone app instead.`;
  return new Response(
    JSON.stringify({ ok: false, error: { code: 'forbidden_caller', message, presence: result.reason } }),
    { status: 403, headers: { 'Content-Type': 'application/json', 'Access-Control-Allow-Origin': '*' } },
  );
}
