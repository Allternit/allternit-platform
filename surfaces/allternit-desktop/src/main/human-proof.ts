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
