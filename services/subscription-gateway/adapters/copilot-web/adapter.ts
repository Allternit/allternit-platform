import { readAccountBots, accountBotUrl } from "../_shared/account-bots.js";
// copilot-web adapter — the shared web-chat base (fresh chat, mapped-thread
// continue with the divergence check, fingerprint reconcile) over
// copilot.microsoft.com. chat.create / chat.continue only; model_class is not
// applied yet (the account's default mode answers). readAccount is DOM-only:
// consumer Copilot exposes no known non-spending same-origin account/usage
// RPC, so the signed-in identity comes from the header account picture and
// usage stays null (hard limits still surface through the banner system
// during tasks). Selectors are v1-unverified (see selectors/v1.yaml).
//
// UNVERIFIED: consumer Copilot is believed to keep past conversations behind
// the sidebar chat list rather than per-chat URLs; the /chats/<id> shape below
// is INFERRED. If live probing confirms no per-chat URL, chat.continue must
// drive the sidebar instead and this pattern + threadUrl need rework.
import type { AccountObservation } from "@allternit/subscription-fabric-contracts";
import type { DeclarativeChatConfig } from "@allternit/subscription-adapter-sdk";
import { readFileSync } from "node:fs";
import { WebChatAdapter, loadManifestAt, type WebChatOptions } from "../_shared/web-chat.js";

export const THREAD_URL_PATTERN = /^https:\/\/copilot\.microsoft\.com\/chats\/([\w-]+)/;

export function copilotWebConfig(overrides: Partial<DeclarativeChatConfig> = {}): DeclarativeChatConfig {
  return {
    manifest: loadManifestAt(new URL("./manifest.yaml", import.meta.url)),
    selectorsYaml: readFileSync(new URL("./selectors/v1.yaml", import.meta.url), "utf8"),
    threadUrlPattern: THREAD_URL_PATTERN,
    banners: [
      { kind: "limit_banner", pattern: /(reached|hit) (your |the )?(daily |usage |message )?limit/i, blocksSend: true },
      { kind: "limit_banner", pattern: /(too many requests|unusually (high|busy) (traffic|activity))/i, blocksSend: true },
      { kind: "reset_notice", pattern: /(limit|quota) (will )?(reset|refresh)(es)? (at|in|on)/i },
    ],
    // send_button is not probed: it may render only once the composer has
    // text. Submit resolves it after fillComposer.
    criticalKeys: ["composer", "logged_in_probe"],
    sampleThreadUrl: "https://copilot.microsoft.com/chats/a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    sampleThreadId: "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    submitFallbackEnter: true,
    ...overrides,
    // After a reply the composer is empty, so Send is disabled or swapped:
    // completion rests on stop/streaming absence + stable text. Merged so a
    // timing override keeps it.
    completion: { ignoreSend: true, ...overrides.completion },
  };
}

export class CopilotWebAdapter extends WebChatAdapter {
  constructor(opts: WebChatOptions = {}, configOverrides: Partial<DeclarativeChatConfig> = {}) {
    super(
      {
        newChatUrl: "https://copilot.microsoft.com/",
        newChatUrlFor: (task) => accountBotUrl("microsoft", task.options, "https://copilot.microsoft.com"),
        threadUrl: (id) => `https://copilot.microsoft.com/chats/${id}`,
      },
      copilotWebConfig(configOverrides),
      opts
    );
  }

  // Who is signed in. Non-spending: the header account picture's alt/label,
  // parsed in the page; only the name leaves the page, never a cookie or
  // token. Usage is null — Copilot exposes no known non-spending usage
  // counter; the task-time banner system still catches hard limits before
  // typing.
  async readAccount(_signal: AbortSignal): Promise<AccountObservation> {
    const page = this.attachedPage();
    if (!page) throw new Error("readAccount called before attach()");
    // tsx wraps the named helpers below in __name(...), absent in the page
    // (see the SDK's extract.ts).
    await page.evaluate("globalThis.__name ??= (fn) => fn");
    const read = await page.evaluate(() => {
      const pic =
        document.querySelector("img#mectrl_headerPicture") ??
        document.querySelector("button[aria-label*='Account'] img") ??
        document.querySelector("header .header-picture");
      const label = pic?.getAttribute("aria-label") ?? "";
      // "Account manager for Eoj" → "Eoj"; fall back to the plain alt text.
      const m = /for\s+(.+)$/i.exec(label.trim());
      const identity = (m?.[1] ?? pic?.getAttribute("alt") ?? "").trim() || null;
      return { identity };
    });
    const agents = await readAccountBots(page, "microsoft");
    return { identity: read.identity, usage: null, agents };
  }
}

// Adapter-package convention: the gateway worker pool instantiates adapters
// through a zero-arg `createAdapter()` found at <adapter dir>/adapter.ts.
export function createAdapter(): CopilotWebAdapter {
  return new CopilotWebAdapter();
}

export default createAdapter;
