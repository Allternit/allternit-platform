// Transport seam between the AAI provider and the ChatGPT dots UI. Two implementations:
// BrowserDotsDriver (live Playwright, browser-driver.ts) and ReplayDotsDriver (offline fixtures, replay-driver.ts).
export type DriverFault = "unreachable" | "not_running" | "consent_required" | "already_running" | "not_trusted";
export class DriverError extends Error {
  constructor(readonly fault: DriverFault, message: string) { super(message); this.name = "DriverError"; }
}
export interface ClickOptions {
  /** Only click a button inside a card whose visible text matches this regex source (scopes to one confirmation). */
  withinText?: string;
}
export interface DotsDriver {
  /** Attach (idempotent). Throws DriverError. */
  connect(): Promise<void>;
  /** Browser session is alive. */
  isAppRunning(): Promise<boolean>;
  /** Page document outerHTML. */
  html(): Promise<string>;
  /** Navigate to the user's dots list view. */
  showDotList(): Promise<boolean>;
  /** Open one dot's conversation by dot id (the /dots/<id> link target). false when no such dot. */
  openDot(id: string): Promise<boolean>;
  /** Reveal the open dot's tasks panel (In progress / Scheduled / Completed). */
  showTasks(): Promise<boolean>;
  /** Insert text into the composer (focus + insertText; no send). */
  typeText(text: string): Promise<boolean>;
  clickButton(nameSource: string, opts?: ClickOptions): Promise<boolean>;
  dispose(): Promise<void>;
}
