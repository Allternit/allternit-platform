// claude-web adapter — the shared web-chat base (fresh chat, mapped-thread
// continue with the divergence check, fingerprint reconcile) over claude.ai.
// chat.create / chat.continue only; model_class is not applied yet (the
// account's default model answers). Selectors are v1-unverified (see
// selectors/v1.yaml).
import type { AccountObservation } from "@allternit/subscription-fabric-contracts";
import type { DeclarativeChatConfig, SdkSelectorResolver } from "@allternit/subscription-adapter-sdk";
import type { Page } from "playwright";
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

  /**
   * The last assistant message from claude.ai's own conversation endpoint (same-origin, in the page): its text blocks
   * and its thinking blocks, kept apart so a thinking summary never leaks into the reply (live 2026-09-30:
   * "Untangling conflicting instructions…" rode along) and can be streamed as thinking instead. null when the
   * endpoint gives nothing (e.g. mid-stream); callers fall back to the DOM.
   */
  private async lastAssistant(page: Page): Promise<{ text: string; thinking: string } | null> {
    const id = THREAD_URL_PATTERN.exec(page.url())?.[1];
    if (!id) return null;
    await page.evaluate("globalThis.__name ??= (fn) => fn");
    return page
      .evaluate(async (conv: string) => {
        const cookieOrg = /(?:^|;\s*)lastActiveOrg=([^;]+)/.exec(document.cookie)?.[1];
        let org = cookieOrg ? decodeURIComponent(cookieOrg) : null;
        if (!org) {
          const r = await fetch("/api/organizations", { credentials: "include" }).catch(() => null);
          const orgs = r && r.ok ? ((await r.json()) as { uuid?: string; capabilities?: string[] }[]) : [];
          org = orgs.find((o) => o.capabilities?.includes("chat"))?.uuid ?? null;
        }
        if (!org) return null;
        const r = await fetch(`/api/organizations/${org}/chat_conversations/${conv}?tree=True&rendering_mode=messages`, { credentials: "include" }).catch(() => null);
        if (!r || !r.ok) return null;
        type Block = { type?: string; text?: string; thinking?: string };
        const j = (await r.json()) as { chat_messages?: { sender?: string; content?: Block[]; text?: string }[] };
        const last = [...(j.chat_messages ?? [])].reverse().find((m) => m.sender === "assistant");
        if (!last) return null;
        const blocks = Array.isArray(last.content) ? last.content : [];
        const text = (blocks.filter((b) => b.type === "text" && typeof b.text === "string").map((b) => b.text as string).join("\n\n") || last.text || "").trim();
        const thinking = blocks.filter((b) => b.type === "thinking" && typeof b.thinking === "string").map((b) => b.thinking as string).join("\n\n").trim();
        return { text, thinking };
      }, id)
      .catch(() => null);
  }

  protected override async extractReply(page: Page, resolver: SdkSelectorResolver): Promise<string> {
    const last = await this.lastAssistant(page);
    if (last?.text) return last.text;
    return super.extractReply(page, resolver);
  }

  protected override async extractThinking(page: Page, _resolver: SdkSelectorResolver): Promise<string | null> {
    const last = await this.lastAssistant(page);
    return last?.thinking || null;
  }

  // Who is signed in and how much is left. Non-spending: claude.ai's own
  // account and usage endpoints, read in the page (same-origin); only the
  // email and numbers leave the page, never a token. Usage is the active org's
  // (lastActiveOrg cookie, else the first chat org) tightest window: whichever
  // of the 5-hour / 7-day limits is most used.
  async readAccount(_signal: AbortSignal): Promise<AccountObservation> {
    const page = this.attachedPage();
    if (!page) throw new Error("readAccount called before attach()");
    // tsx wraps the named helpers below in __name(...), absent in the page
    // (see the SDK's extract.ts).
    await page.evaluate("globalThis.__name ??= (fn) => fn");
    const read = await page.evaluate(async () => {
      const getJson = async (url: string): Promise<unknown> => {
        try {
          const r = await fetch(url, { credentials: "include" });
          return r.ok ? await r.json() : null;
        } catch {
          return null; // not signed in, or the endpoint moved: unknown
        }
      };
      const acct = (await getJson("/api/account")) as { email_address?: unknown; account?: { email_address?: unknown } } | null;
      const email = acct?.email_address ?? acct?.account?.email_address;
      const identity = typeof email === "string" ? email : null;

      type Window = { utilization?: unknown; resets_at?: unknown } | null;
      let pct: number | null = null;
      let resetsAt: string | null = null;
      const cookieOrg = /(?:^|;\s*)lastActiveOrg=([^;]+)/.exec(document.cookie)?.[1];
      let org = cookieOrg ? decodeURIComponent(cookieOrg) : null;
      if (!org) {
        const orgs = (await getJson("/api/organizations")) as { uuid?: unknown; capabilities?: unknown }[] | null;
        const chat = Array.isArray(orgs)
          ? orgs.find((o) => Array.isArray(o?.capabilities) && o.capabilities.includes("chat"))
          : undefined;
        org = typeof chat?.uuid === "string" ? chat.uuid : null;
      }
      if (org) {
        const u = (await getJson(`/api/organizations/${org}/usage`)) as Record<string, Window> | null;
        for (const w of [u?.five_hour, u?.seven_day]) {
          if (!w || typeof w.utilization !== "number") continue;
          const left = Math.max(0, Math.round(100 - w.utilization));
          if (pct === null || left < pct) {
            pct = left;
            resetsAt = typeof w.resets_at === "string" ? w.resets_at : null;
          }
        }
      }
      return { identity, pct, resetsAt };
    });
    return {
      identity: read.identity,
      usage:
        read.pct === null
          ? null
          : { remaining_pct: read.pct, resets_at: read.resetsAt, observed_at: new Date().toISOString() },
    };
  }
}

// Adapter-package convention: the gateway worker pool instantiates adapters
// through a zero-arg `createAdapter()` found at <adapter dir>/adapter.ts.
export function createAdapter(): ClaudeWebAdapter {
  return new ClaudeWebAdapter();
}

export default createAdapter;
