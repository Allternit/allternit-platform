// Helpers for adapter drivers built on an AxDriver: attach + error mapping, button lookup/press, composer write,
// and observer-event normalization (AX notifications -> transport-neutral hints).
import type { AxDriver } from "./bridge.js";
import { AxError, labelOf, textUnder, walk, type AxEvent, type AxNode, type AxSnapshot } from "./types.js";

export const BUTTON_ROLES = new Set(["AXButton", "AXRadioButton", "AXTab", "AXCheckBox", "AXMenuItem", "AXLink", "AXPopUpButton"]);

/** Attach once (idempotent); a missing permission surfaces as AxError not_trusted for the adapter to map to AUTH_REQUIRED. */
export async function ensureAttached(ax: AxDriver, bundleId: string, state: { attached: boolean }) {
  if (state.attached) return;
  if (!(await ax.trust())) throw new AxError("not_trusted", "Accessibility permission not granted");
  await ax.attach(bundleId);
  state.attached = true;
}

export const buttonLabel = (n: AxNode) => (n.title || n.description || (typeof n.value === "string" && !/^\d+$/.test(n.value) ? n.value : "") || "").trim();

/** Buttons (or tab/radio) in the tree whose accessible name matches `nameSource` (case-insensitive). */
export function findButtons(root: AxNode, nameSource: string, withinText?: string): AxNode[] {
  const re = new RegExp(nameSource, "i");
  const within = withinText ? new RegExp(withinText, "i") : undefined;
  const out: AxNode[] = [];
  for (const { node, ancestors } of walk(root)) {
    if (!BUTTON_ROLES.has(node.role) || !re.test(buttonLabel(node))) continue;
    if (within && !ancestors.some((a) => a.role !== "AXWindow" && a.role !== "AXApplication" && a.role !== "AXWebArea" && within.test(textUnder(a)))) continue;
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
