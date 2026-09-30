// claude-web adapter — the shared web-chat base (fresh chat, mapped-thread
// continue with the divergence check, fingerprint reconcile) over claude.ai.
// chat.create / chat.continue only; model_class is not applied yet (the
// account's default model answers). Selectors are v1-unverified (see
// selectors/v1.yaml).
import type { AccountObservation } from "@allternit/subscription-fabric-contracts";
import type { DeclarativeChatConfig } from "@allternit/subscription-adapter-sdk";
import { readFileSync } from "node:fs";
import { WebChatAdapter, loadManifestAt, type WebChatOptions } from "../_shared/web-chat.js";

export const THREAD_URL_PATTERN = /^https:\/\/claude\.ai\/chat\/([0-9a-f-]{36})/;

export function claudeWebConfig(overrides: Partial<DeclarativeChatConfig> = {}): DeclarativeChatConfig {
  return {
    manifest: loadManifestAt(new URL("./manifest.yaml", import.meta.url)),
    selectorsYaml: readFileSync(new URL("./selectors/v1.yaml", import.meta.url), "utf8"),
    threadUrlPattern: THREAD_URL_PATTERN,
    banners: [
      { kind: "limit_banner", pattern: /out of (free )?messages/i, blocksSend: true },
      { kind: "limit_banner", pattern: /(you'?ve )?(hit|reached) (your|the) (usage |message )?limit/i, blocksSend: true },
      { kind: "limit_banner", pattern: /approaching (your |the )?(usage |message )?limit/i },
      { kind: "reset_notice", pattern: /(limit|usage|messages?) (will )?resets? (at|in|on)/i },
    ],
    // send_button is not probed: like ChatGPT, it may render only once the
    // composer has text. Submit resolves it after fillComposer.
    criticalKeys: ["composer", "logged_in_probe"],
    sampleThreadUrl: "https://claude.ai/chat/0f6f1c2e-6a3b-4c1d-9e8f-123456789abc",
    sampleThreadId: "0f6f1c2e-6a3b-4c1d-9e8f-123456789abc",
    submitFallbackEnter: true,
    ...overrides,
    // After a reply the composer is empty, so Send is disabled or gone:
    // completion rests on stop/streaming absence + stable text. Merged so a
    // timing override keeps it.
    completion: { ignoreSend: true, ...overrides.completion },
  };
}

export class ClaudeWebAdapter extends WebChatAdapter {
  constructor(opts: WebChatOptions = {}, configOverrides: Partial<DeclarativeChatConfig> = {}) {
    super(
      {
        newChatUrl: "https://claude.ai/new",
        threadUrl: (id) => `https://claude.ai/chat/${id}`,
      },
      claudeWebConfig(configOverrides),
      opts
    );
  }

  // Who is signed in. Non-spending: claude.ai's own account endpoint, read
  // in the page (same-origin); only the email leaves the page, never a token.
  // Usage is not read yet (the UI shows it only near a limit).
  async readAccount(_signal: AbortSignal): Promise<AccountObservation> {
    const page = this.attachedPage();
    if (!page) throw new Error("readAccount called before attach()");
    const identity = await page.evaluate(async () => {
      try {
        const r = await fetch("/api/account", { credentials: "include" });
        if (!r.ok) return null;
        const j = (await r.json()) as { email_address?: unknown; account?: { email_address?: unknown } };
        const email = j?.email_address ?? j?.account?.email_address;
        return typeof email === "string" ? email : null;
      } catch {
        return null; // not signed in, or the endpoint moved: identity unknown
      }
    });
    return { identity, usage: null };
  }
}

// Adapter-package convention: the gateway worker pool instantiates adapters
// through a zero-arg `createAdapter()` found at <adapter dir>/adapter.ts.
export function createAdapter(): ClaudeWebAdapter {
  return new ClaudeWebAdapter();
}

export default createAdapter;
