// Offline driver: a scripted ChatGPT dots UI built from fixtures/markup.ts. Implements DotsDriver so the provider +
// runConformance run with no browser, no network, no ChatGPT account.
import { DriverError, type ClickOptions, type DotsDriver } from "./driver.js";
import { NAMES } from "./selectors.js";
import { classify } from "./observe.js";
import { renderPage, type Scenario } from "./fixtures/markup.js";

export type ReplayMode = "normal" | "down" | "rate_limited" | "blocked" | "logged_out" | "drift" | "paused";
export interface ReplayOptions {
  mode?: ReplayMode;
  reply?: (userText: string) => string;
  /** Render frames per reply (streaming for frames-1 html() reads). */
  frames?: number;
  dots?: Array<{ id: string; name: string; handle?: string }>;
  /** A confirmation card that stays on screen until answered (Ask first) or forever (Hand off). */
  confirmation?: { policy: "ask_first" | "hand_off"; text: string };
  tasks?: Array<{ title: string; state: "in_progress" | "scheduled" | "completed" }>;
}
type Turn = { role: "user" | "assistant"; text: string };

export class ReplayDotsDriver implements DotsDriver {
  mode: ReplayMode;
  dots: NonNullable<ReplayOptions["dots"]>;
  confirmation: ReplayOptions["confirmation"];
  confirmationResolution: "approved" | "denied" | undefined;
  sends = 0;
  private view: "list" | "dot" = "list";
  private current = "";
  private threads = new Map<string, Turn[]>(); // a dot's conversation persists across opens
  private tasksShown = false;
  private composer = "";
  private phase = 0; private full = ""; private active = false; private stopped = false; private visibleLen = 0;
  constructor(private opts: ReplayOptions = {}) {
    this.mode = opts.mode ?? "normal"; this.confirmation = opts.confirmation;
    this.dots = opts.dots ?? [{ id: "nova-dot", name: "Nova", handle: "@nova-dot" }];
  }

  /** The id the provider will derive for the on-screen confirmation (same observation path). */
  get confirmationId() { return this.confirmation ? classify(renderPage({ confirmation: this.confirmation })).confirmations[0]?.id : undefined; }
  async connect() { if (this.mode === "down") throw new DriverError("unreachable", "replay: browser unreachable"); }
  async isAppRunning() { return this.mode !== "down"; }
  async showDotList() { this.view = "list"; return true; }
  async openDot(id: string) {
    if (!this.dots.some((d) => d.id === id)) return false;
    this.view = "dot"; this.current = id; this.tasksShown = false; this.composer = ""; this.active = false; return true;
  }
  async showTasks() { this.tasksShown = true; return true; }
  async typeText(t: string) { this.composer = t; return true; }

  private scenario(): Scenario {
    const dot = this.dots.find((d) => d.id === this.current);
    const frames = this.opts.frames ?? 3;
    let turns = [...(this.threads.get(this.current) ?? [])];
    let streaming = false;
    if (this.active) {
      if (!this.stopped && this.phase < frames) this.phase += 1;
      streaming = !this.stopped && this.phase < frames;
      this.visibleLen = this.stopped ? this.visibleLen : Math.ceil((this.full.length * Math.min(this.phase, frames)) / frames);
      turns = [...turns, { role: "assistant", text: this.full.slice(0, this.visibleLen) }];
      if (!streaming) { this.threads.set(this.current, turns); this.active = false; }
    }
    const s: Scenario = {
      view: this.view, dots: this.dots, dot, turns, streaming, composerText: this.composer, confirmation: this.confirmation,
      tasks: this.tasksShown ? this.opts.tasks ?? [] : undefined, showTasks: this.tasksShown,
      activity: streaming ? "Working" : undefined, drift: this.mode === "drift" ? "composer" : undefined,
    };
    if (this.mode === "rate_limited") s.banner = "You've reached your usage limit. Your quota resets at 14:00.";
    if (this.mode === "paused") s.banner = "Nova has been paused for safety monitoring.";
    if (this.mode === "blocked") s.challenge = true;
    if (this.mode === "logged_out") s.loggedOut = true;
    return s;
  }
  async html() {
    if (this.mode === "down") throw new DriverError("unreachable", "replay: browser unreachable");
    return renderPage(this.scenario());
  }
  async clickButton(nameSource: string, o?: ClickOptions) {
    void o;
    if (nameSource === NAMES.send) {
      if (!this.composer.trim() || this.active) return false;
      const text = this.composer; this.composer = ""; this.sends += 1;
      const th = this.threads.get(this.current) ?? []; th.push({ role: "user", text }); this.threads.set(this.current, th);
      this.full = (this.opts.reply ?? ((u) => `Echo: ${u}`))(text); this.phase = 0; this.visibleLen = 0; this.active = true; this.stopped = false;
      return true;
    }
    if (nameSource === NAMES.stop) { if (!this.active || this.stopped) return false; this.stopped = true; return true; }
    if (nameSource === NAMES.approve || nameSource === NAMES.deny) {
      if (!this.confirmation || this.confirmation.policy !== "ask_first") return false; // Hand off has no approve button
      this.confirmationResolution = nameSource === NAMES.approve ? "approved" : "denied"; this.confirmation = undefined; return true;
    }
    return false;
  }
  async dispose() {}
}
