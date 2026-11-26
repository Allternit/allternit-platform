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
  /** Ask the person to review and sign a PDF. Nothing reaches the app until they Sign & send; it never gets strokes or the signature image. Rejects with -32602 (invalid params) or -32000 (denied). */
  requestSignature(args: RequestSignatureParams): Promise<RequestSignatureResult>;
}

/** A signing area in PDF points, origin bottom-left, page 0-based. */
export interface SignSlotInput {
  id: string;
  label: string;
  page: number;
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface RequestSignatureParams {
  title: string;
  summary?: string;
  /** At most 8 rows. */
  details?: { label: string; value: string }[];
  slots?: SignSlotInput[];
  send_to?: { name?: string; email: string };
  /** PDF only, max 20 MB. */
  document:
    | { name: string; mime_type: "application/pdf"; data_base64: string }
    | { file_id: string };
}

export type RequestSignatureResult =
  | {
      status: "signed";
      request_id: string;
      signed_at: string;
      document_sha256: string;
      signed_sha256: string;
      signed_file_id: string;
      signed_url: string;
      signer: { name: string };
    }
  | { status: "declined" | "cancelled"; request_id: string };

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
