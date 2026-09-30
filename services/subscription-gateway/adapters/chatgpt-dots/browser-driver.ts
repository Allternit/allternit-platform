// LIVE driver: Playwright over a persistent, headed Chrome profile that belongs to the user's ChatGPT account (the
// same profile model chatgpt-web's WorkerPool uses; see src/worker/pool.ts createPlaywrightLauncher). NOT exercised by
// any offline test: nothing here has run against chatgpt.com. It only ever:
//   - launches after explicit consent (userConsented), never on its own;
//   - navigates within the manifest's origins (navlock-style allowlist);
//   - reads page HTML and clicks/types like a person; it never reads cookies, storage or the profile's files.
import type { BrowserContext, Page } from "playwright";
import { isProfileLockError } from "../chatgpt-web/adapter.js";
import { DriverError, type ClickOptions, type DotsDriver } from "./driver.js";
import { CHATGPT_DOTS_MANIFEST } from "./manifest.js";
import { DOTS_LIST_URL, DOT_URL, NAMES, SELECTORS } from "./selectors.js";

export interface PageHandle { page: Page; close(): Promise<void> }
export type OpenPage = () => Promise<PageHandle>;

export interface BrowserDotsDriverOptions {
  /** Absolute Chrome user-data dir of the account's profile (never read by us; only handed to Chrome). */
  profileDir?: string;
  /** Explicit user consent to open the browser window. Without it connect() throws consent_required. */
  userConsented?: boolean;
  /** Test/wiring seam: supply an already-open page instead of launching Chrome. */
  openPage?: OpenPage;
}

const ORIGINS = new Set(CHATGPT_DOTS_MANIFEST.origins.map((o) => new URL(o).origin));
const nav = (url: string) => { if (!ORIGINS.has(new URL(url).origin)) throw new DriverError("unreachable", `navigation outside the ChatGPT origins refused: ${new URL(url).origin}`); return url; };

async function launchChrome(profileDir: string): Promise<PageHandle> {
  const { chromium } = await import("playwright");
  let context: BrowserContext;
  try {
    context = await chromium.launchPersistentContext(profileDir, { channel: "chrome", headless: false, args: ["--hide-crash-restore-bubble"] });
  } catch (e) {
    if (isProfileLockError(e)) throw new DriverError("already_running", "That ChatGPT browser profile is already open (another Allternit session or Chrome). Close it, then try again.");
    throw new DriverError("unreachable", `Could not open Chrome: ${(e as Error).message}`);
  }
  const page = context.pages()[0] ?? (await context.newPage());
  return { page, close: () => context.close() };
}

export class BrowserDotsDriver implements DotsDriver {
  private handle: PageHandle | undefined;
  constructor(private opts: BrowserDotsDriverOptions = {}) {}

  async connect() {
    if (this.handle) return;
    if (this.opts.openPage) { this.handle = await this.opts.openPage(); return; }
    if (!this.opts.userConsented) throw new DriverError("consent_required", "Allternit needs your OK before it opens the ChatGPT browser window for your dots.");
    if (!this.opts.profileDir) throw new DriverError("not_running", "No ChatGPT browser profile is configured for dots.");
    this.handle = await launchChrome(this.opts.profileDir);
    await this.handle.page.goto(nav(DOTS_LIST_URL), { waitUntil: "domcontentloaded" });
  }
  private get page(): Page { if (!this.handle) throw new DriverError("not_running", "ChatGPT browser session is not open"); return this.handle.page; }
  async isAppRunning() { return !!this.handle && !this.handle.page.isClosed(); }
  async html() { try { return await this.page.content(); } catch (e) { if (e instanceof DriverError) throw e; throw new DriverError("unreachable", (e as Error).message); } }

  async showDotList() { await this.page.goto(nav(DOTS_LIST_URL), { waitUntil: "domcontentloaded" }); return true; }
  async openDot(id: string) {
    const row = this.page.locator(`a[href$="/dots/${id}"], a[href$="/dots/${encodeURIComponent(id)}"]`).first();
    if (await row.count()) await row.click({ timeout: 8000 });
    else await this.page.goto(nav(DOT_URL(id)), { waitUntil: "domcontentloaded" });
    return true;
  }
  async showTasks() {
    if (await this.page.locator(SELECTORS.tasksPanel.css.join(", ")).count()) return true;
    const b = this.page.getByRole("button", { name: new RegExp(NAMES.tasks, "i") }).first();
    if (!(await b.count())) return false;
    await b.click({ timeout: 8000 });
    return true;
  }
  async typeText(text: string) {
    const composer = this.page.locator(SELECTORS.composer.css.join(", ")).first();
    if (!(await composer.count())) return false;
    await composer.click({ timeout: 8000 });
    await this.page.keyboard.press("ControlOrMeta+A");
    await this.page.keyboard.insertText(text);
    return true;
  }
  async clickButton(nameSource: string, o?: ClickOptions) {
    let scope = this.page.locator("body");
    if (o?.withinText) scope = this.page.locator("[role=group]").filter({ hasText: new RegExp(o.withinText, "i") }).first();
    const b = scope.getByRole("button", { name: new RegExp(nameSource, "i") }).first();
    if (!(await b.count())) return false;
    await b.click({ timeout: 8000 });
    return true;
  }
  async dispose() { const h = this.handle; this.handle = undefined; await h?.close().catch(() => {}); }
}
