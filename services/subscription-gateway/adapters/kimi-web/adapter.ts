// kimi-web adapter — the shared web-chat base (fresh chat, mapped-thread
// continue with the divergence check, fingerprint reconcile) over
// www.kimi.ai (where the session lives; www.kimi.com has separate storage). chat.create / chat.continue only; model_class is not applied
// yet (the account's default model answers), and identity/usage are not read
// yet (no known same-origin account endpoint — added after the live probe).
// Selectors are v1-unverified (see selectors/v1.yaml).
import type { DeclarativeChatConfig } from "@allternit/subscription-adapter-sdk";
import { readFileSync } from "node:fs";
import { WebChatAdapter, loadManifestAt, type WebChatOptions } from "../_shared/web-chat.js";

export const THREAD_URL_PATTERN = /^https:\/\/(?:www\.)?kimi\.(?:com|ai)\/chat\/([\w-]+)/;

export function kimiWebConfig(overrides: Partial<DeclarativeChatConfig> = {}): DeclarativeChatConfig {
  return {
    manifest: loadManifestAt(new URL("./manifest.yaml", import.meta.url)),
    selectorsYaml: readFileSync(new URL("./selectors/v1.yaml", import.meta.url), "utf8"),
    threadUrlPattern: THREAD_URL_PATTERN,
    banners: [
      { kind: "limit_banner", pattern: /(reached|hit) (the |your )?(usage |daily |message |chat )?limit/i },
      { kind: "limit_banner", pattern: /(not enough|insufficient|out of) credits/i },
      { kind: "reset_notice", pattern: /(limit|quota|credits?) (will )?(reset|refresh)(es)? (at|in|on)/i },
    ],
    // send_button is not probed: it may render only once the composer has
    // text. Submit resolves it after fillComposer.
    criticalKeys: ["composer", "logged_in_probe"],
    sampleThreadUrl: "https://www.kimi.ai/chat/d3k5a1b2c3d4e5f6g7h8",
    sampleThreadId: "d3k5a1b2c3d4e5f6g7h8",
    submitFallbackEnter: true,
    ...overrides,
    // After a reply the composer is empty, so Send is disabled or swapped:
    // completion rests on stop/streaming absence + stable text. Merged so a
    // timing override keeps it.
    completion: { ignoreSend: true, ...overrides.completion },
  };
}

export class KimiWebAdapter extends WebChatAdapter {
  constructor(opts: WebChatOptions = {}, configOverrides: Partial<DeclarativeChatConfig> = {}) {
    super(
      {
        newChatUrl: "https://www.kimi.ai/",
        threadUrl: (id) => `https://www.kimi.ai/chat/${id}`,
      },
      kimiWebConfig(configOverrides),
      opts
    );
  }
}

// Adapter-package convention: the gateway worker pool instantiates adapters
// through a zero-arg `createAdapter()` found at <adapter dir>/adapter.ts.
export function createAdapter(): KimiWebAdapter {
  return new KimiWebAdapter();
}

export default createAdapter;
