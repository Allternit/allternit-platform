// Transport seam between the AAI provider and Grok Bot's renderer. Two implementations:
// CdpGrokDriver (live, cdp-driver.ts) and ReplayGrokDriver (offline fixtures, replay-driver.ts).
export type DriverFault = "unreachable" | "not_running" | "consent_required" | "already_running";
export class DriverError extends Error {
  constructor(readonly fault: DriverFault, message: string) { super(message); this.name = "DriverError"; }
}
export interface ClickOptions {
  /** Only click a button whose ancestor's visible text matches this regex source (scopes to one approval card). */
  withinText?: string;
}
export interface GrokDriver {
  /** Attach (idempotent). Throws DriverError. */
  connect(): Promise<void>;
  /** Grok Bot process is alive (never inspects its user-data). */
  isAppRunning(): Promise<boolean>;
  /** Renderer document outerHTML. */
  html(): Promise<string>;
  /** Start a fresh conversation (the "New chat" control). */
  newChat(): Promise<boolean>;
  /** Insert text into the composer (focus + insertText; no send). */
  typeText(text: string): Promise<boolean>;
  /** Click the button whose accessible name matches the regex source. Returns false when not present. */
  clickButton(nameSource: string, opts?: ClickOptions): Promise<boolean>;
  /**
   * Each sidebar Bot's mascot as the app draws it, rasterized to a PNG data URI in the page
   * (`name` is the row's first text, the Bot's name; `text` its whole visible text). Optional: read-only.
   */
  avatars?(): Promise<{ name?: string; text: string; png: string }[]>;
  dispose(): Promise<void>;
}
