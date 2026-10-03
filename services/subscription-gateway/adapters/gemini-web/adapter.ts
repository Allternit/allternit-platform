// gemini-web adapter — the shared web-chat base (fresh chat, mapped-thread
// continue with the divergence check, fingerprint reconcile) over
// gemini.google.com. chat.create / chat.continue only; model_class is not
// applied yet (the account's default model answers). readAccount is DOM-only:
// Gemini exposes no known non-spending same-origin account/usage RPC, so the
// signed-in identity comes from the header avatar's accessible name and usage
// stays null (hard limits still surface through the banner system during
// tasks). Selectors are v1-unverified (see selectors/v1.yaml).
import type { AccountObservation } from "@allternit/subscription-fabric-contracts";
import type { DeclarativeChatConfig } from "@allternit/subscription-adapter-sdk";
import { readFileSync } from "node:fs";
import { WebChatAdapter, loadManifestAt, type WebChatOptions } from "../_shared/web-chat.js";

// Conversation URLs are /app/<16-hex> (the Bard-era /chat/<id> became
// /app/<id>); v1-unverified.
export const THREAD_URL_PATTERN = /^https:\/\/gemini\.google\.com\/app\/([0-9a-f]{16})/;

export function geminiWebConfig(overrides: Partial<DeclarativeChatConfig> = {}): DeclarativeChatConfig {
  return {
    manifest: loadManifestAt(new URL("./manifest.yaml", import.meta.url)),
    selectorsYaml: readFileSync(new URL("./selectors/v1.yaml", import.meta.url), "utf8"),
    threadUrlPattern: THREAD_URL_PATTERN,
    banners: [
      { kind: "limit_banner", pattern: /(reached|hit|exceeded) (your |the )?(gemini |usage |daily )?limit/i, blocksSend: true },
      { kind: "limit_banner", pattern: /(not enough|insufficient|out of) (credits|quota)/i, blocksSend: true },
      { kind: "reset_notice", pattern: /(limit|quota|credits?) (will )?(reset|refresh)(es)? (at|in|on)/i },
    ],
    // send_button is not probed: it may render only once the composer has
    // text. Submit resolves it after fillComposer.
    criticalKeys: ["composer", "logged_in_probe"],
    sampleThreadUrl: "https://gemini.google.com/app/7c4a9e2b1d5f38a6",
    sampleThreadId: "7c4a9e2b1d5f38a6",
    submitFallbackEnter: true,
    ...overrides,
    // After a reply the composer is empty, so Send is disabled or swapped:
    // completion rests on stop/streaming absence + stable text. Merged so a
    // timing override keeps it.
    completion: { ignoreSend: true, ...overrides.completion },
  };
}

export class GeminiWebAdapter extends WebChatAdapter {
  constructor(opts: WebChatOptions = {}, configOverrides: Partial<DeclarativeChatConfig> = {}) {
    super(
      {
        newChatUrl: "https://gemini.google.com/app",
        threadUrl: (id) => `https://gemini.google.com/app/${id}`,
      },
      geminiWebConfig(configOverrides),
      opts
    );
  }

  // Who is signed in. Non-spending: the header avatar button's accessible
  // name ("Google Account: Name (email)"), parsed in the page; only the name
  // and email leave the page, never a cookie or token. Usage is null — Gemini
  // exposes no known non-spending usage counter; the task-time banner system
  // still catches hard limits before typing.
  async readAccount(_signal: AbortSignal): Promise<AccountObservation> {
    const page = this.attachedPage();
    if (!page) throw new Error("readAccount called before attach()");
    // tsx wraps the named helpers below in __name(...), absent in the page
    // (see the SDK's extract.ts).
    await page.evaluate("globalThis.__name ??= (fn) => fn");
    const read = await page.evaluate(() => {
      const btn =
        document.querySelector("button[aria-label*='Google Account']") ??
        document.querySelector("button.account-avatar");
      const label = btn?.getAttribute("aria-label") ?? btn?.textContent ?? "";
      // "Google Account: Eoj (eoj@example.com)" → prefer the parenthesized
      // email, else whatever name the label carries after the colon.
      const m = /Google Account:\s*([^(]+?)(?:\s*\(([^)]+)\))?$/i.exec(label.trim());
      const identity = (m?.[2] ?? m?.[1] ?? "").trim() || null;
      return { identity };
    });
    return { identity: read.identity, usage: null };
  }
}

// Adapter-package convention: the gateway worker pool instantiates adapters
// through a zero-arg `createAdapter()` found at <adapter dir>/adapter.ts.
export function createAdapter(): GeminiWebAdapter {
  return new GeminiWebAdapter();
}

export default createAdapter;
