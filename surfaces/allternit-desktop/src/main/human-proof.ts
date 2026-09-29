/**
 * D16 human proof in Desktop.
 *
 * The UI marks a request that is the person's own act (approving a
 * subscription card, sending to a subscription model, acknowledging the
 * provider terms) with `X-Allternit-Human-Proof: desktop`. Electron main
 * swaps that marker for `desktop:<per-launch secret>` on requests from the
 * app window only; allternit-api got the secret on its stdin and accepts it
 * as a person's proof. Agents never pass through Electron main and can't read
 * its memory, so they can't produce it. Any other `desktop…` value is
 * removed: only main writes one. A Clerk session token (web, phone) is left
 * untouched.
 *
 * The window's `/api` requests are redirected to `allternit-api://` in
 * onBeforeRequest, and Chromium drops custom request headers on that
 * redirect, so the UI also marks the URL (`?allternit_person=desktop`). Only
 * the protocol handler turns it into the proof; the API never trusts it.
 */
export const HUMAN_PROOF_HEADER = 'X-Allternit-Human-Proof';
export const DESKTOP_PROOF_MARKER = 'desktop';

type HeaderBag = Record<string, string | string[]>;

/** Rewrite the marker in an Electron `requestHeaders` object in place. */
export function applyDesktopHumanProof(headers: HeaderBag, proof: string | null): void {
  for (const key of Object.keys(headers)) {
    if (key.toLowerCase() !== HUMAN_PROOF_HEADER.toLowerCase()) continue;
    const value = String(headers[key]).trim();
    if (!value.toLowerCase().startsWith(DESKTOP_PROOF_MARKER)) continue;
    delete headers[key];
    if (value === DESKTOP_PROOF_MARKER && proof) headers[HUMAN_PROOF_HEADER] = proof;
  }
}

/** Same, for a fetch `Headers` object. */
export function applyDesktopHumanProofTo(headers: Headers, proof: string | null): void {
  const value = headers.get(HUMAN_PROOF_HEADER)?.trim();
  if (!value || !value.toLowerCase().startsWith(DESKTOP_PROOF_MARKER)) return;
  headers.delete(HUMAN_PROOF_HEADER);
  if (value === DESKTOP_PROOF_MARKER && proof) headers.set(HUMAN_PROOF_HEADER, proof);
}

/** A request relayed from another device never carries Desktop's proof. */
export function stripDesktopHumanProof(headers: Headers): void {
  applyDesktopHumanProofTo(headers, null);
}

/** URL marker for the same person-act, which survives the /api redirect. */
export const DESKTOP_PROOF_PARAM = 'allternit_person';

/** Remove the URL marker; true when it marked a person's act. */
export function takeDesktopProofParam(url: URL): boolean {
  const value = url.searchParams.get(DESKTOP_PROOF_PARAM);
  if (value === null) return false;
  url.searchParams.delete(DESKTOP_PROOF_PARAM);
  return value === DESKTOP_PROOF_MARKER;
}

/** Strip the URL marker from a relayed path (another device never gets it). */
export function stripDesktopProofParam(requestPath: string): string {
  const url = new URL(requestPath, 'http://relay.invalid');
  if (!url.searchParams.has(DESKTOP_PROOF_PARAM)) return requestPath;
  url.searchParams.delete(DESKTOP_PROOF_PARAM);
  return `${url.pathname}${url.search}`;
}
