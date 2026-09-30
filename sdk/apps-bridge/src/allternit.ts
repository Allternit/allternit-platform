/** Types for `window.allternit`, the optional host extensions installed by runtime/allternit-ext.js. */

export interface AllternitHost {
  version: number;
  /** Resolves true when the host advertises `x-allternit`. */
  detect(): Promise<boolean>;
  supports(method: string): boolean;
  saveArtifact(args: Record<string, unknown>): Promise<unknown>;
  openInAci(args: Record<string, unknown>): Promise<unknown>;
  dispatchAgent(args: Record<string, unknown>): Promise<unknown>;
  handoffToComputer(args: Record<string, unknown>): Promise<unknown>;
  requestCheckout(args: Record<string, unknown>): Promise<unknown>;
}

declare global {
  interface Window {
    allternit?: AllternitHost;
  }
}

/** The host extensions when the host provides them, else null. Never throws. */
export async function getAllternit(win: Window = window): Promise<AllternitHost | null> {
  const host = win.allternit;
  if (!host) return null;
  try {
    return (await host.detect()) ? host : null;
  } catch {
    return null;
  }
}
