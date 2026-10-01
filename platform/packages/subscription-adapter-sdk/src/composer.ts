// §A3.1 — fillComposer / submit shared primitives.
import type { ElementHandle, Locator, Page } from "playwright";
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

/**
 * Whether this exact element is still in the page and shows the prompt. A
 * replaced element is neither "still holding" nor "emptied by a send".
 */
export async function composerState(
  element: ElementHandle<Node>,
  text: string
): Promise<"holds" | "cleared" | "replaced"> {
  const expected = norm(text).slice(0, 40);
  const got = await element
    .evaluate((node) => {
      const el = node as HTMLElement;
      if (!el.isConnected) return null;
      return el.tagName === "TEXTAREA" || el.tagName === "INPUT"
        ? (el as HTMLTextAreaElement).value
        : el.innerText;
    })
    .catch(() => null);
  if (got === null) return "replaced";
  return norm(got).includes(expected) ? "holds" : "cleared";
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
    // Don't type into an element the page is about to replace.
    await page.waitForTimeout(SETTLE_MS);
    const stable = handle
      ? await target.evaluate((el, h) => el === h && el.isConnected, handle).catch(() => false)
      : false;
    if (!stable) {
      seen.push("(replaced before typing)");
      lastDetail = "the composer was replaced before typing";
      continue;
    }
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
      // Always start from an empty editor: a retry may hold part of the prompt,
      // and a provider keeps an unsent draft across visits (live 2026-09-30:
      // claude.ai kept an earlier draft and the prompt was appended to it).
      await page.keyboard.press("ControlOrMeta+A");
      await page.keyboard.press("Backspace");
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
    // The prompt must be the composer's content, not appended to leftover text.
    if (same && (!expected || shown.startsWith(expected))) return target;
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
): Promise<"click" | "enter"> {
  const sendLoc = await resolver.tryResolveLocator(opts.sendKey ?? "send_button");
  if (sendLoc && (await sendLoc.first().isEnabled())) {
    await sendLoc.first().click();
    return "click";
  }
  if (opts.fallback === "enter") {
    const composer = await resolver.resolveLocator(opts.composerKey ?? "composer");
    await composer.first().click();
    await page.keyboard.press("Enter");
    return "enter";
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
