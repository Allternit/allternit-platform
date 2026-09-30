// ChatGPT.app over the macOS Accessibility tree, as a DotsDriver ("chatgpt-app" transport for the chatgpt-dots adapter).
// Same trick as claude-desktop/ax: AX tree -> the DOM adapter's Scenario -> renderPage -> classify, so the provider is unchanged.
import { AxError, assertPackCompatible, axFault, ensureAttached, findButtons, missingCritical, pressButton, resolveKey, resolveKeys, textUnder, walk, writeComposer, type AxDriver, type AxNode, type AxSnapshot } from "../../_shared/ax/index.js";
import { DriverError, type ClickOptions, type DotsDriver } from "../driver.js";
import { NAMES } from "../selectors.js";
import { renderPage, type Scenario } from "../fixtures/markup.js";
import { CHATGPT_APP_AX_PACK, CHATGPT_APP_BUNDLE_ID } from "./selectors.js";

export const dotSlug = (s: string) => s.replace(/^@/, "").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
const SECTION: Record<string, "in_progress" | "scheduled" | "completed"> = { "in progress": "in_progress", scheduled: "scheduled", completed: "completed" };
const texts = (n: AxNode) => [...walk(n)].map((x) => x.node).filter((x) => x.role === "AXStaticText" || x.role === "AXHeading").map((x) => (x.value ?? x.title ?? x.description ?? "").trim()).filter(Boolean);

export class AxChatGptAppDriver implements DotsDriver {
  private st = { attached: false };
  lastDrift: { missing: string[]; packVersion: string; reason: string } | undefined;
  constructor(private ax: AxDriver, private consented = true, private pack = CHATGPT_APP_AX_PACK) {}

  private async guard<T>(f: () => Promise<T>): Promise<T> {
    if (!this.consented) throw new DriverError("consent_required", "Driving the ChatGPT app through Accessibility needs your explicit OK first. Enable it for this account, then try again.");
    try { return await f(); } catch (e) {
      const m = axFault(e); if (m) throw new DriverError(m.fault, m.message);
      throw e;
    }
  }
  async connect() { await this.guard(() => ensureAttached(this.ax, CHATGPT_APP_BUNDLE_ID, this.st)); }
  async isAppRunning() {
    if (!this.consented) return false;
    try { await ensureAttached(this.ax, CHATGPT_APP_BUNDLE_ID, this.st); return true; }
    catch (e) { return !(e instanceof AxError && e.fault === "not_running"); }
  }
  private snap(): Promise<AxSnapshot> { return this.guard(async () => { await ensureAttached(this.ax, CHATGPT_APP_BUNDLE_ID, this.st); return this.ax.snapshot(); }); }

  private dots(root: AxNode) {
    return resolveKey(root, this.pack, "dotRow").map((row) => {
      const t = texts(row); const label = row.title || row.description || t[0] || "";
      const name = (t.find((x) => !x.startsWith("@")) ?? label).trim(); const handle = t.find((x) => x.startsWith("@"));
      return { row, id: dotSlug(handle ?? name), name, handle };
    }).filter((d) => d.id);
  }
  toScenario(root: AxNode): Scenario {
    try { assertPackCompatible(this.pack); } catch (e) {
      this.lastDrift = { missing: [], packVersion: this.pack.packVersion, reason: (e as Error).message };
      return { drift: "composer" };
    }
    const composer = resolveKey(root, this.pack, "composer")[0];
    const dots = this.dots(root);
    const banner = resolveKey(root, this.pack, "alert").map(textUnder).filter(Boolean).join(" | ") || undefined;
    if (!composer) {
      if (dots.length) { this.lastDrift = undefined; return { view: "list", dots: dots.map(({ id, name, handle }) => ({ id, name, handle })), banner }; }
      if (findButtons(root, NAMES.signIn).length) { this.lastDrift = undefined; return { loggedOut: true }; }
      this.lastDrift = { missing: missingCritical(root, this.pack), packVersion: this.pack.packVersion, reason: "neither composer nor dots list found by any AX selector" };
      return { drift: "composer" };
    }
    this.lastDrift = undefined;
    const turns = resolveKeys(root, this.pack, ["userTurn", "assistantTurn"]).map(({ key, node }) => ({ role: key === "userTurn" ? "user" as const : "assistant" as const, text: textUnder(node) }));
    const s: Scenario = { view: "dot", turns, streaming: findButtons(root, NAMES.stop).length > 0, composerText: composer.value ?? "", banner };
    const header = resolveKey(root, this.pack, "dotHeader")[0];
    if (header) { const t = texts(header); const name = t.find((x) => !x.startsWith("@")); if (name) s.dot = { id: dotSlug(t.find((x) => x.startsWith("@")) ?? name), name, handle: t.find((x) => x.startsWith("@")) }; }
    const conf = resolveKey(root, this.pack, "confirmation")[0];
    if (conf) s.confirmation = { policy: /^hand off/i.test(conf.description ?? conf.title ?? "") ? "hand_off" : "ask_first", text: textUnder(conf) };
    const panel = resolveKey(root, this.pack, "tasksPanel")[0];
    if (panel) {
      s.showTasks = true; s.tasks = [];
      let state: "in_progress" | "scheduled" | "completed" | undefined;
      for (const { node } of walk(panel)) {
        if (node.role !== "AXHeading" && node.role !== "AXStaticText") continue;
        const t = (node.value ?? node.title ?? "").trim(); if (!t) continue;
        if (node.role === "AXHeading" && SECTION[t.toLowerCase()]) state = SECTION[t.toLowerCase()]; else if (state) s.tasks.push({ title: t, state });
      }
    }
    const act = resolveKey(root, this.pack, "activity")[0];
    if (act) s.activity = textUnder(act);
    return s;
  }
  async html() { return renderPage(this.toScenario((await this.snap()).root)); }

  async showDotList() {
    return this.guard(async () => {
      const snap = await this.snap();
      if (await pressButton(this.ax, snap, NAMES.dotsList)) return true;
      return this.dots(snap.root).length > 0;
    });
  }
  async openDot(id: string) {
    return this.guard(async () => {
      let snap = await this.snap();
      let d = this.dots(snap.root).find((x) => x.id === id);
      if (!d && (await pressButton(this.ax, snap, NAMES.dotsList))) { snap = await this.snap(); d = this.dots(snap.root).find((x) => x.id === id); }
      return d ? this.ax.press(d.row.path) : false;
    });
  }
  async showTasks() { return this.guard(async () => { const snap = await this.snap(); return resolveKey(snap.root, this.pack, "tasksPanel").length > 0 || (await pressButton(this.ax, snap, NAMES.tasks)); }); }
  async typeText(text: string) {
    return this.guard(async () => {
      const find = async () => resolveKey((await this.snap()).root, this.pack, "composer")[0];
      return writeComposer(this.ax, await find(), text, find);
    });
  }
  async clickButton(nameSource: string, o?: ClickOptions) { return this.guard(async () => pressButton(this.ax, await this.snap(), nameSource, o?.withinText)); }
  async dispose() { await this.ax.dispose(); }
}
