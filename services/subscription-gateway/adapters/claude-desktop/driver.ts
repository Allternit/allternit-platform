// Transport seam between the AAI provider and Claude's renderer. Two implementations:
// CdpClaudeDesktopDriver (live, cdp-driver.ts) and ReplayClaudeDesktopDriver (offline fixtures, replay-driver.ts).
export type DriverFault = "unreachable" | "not_running" | "consent_required" | "already_running";
export class DriverError extends Error {
  constructor(readonly fault: DriverFault, message: string) { super(message); this.name = "DriverError"; }
}
export interface ClickOptions {
  /** Only click a button whose ancestor's visible text matches this regex source (scopes to one approval card). */
  withinText?: string;
}
export interface ClaudeDesktopDriver {
  /** Attach (idempotent). Throws DriverError. */
  connect(): Promise<void>;
  /** Claude process is alive (never inspects its user-data). */
  isAppRunning(): Promise<boolean>;
  /** Renderer document outerHTML. */
  html(): Promise<string>;
  /** Start a fresh conversation (the "New chat" control). */
  newChat(): Promise<boolean>;
  /** Insert text into the composer (focus + insertText; no send). */
  typeText(text: string): Promise<boolean>;
  /** Click the button whose accessible name matches the regex source. Returns false when not present. */
  clickButton(nameSource: string, opts?: ClickOptions): Promise<boolean>;
  dispose(): Promise<void>;
}
