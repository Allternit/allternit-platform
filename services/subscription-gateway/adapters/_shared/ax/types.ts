// AX tree types shared by the bridge client, the replay driver and the selector engine. Structure mirrors the JSON that
// native/ax-bridge emits (one node per AXUIElement; `path` = child indices from the app element).
export const AX_SNAPSHOT_FORMAT = 1;
export interface AxNode {
  role: string;
  subrole?: string;
  title?: string;
  description?: string;
  value?: string;
  identifier?: string;
  frame?: { x?: number; y?: number; w?: number; h?: number };
  actions?: string[];
  path: number[];
  children?: AxNode[];
}
export interface AxSnapshot {
  /** Snapshot document format (fixtures + bridge output). Unknown versions are refused, not guessed at. */
  formatVersion: number;
  bundleId: string;
  capturedAt: number;
  root: AxNode;
  truncated?: boolean;
}
export interface AxEvent { event: string; role?: string; value?: string; title?: string; description?: string; ts: number }
export const DEFAULT_OBSERVE = ["AXValueChanged", "AXUIElementCreated", "AXFocusedUIElementChanged"] as const;

export type AxFault = "not_trusted" | "not_running" | "not_attached" | "timeout" | "bridge_unavailable" | "failed" | "selector_version";
export class AxError extends Error {
  constructor(readonly fault: AxFault, message: string) { super(message); this.name = "AxError"; }
}
/** Shown to the user (AAI humanMessage) when the helper is not trusted for Accessibility. */
export const AX_NOT_TRUSTED_MESSAGE =
  "Allternit needs the macOS Accessibility permission to read and operate this app. Open System Settings > Privacy & Security > " +
  "Accessibility, switch on the Allternit helper (ax-bridge), then try again. Allternit never grants this for you and does not touch " +
  "the app until you have.";

export function* walk(n: AxNode, ancestors: AxNode[] = []): Generator<{ node: AxNode; ancestors: AxNode[] }> {
  yield { node: n, ancestors };
  for (const c of n.children ?? []) yield* walk(c, [...ancestors, n]);
}
export const labelOf = (n: AxNode) => [n.title, n.description].filter(Boolean).join(" ").trim();
/** Visible text under a node: static-text values (and titles of text-ish leaves), joined by single spaces. */
export function textUnder(n: AxNode): string {
  const parts: string[] = [];
  for (const { node } of walk(n)) {
    if (node.role === "AXStaticText" || node.role === "AXHeading") { const t = node.value ?? node.title ?? node.description; if (t) parts.push(t); }
  }
  return parts.join(" ").replace(/\s+/g, " ").trim();
}
