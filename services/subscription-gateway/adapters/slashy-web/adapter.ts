// slashy-web adapter — Slashy through its web app (route 2, browser sign-in),
// on the shared WebChatAdapter base. v1-UNCONFIRMED: the product identity,
// origin and selectors are guesses (see manifest.yaml). readAccount is a
// non-spending stub: identity and usage stay null until the real account page
// is known, so nothing is read from the page.
import { readFileSync } from "node:fs";
import type { DeclarativeChatConfig } from "@allternit/subscription-adapter-sdk";
import type { AccountObservation, ExecutionContext } from "@allternit/subscription-fabric-contracts";
import { WebChatAdapter, loadManifestAt, sdkPage, type WebChatOptions } from "../_shared/web-chat.js";

export const THREAD_URL_PATTERN = /^https:\/\/slashy\.com\/(?:chat|t)\/([\w-]+)/;

export function slashyWebConfig(overrides: Partial<DeclarativeChatConfig> = {}): DeclarativeChatConfig {
  return {
    manifest: loadManifestAt(new URL("./manifest.yaml", import.meta.url)),
    selectorsYaml: readFileSync(new URL("./selectors/v1.yaml", import.meta.url), "utf8"),
    threadUrlPattern: THREAD_URL_PATTERN,
    banners: [
      { kind: "limit_banner", pattern: /(reached|hit|exceeded) (your |the )?(usage |daily )?limit/i, blocksSend: true },
      { kind: "reset_notice", pattern: /(limit|quota|credits?) (will )?(reset|refresh)(es)? (at|in|on)/i },
    ],
    criticalKeys: ["composer", "logged_in_probe"],
    submitFallbackEnter: true,
    completion: { ignoreSend: true, ...overrides.completion },
    ...overrides,
  };
}

export class SlashyWebAdapter extends WebChatAdapter {
  constructor(opts: WebChatOptions = {}, configOverrides: Partial<DeclarativeChatConfig> = {}) {
    super(
      {
        newChatUrl: "https://www.slashy.com/",
        threadUrl: (id) => `https://www.slashy.com/chat/${id}`,
      },
      slashyWebConfig(configOverrides),
      opts
    );
  }

  // Non-spending stub until the real account page is confirmed (v1-unverified).
  async readAccount(_signal: AbortSignal): Promise<AccountObservation> {
    const page = this.attachedPage();
    if (!page) throw new Error("readAccount called before attach()");
    return { identity: null, usage: null, agents: [] } as AccountObservation;
  }
}

export function createAdapter(): SlashyWebAdapter {
  return new SlashyWebAdapter();
}

export default createAdapter;
