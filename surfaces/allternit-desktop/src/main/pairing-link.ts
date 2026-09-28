/**
 * `gizzi login` from a terminal on this Mac opens allternit://pair?code=…
 * so the pairing is approved in the signed-in app (see unified-main
 * handleProtocolCallback). Only a plausible pairing code is accepted.
 */
export function isPairingLink(url: string): boolean {
  return url.startsWith('allternit://pair') || url.startsWith('allternit-dev://pair');
}

export function pairingCodeFromUrl(url: string): string | null {
  try {
    const code = new URL(url).searchParams.get('code')?.trim() ?? '';
    return /^[A-Za-z0-9-]{4,32}$/.test(code) ? code : null;
  } catch {
    return null;
  }
}
