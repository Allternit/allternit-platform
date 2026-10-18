// abacusai-bot-web adapter — the shared web-chat base (fresh chat, mapped-thread
// continue with the divergence check, fingerprint reconcile) over
// apps.abacus.ai, the Abacus.AI web app the user's own bot runs behind. Signed
// in through Allternit's login browser with the user's own Abacus.AI account.
// chat.create / chat.continue only. Selectors are v1-unverified (see selectors/v1.yaml).
import type { AccountObservation } from "@allternit/subscription-fabric-contracts";
import type { DeclarativeChatConfig } from "@allternit/subscription-adapter-sdk";
import { readFileSync } from "node:fs";
import { WebChatAdapter, loadManifestAt, type WebChatOptions } from "../_shared/web-chat.js";
import { readAccountBots } from "../_shared/account-bots.js";
import { adapterFile } from "../_shared/adapter-file.js";

export const THREAD_URL_PATTERN = /^https:\/\/apps\.abacus\.ai\/bot\/(?:chat\/)?([\w-]+)/;

export function abacusaiBotWebConfig(overrides: Partial<DeclarativeChatConfig> = {}): DeclarativeChatConfig {
  return {
    manifest: loadManifestAt(adapterFile(import.meta.url, "abacusai-bot-web", "./manifest.yaml")),
    selectorsYaml: readFileSync(adapterFile(import.meta.url, "abacusai-bot-web", "./selectors/v1.yaml"), "utf8"),
    threadUrlPattern: THREAD_URL_PATTERN,
    banners: [
      { kind: "limit_banner", pattern: /(reached|hit) (the |your )?(usage |daily |message |chat )?limit/i, blocksSend: true },
      { kind: "limit_banner", pattern: /(not enough|insufficient|out of) credits/i, blocksSend: true },
      { kind: "reset_notice", pattern: /(limit|quota|credits?) (will )?(reset|refresh)(es)? (at|in|on)/i },
    ],
    // send_button may render only once the composer has text; submit resolves it after fillComposer.
    criticalKeys: ["composer", "logged_in_probe"],
    sampleThreadUrl: "https://apps.abacus.ai/bot/chat/d3k5a1b2c3d4e5f6g7h8",
    sampleThreadId: "d3k5a1b2c3d4e5f6g7h8",
    submitFallbackEnter: true,
    ...overrides,
    completion: { ignoreSend: true, ...overrides.completion },
  };
}

export class AbacusaiBotWebAdapter extends WebChatAdapter {
  constructor(opts: WebChatOptions = {}, configOverrides: Partial<DeclarativeChatConfig> = {}) {
    super(
      {
        newChatUrl: "https://apps.abacus.ai/bot/",
        threadUrl: (id) => `https://apps.abacus.ai/bot/chat/${id}`,
      },
      abacusaiBotWebConfig(configOverrides),
      opts
    );
  }

  // Who is signed in, from the page itself. Non-spending: no token leaves the page.
  // Unverified: the bot list relies on the account-nav rules in _shared/account-bots
  // and may come back empty until those are written for Abacus.AI.
  async readAccount(_signal: AbortSignal): Promise<AccountObservation> {
    const page = this.attachedPage();
    if (!page) throw new Error("readAccount called before attach()");
    const agents = await readAccountBots(page, "abacusai");
    return { agents, identity: null, usage: null };
  }
}

// Adapter-package convention: the gateway worker pool instantiates adapters
// through a zero-arg `createAdapter()` found at <adapter dir>/adapter.ts.
export function createAdapter(): AbacusaiBotWebAdapter {
  return new AbacusaiBotWebAdapter();
}

export default createAdapter;
