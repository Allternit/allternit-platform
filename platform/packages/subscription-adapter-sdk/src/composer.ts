// §A3.1 — fillComposer / submit shared primitives.
import type { Locator, Page } from "playwright";
import type { SdkSelectorResolver } from "./selectors";

const LONG_TEXT_THRESHOLD = 500;

// Handles contenteditable and textarea composers; long prompts go through
// insertText (never per-key typing).
/** The prompt didn't land in the visible composer (markup drift). Nothing was sent. */
export class ComposerNotFilledError extends Error {
  constructor(readonly detail: string) {
    super(`composer did not take the prompt (${detail})`);
    this.name = "ComposerNotFilledError";
  }
}

const norm = (s: string) => s.replace(/\s+/g, " ").trim();

// Providers keep hidden fallback fields next to the real editor; typing into
// one of those leaves the visible composer empty (live: ChatGPT, 2026-09-29).
async function firstVisible(locator: Locator): Promise<Locator | null> {
  const n = Math.min(await locator.count(), 8);
  for (let i = 0; i < n; i++) {
    const candidate = locator.nth(i);
    if (await candidate.isVisible().catch(() => false)) return candidate;
  }
  return null;
}

/** What the composer shows now (value for fields, text for editors). */
export async function composerText(target: Locator): Promise<string> {
  return target.evaluate((el) =>
    el.tagName === "TEXTAREA" || el.tagName === "INPUT"
      ? (el as HTMLTextAreaElement).value
      : (el as HTMLElement).innerText
  );
}

// How long the composer must hold the prompt before it counts: some providers
// swap a pre-hydration <textarea> for their real editor right after the first
// input, dropping what was typed (live: ChatGPT, 2026-09-29).
const SETTLE_MS = 400;
const FILL_ATTEMPTS = 3;

export async function fillComposer(
  page: Page,
  resolver: SdkSelectorResolver,
  text: string,
  opts: { key?: string } = {}
): Promise<Locator> {
  const expected = norm(text).slice(0, 40);
  const seen: string[] = [];
  let lastDetail = "";
  for (let attempt = 0; attempt < FILL_ATTEMPTS; attempt++) {
    const locator = await resolver.resolveLocator(opts.key ?? "composer");
    const target = await firstVisible(locator);
    // Typing into a hidden match would hang until Playwright's timeout; fail
    // fast instead, before anything is sent.
    if (!target) throw new ComposerNotFilledError(`no visible match among ${await locator.count()}`);
    const handle = await target.elementHandle();
    const { kind, tag } = await target.evaluate((el) => {
      const tag = el.tagName.toLowerCase();
      if (tag === "textarea" || tag === "input") return { kind: "field", tag };
      if ((el as HTMLElement).isContentEditable) return { kind: "contenteditable", tag };
      return { kind: "unknown", tag };
    });
    seen.push(`<${tag}>`);

    if (kind === "field") {
      await target.fill(text);
    } else {
      await target.click();
      // A retry may land on an editor still holding part of the prompt.
      if (attempt > 0) {
        await page.keyboard.press("ControlOrMeta+A");
        await page.keyboard.press("Backspace");
      }
      if (text.length > LONG_TEXT_THRESHOLD) {
        await page.keyboard.insertText(text);
      } else {
        await target.pressSequentially(text);
      }
    }
    // Verify before anything is sent: after a settle, the same element is
    // still the visible composer and holds the prompt.
    await page.waitForTimeout(SETTLE_MS);
    const same = handle
      ? await target.evaluate((el, h) => el === h && el.isConnected, handle).catch(() => false)
      : false;
    const shown = norm(await composerText(target).catch(() => ""));
    if (same && (!expected || shown.includes(expected))) return target;
    const visible = await target.isVisible().catch(() => false);
    lastDetail = same
      ? `${kind} <${tag}> visible=${visible}, shows ${shown.length} chars`
      : `the composer was replaced after typing (${kind} <${tag}>)`;
  }
  throw new ComposerNotFilledError(`${lastDetail}; tried ${seen.join(" → ")}`);
}

export interface SubmitOptions {
  composerKey?: string;
  sendKey?: string;
  // Pack hint: allow pressing Enter in the composer when the send button is
  // missing or disabled.
  fallback?: "enter";
}

export async function submit(
  page: Page,
  resolver: SdkSelectorResolver,
  opts: SubmitOptions = {}
): Promise<void> {
  const sendLoc = await resolver.tryResolveLocator(opts.sendKey ?? "send_button");
  if (sendLoc && (await sendLoc.first().isEnabled())) {
    await sendLoc.first().click();
    return;
  }
  if (opts.fallback === "enter") {
    const composer = await resolver.resolveLocator(opts.composerKey ?? "composer");
    await composer.first().click();
    await page.keyboard.press("Enter");
    return;
  }
  throw new Error("submit: send button unavailable and no fallback configured");
}

export interface AttachFile {
  name: string;
  mimeType: string;
  buffer: Buffer;
}

// Attach files through the composer's (usually hidden) file input — no
// native file chooser. The pack key points at the input to use (e.g. the
// image-only one). Throws SelectorNotFoundError when it is gone (UI drift).
export async function attachFiles(
  resolver: SdkSelectorResolver,
  files: AttachFile[],
  opts: { key?: string } = {}
): Promise<void> {
  if (files.length === 0) return;
  const input = await resolver.resolveLocator(opts.key ?? "file_input");
  await input.first().setInputFiles(files);
}

export interface SendReadyOptions {
  sendKey?: string;
  timeoutMs?: number;
  pollIntervalMs?: number;
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
}

// Wait until the send button exists and is enabled — providers disable it
// while an attachment uploads. Returns false on timeout.
export async function waitForSendReady(
  resolver: SdkSelectorResolver,
  opts: SendReadyOptions = {}
): Promise<boolean> {
  const now = opts.now ?? (() => Date.now());
  const sleep = opts.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  const deadline = now() + (opts.timeoutMs ?? 60000);
  for (;;) {
    const send = await resolver.tryResolveLocator(opts.sendKey ?? "send_button");
    if (send && (await send.first().isEnabled())) return true;
    if (now() >= deadline) return false;
    await sleep(opts.pollIntervalMs ?? 250);
  }
}
