// Claude Desktop over the macOS Accessibility tree (no debug port). Implements ClaudeDesktopDriver so the existing
// provider/classifier run unchanged: the AX tree is mapped through CLAUDE_AX_PACK into the same Scenario the offline
// DOM fixtures use, rendered to markup, and classified. Guarantee stays best_effort / inferred.
import { AxError, assertPackCompatible, axFault, ensureAttached, findButtons, missingCritical, pressButton, resolveKey, resolveKeys, textUnder, writeComposer, type AxDriver, type AxNode, type AxSnapshot } from "../../_shared/ax/index.js";
import { DriverError, type ClaudeDesktopDriver, type ClickOptions } from "../driver.js";
import { NAMES } from "../selectors.js";
import { renderPage, type Scenario } from "../fixtures/markup.js";
import { CLAUDE_AX_PACK, CLAUDE_BUNDLE_ID } from "./selectors.js";

export class AxClaudeDesktopDriver implements ClaudeDesktopDriver {
  private st = { attached: false };
  /** Why the last html() read as drift (selector evidence for humans/logs). */
  lastDrift: { missing: string[]; packVersion: string; reason: string } | undefined;
  constructor(private ax: AxDriver, private pack = CLAUDE_AX_PACK) {}

  private async guard<T>(f: () => Promise<T>): Promise<T> {
    try { return await f(); } catch (e) {
      const m = axFault(e); if (m) throw new DriverError(m.fault, m.message);
      throw e;
    }
  }
  async connect() { await this.guard(() => ensureAttached(this.ax, CLAUDE_BUNDLE_ID, this.st)); }
  async isAppRunning() {
    try { await ensureAttached(this.ax, CLAUDE_BUNDLE_ID, this.st); return true; }
    catch (e) { return !(e instanceof AxError && e.fault === "not_running"); }
  }
  private snap(): Promise<AxSnapshot> { return this.guard(async () => { await ensureAttached(this.ax, CLAUDE_BUNDLE_ID, this.st); return this.ax.snapshot(); }); }

  /** Pure: AX tree -> Scenario (exported for tests). */
  toScenario(root: AxNode): Scenario {
    try { assertPackCompatible(this.pack); } catch (e) {
      this.lastDrift = { missing: [], packVersion: this.pack.packVersion, reason: (e as Error).message };
      return { drift: "composer" };
    }
    const composer = resolveKey(root, this.pack, "composer")[0];
    if (!composer) {
      if (findButtons(root, "^(log in|sign in|continue with (google|email))$").length) { this.lastDrift = undefined; return { loggedOut: true }; }
      this.lastDrift = { missing: missingCritical(root, this.pack), packVersion: this.pack.packVersion, reason: "composer not found by any AX selector" };
      return { drift: "composer" };
    }
    this.lastDrift = undefined;
    const streaming = findButtons(root, NAMES.stop).length > 0;
    const turns = resolveKeys(root, this.pack, ["userTurn", "assistantTurn"]).map(({ key, node }) => ({ role: key === "userTurn" ? "user" as const : "assistant" as const, text: textUnder(node) }));
    const text = (k: string) => resolveKey(root, this.pack, k).map(textUnder).filter(Boolean);
    const approval = resolveKey(root, this.pack, "approval")[0];
    // The DOM classifier re-wraps the card as "Allow Claude to use <x>?", so hand it only the subject.
    const approvalText = approval ? textUnder(approval).replace(/^allow claude to use\s*/i, "").replace(/\?\s*$/, "") : undefined;
    const cowork = resolveKey(root, this.pack, "coworkTab")[0];
    return {
      turns, streaming, composerText: composer.value ?? "",
      banner: text("alert").join(" | ") || undefined,
      approval: approvalText, tool: text("toolCue")[0], artifact: text("artifactCue")[0],
      cowork: cowork ? cowork.value === "1" || cowork.value === "true" : false,
    };
  }
  async html() { return renderPage(this.toScenario((await this.snap()).root)); }

  async newChat() { return this.guard(async () => pressButton(this.ax, await this.snap(), NAMES.newChat)); }
  async typeText(text: string) {
    return this.guard(async () => {
      const find = async () => resolveKey((await this.snap()).root, this.pack, "composer")[0];
      return writeComposer(this.ax, await find(), text, find);
    });
  }
  async clickButton(nameSource: string, o?: ClickOptions) { return this.guard(async () => pressButton(this.ax, await this.snap(), nameSource, o?.withinText)); }
  async dispose() { await this.ax.dispose(); }
}
