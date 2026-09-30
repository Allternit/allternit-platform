// AxReplayDriver: an AxDriver over recorded/hand-built snapshots. Offline, no bridge, no app.
//  - static: pass snapshots[]; each snapshot() call advances, the last repeats.
//  - scripted: pass `frame()` (current tree) and `onAction` to mutate scripted app state on press/setValue/focus.
import type { AxDriver } from "./bridge.js";
import { AX_SNAPSHOT_FORMAT, AxError, DEFAULT_OBSERVE, walk, type AxEvent, type AxNode, type AxSnapshot } from "./types.js";

export interface AxAction { kind: "press" | "setValue" | "focus"; path: number[]; node?: AxNode; value?: string }
export interface AxReplayOptions {
  bundleId?: string;
  trusted?: boolean;
  running?: boolean;
  snapshots?: AxSnapshot[];
  frame?: () => AxNode;
  onAction?: (a: AxAction) => boolean | void;
}
export class AxReplayDriver implements AxDriver {
  actions: AxAction[] = [];
  attached = false;
  observed: string[] = [];
  private i = 0;
  private listeners = new Set<(e: AxEvent) => void>();
  constructor(private o: AxReplayOptions = {}) {}
  async trust() { return this.o.trusted !== false; }
  async attach(bundleId: string) {
    if (this.o.trusted === false) throw new AxError("not_trusted", "Accessibility permission not granted");
    if (this.o.running === false) throw new AxError("not_running", `${bundleId} is not running`);
    this.attached = true; return { pid: 1, manualAccessibility: true };
  }
  private tree(): AxSnapshot {
    if (this.o.trusted === false) throw new AxError("not_trusted", "Accessibility permission not granted");
    if (!this.attached) throw new AxError("not_attached", "attach first");
    if (this.o.frame) return { formatVersion: AX_SNAPSHOT_FORMAT, bundleId: this.o.bundleId ?? "replay", capturedAt: 0, root: this.o.frame() };
    const s = this.o.snapshots ?? [];
    if (!s.length) throw new AxError("failed", "no snapshots loaded");
    return s[Math.min(this.i++, s.length - 1)];
  }
  async snapshot() { return this.tree(); }
  async find(q: { role?: string; labelRegex?: string }) {
    const re = q.labelRegex ? new RegExp(q.labelRegex, "i") : undefined;
    return [...walk(this.tree().root)].map((x) => x.node).filter((n) => (!q.role || n.role === q.role) && (!re || re.test([n.title, n.description, n.value].filter(Boolean).join(" "))));
  }
  private act(kind: AxAction["kind"], path: number[], value?: string) {
    const node = [...walk(this.tree().root)].find((x) => x.node.path.join(".") === path.join("."))?.node;
    const a: AxAction = { kind, path, node, value }; this.actions.push(a);
    if (!node) return false;
    return this.o.onAction ? this.o.onAction(a) !== false : true;
  }
  async press(p: number[]) { return this.act("press", p); }
  async setValue(p: number[], v: string) { return this.act("setValue", p, v); }
  async focus(p: number[]) { return this.act("focus", p); }
  async observe(l: (e: AxEvent) => void, n: readonly string[] = DEFAULT_OBSERVE) { this.observed = [...n]; this.listeners.add(l); return () => { this.listeners.delete(l); }; }
  /** Test hook: deliver an AX notification to observers. */
  emit(e: AxEvent) { for (const l of this.listeners) l(e); }
  async dispose() { this.listeners.clear(); }
}
