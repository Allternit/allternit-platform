// Offline driver: a scripted Grok Bot renderer built from fixtures/markup.ts. Implements GrokDriver so the
// provider + runConformance run with no app, no network, no CDP.
import { DriverError, type ClickOptions, type GrokDriver } from "./driver.js";
import { NAMES } from "./selectors.js";
import { classify } from "./observe.js";
import { renderPage, type Scenario } from "./fixtures/markup.js";

export type ReplayMode = "normal" | "down" | "rate_limited" | "blocked" | "logged_out" | "drift";
export interface ReplayOptions {
  mode?: ReplayMode;
  reply?: (userText: string) => string;
  /** Render frames per reply (streaming for frames-1 html() reads). */
  frames?: number;
  /** A vendor approval card that stays on screen until a button is clicked. */
  approval?: string;
  /** Bots shown by the New-chat picker. When set, newChat() opens the picker until a Bot row or Close is clicked. */
  bots?: string[];
}

export class ReplayGrokDriver implements GrokDriver {
  mode: ReplayMode;
  turns: Array<{ role: "user" | "assistant"; text: string }> = [];
  composer = "";
  approval: string | undefined;
  approvalResolution: "approved" | "denied" | undefined;
  sends = 0;
  pickerOpen = false;
  chosenBot: string | undefined;
  private phase = 0; private full = ""; private active = false; private stopped = false;
  constructor(private opts: ReplayOptions = {}) { this.mode = opts.mode ?? "normal"; this.approval = opts.approval; }

  /** The id the provider will derive for the on-screen approval card (same observation path). */
  get approvalId() { return this.approval ? classify(renderPage({ approval: this.approval })).approvals[0]?.id : undefined; }
  async connect() { if (this.mode === "down") throw new DriverError("unreachable", "replay: renderer unreachable"); }
  async isAppRunning() { return this.mode !== "down"; }
  async newChat() { this.turns = []; this.active = false; this.composer = ""; this.chosenBot = undefined; if (this.opts.bots) this.pickerOpen = true; return true; }
  async typeText(t: string) { this.composer = t; return true; }

  private view(): Scenario {
    const frames = this.opts.frames ?? 3;
    let turns = this.turns.map((t) => ({ ...t }));
    let streaming = false;
    if (this.active) {
      if (!this.stopped && this.phase < frames) this.phase += 1;
      streaming = !this.stopped && this.phase < frames;
      const upto = this.stopped ? this.visibleLen : Math.ceil((this.full.length * Math.min(this.phase, frames)) / frames);
      this.visibleLen = upto;
      turns = [...turns, { role: "assistant", text: this.full.slice(0, upto) }];
      if (!streaming) { this.turns = turns; this.active = false; }
    }
    const s: Scenario = { turns, streaming, composerText: this.composer, approval: this.approval, picker: this.pickerOpen, bots: this.opts.bots, drift: this.mode === "drift" ? "composer" : undefined };
    if (this.mode === "rate_limited") s.banner = "This request has been rate limited, try again shortly";
    if (this.mode === "blocked") s.banner = "Unusual activity detected. Verify you are human to continue";
    if (this.mode === "logged_out") s.loggedOut = true;
    return s;
  }
  private visibleLen = 0;
  async html() {
    if (this.mode === "down") throw new DriverError("unreachable", "replay: renderer unreachable");
    return renderPage(this.view());
  }
  async clickButton(nameSource: string, o?: ClickOptions) {
    void o;
    if (this.pickerOpen) {
      if (new RegExp(NAMES.closePicker, "i").test("close new chat") && nameSource === NAMES.closePicker) { this.pickerOpen = false; return true; }
      const bot = (this.opts.bots ?? []).find((b) => new RegExp(nameSource, "i").test(b));
      if (bot) { this.chosenBot = bot; this.pickerOpen = false; return true; }
    }
    if (nameSource === NAMES.send) {
      if (!this.composer.trim() || this.active) return false;
      const text = this.composer; this.composer = ""; this.sends += 1;
      this.turns.push({ role: "user", text });
      this.full = (this.opts.reply ?? ((u) => `Echo: ${u}`))(text); this.phase = 0; this.visibleLen = 0; this.active = true; this.stopped = false;
      return true;
    }
    if (nameSource === NAMES.stop) { if (!this.active || this.stopped) return false; this.stopped = true; return true; }
    if (nameSource === NAMES.approve || nameSource === NAMES.deny) {
      if (!this.approval) return false;
      this.approvalResolution = nameSource === NAMES.approve ? "approved" : "denied"; this.approval = undefined; return true;
    }
    return false;
  }
  async dispose() {}
}
