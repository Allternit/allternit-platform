// Helpers for adapter drivers built on an AxDriver: attach + error mapping, button lookup/press, composer write,
// and observer-event normalization (AX notifications -> transport-neutral hints).
import type { AxDriver } from "./bridge.js";
import { AX_NOT_TRUSTED_MESSAGE, AxError, labelOf, textUnder, walk, type AxEvent, type AxNode, type AxSnapshot } from "./types.js";

export const BUTTON_ROLES = new Set(["AXButton", "AXRadioButton", "AXTab", "AXCheckBox", "AXMenuItem", "AXLink", "AXPopUpButton"]);

/** Attach once (idempotent); a missing permission surfaces as AxError not_trusted for the adapter to map to AUTH_REQUIRED. */
export async function ensureAttached(ax: AxDriver, bundleId: string, state: { attached: boolean }) {
  if (state.attached) return;
  if (!(await ax.trust())) throw new AxError("not_trusted", "Accessibility permission not granted");
  await ax.attach(bundleId);
  state.attached = true;
}

/** Static text plus button labels under a node (what a person reads on a card, buttons included). */
export function fullText(n: AxNode): string {
  const parts: string[] = [];
  for (const { node } of walk(n)) {
    if (node.role === "AXStaticText" || node.role === "AXHeading") { const t = node.value ?? node.title ?? node.description; if (t) parts.push(t); }
    else if (BUTTON_ROLES.has(node.role)) { const t = buttonLabel(node); if (t) parts.push(t); }
  }
  return parts.join(" ");
}
export const buttonLabel = (n: AxNode) => (n.title || n.description || (typeof n.value === "string" && !/^\d+$/.test(n.value) ? n.value : "") || "").trim();

/** Buttons (or tab/radio) in the tree whose accessible name matches `nameSource` (case-insensitive). */
export function findButtons(root: AxNode, nameSource: string, withinText?: string): AxNode[] {
  const re = new RegExp(nameSource, "i");
  // Whitespace-insensitive: DOM textOf() concatenates block text without spaces, AX joins with spaces.
  const within = withinText ? new RegExp(withinText.replace(/\s+/g, ""), "i") : undefined;
  const out: AxNode[] = [];
  for (const { node, ancestors } of walk(root)) {
    if (!BUTTON_ROLES.has(node.role) || !re.test(buttonLabel(node))) continue;
    if (within && !ancestors.some((a) => a.role !== "AXWindow" && a.role !== "AXApplication" && a.role !== "AXWebArea" && within.test(fullText(a).replace(/\s+/g, "")))) continue;
    out.push(node);
  }
  return out;
}
export async function pressButton(ax: AxDriver, snap: AxSnapshot, nameSource: string, withinText?: string): Promise<boolean> {
  const b = findButtons(snap.root, nameSource, withinText)[0];
  return b ? ax.press(b.path) : false;
}
/** Write text into a composer node and read it back: an AX setValue that the web view ignores must read as false, not "sent". */
export async function writeComposer(ax: AxDriver, node: AxNode | undefined, text: string, reread: () => Promise<AxNode | undefined>): Promise<boolean> {
  if (!node) return false;
  await ax.focus(node.path);
  if (!(await ax.setValue(node.path, text))) return false;
  const back = await reread();
  return (back?.value ?? "") === text;
}

// ---- observer normalization ----
export type AxHintKind = "text_changed" | "element_added" | "focus_moved" | "other";
export interface AxHint { kind: AxHintKind; role?: string; text?: string; at: number }
export function normalizeAxEvent(e: AxEvent): AxHint {
  const text = (e.value ?? e.title ?? e.description ?? "").trim() || undefined;
  const kind: AxHintKind = e.event === "AXValueChanged" ? "text_changed" : e.event === "AXUIElementCreated" ? "element_added" : e.event === "AXFocusedUIElementChanged" ? "focus_moved" : "other";
  return { kind, role: e.role, text, at: e.ts };
}
/** Bounded buffer of normalized hints; `changedSince` lets a poller skip re-snapshotting an idle app. */
export class AxHintBuffer {
  private items: AxHint[] = [];
  constructor(private cap = 200) {}
  push(e: AxEvent) { this.items.push(normalizeAxEvent(e)); if (this.items.length > this.cap) this.items.shift(); }
  drain(): AxHint[] { const o = this.items; this.items = []; return o; }
  get size() { return this.items.length; }
  changedSince(ts: number) { return this.items.some((h) => h.at > ts && h.kind !== "focus_moved"); }
}
export { labelOf };

/** Map a bridge/replay AxError to the adapters' DriverError vocabulary. */
export function axFault(e: unknown): { fault: "not_trusted" | "not_running" | "unreachable"; message: string } | undefined {
  if (!(e instanceof AxError)) return undefined;
  if (e.fault === "not_trusted") return { fault: "not_trusted", message: AX_NOT_TRUSTED_MESSAGE };
  if (e.fault === "not_running") return { fault: "not_running", message: e.message };
  return { fault: "unreachable", message: e.message };
}
