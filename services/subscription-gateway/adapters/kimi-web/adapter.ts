import { readAccountBots, accountBotUrl } from "../_shared/account-bots.js";
// kimi-web adapter — the shared web-chat base (fresh chat, mapped-thread
// continue with the divergence check, fingerprint reconcile) over
// www.kimi.ai (where the session lives; www.kimi.com has separate storage). chat.create / chat.continue only; model_class is not applied
// yet (the account's default model answers). readAccount uses Kimi's own
// same-origin account/membership RPCs (shapes from the 2026-09-30 live probe).
// Selectors are v1-unverified (see selectors/v1.yaml).
import type { AccountObservation } from "@allternit/subscription-fabric-contracts";
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
      { kind: "limit_banner", pattern: /(reached|hit) (the |your )?(usage |daily |message |chat )?limit/i, blocksSend: true },
      { kind: "limit_banner", pattern: /(not enough|insufficient|out of) credits/i, blocksSend: true },
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
        newChatUrlFor: (task) => accountBotUrl("kimi", task.options, "https://www.kimi.ai"),
        threadUrl: (id) => `https://www.kimi.ai/chat/${id}`,
      },
      kimiWebConfig(configOverrides),
      opts
    );
  }

  // Who is signed in and how much of the plan is left. Non-spending: the
  // page's own Connect RPCs, called in the page with the page's stored bearer;
  // only the name and numbers leave the page, never a token. Usage is the
  // subscription's credit balance (Settings → Subscription "Total usage").
  async readAccount(_signal: AbortSignal): Promise<AccountObservation> {
    const page = this.attachedPage();
    if (!page) throw new Error("readAccount called before attach()");
    // tsx wraps the named helpers below in __name(...), absent in the page
    // (see the SDK's extract.ts).
    await page.evaluate("globalThis.__name ??= (fn) => fn");
    const read = await page.evaluate(async () => {
      const tok = localStorage.getItem("access_token");
      const rpc = async (svc: string): Promise<unknown> => {
        try {
          const r = await fetch(`/apiv2/${svc}`, {
            method: "POST",
            credentials: "include",
            headers: {
              "content-type": "application/json",
              "connect-protocol-version": "1",
              ...(tok ? { authorization: `Bearer ${tok}` } : {}),
            },
            body: "{}",
          });
          return r.ok ? await r.json() : null;
        } catch {
          return null; // not signed in, or the endpoint moved: unknown
        }
      };
      const me = (await rpc("kimi.gateway.account.v1.UserService/GetCurrentUser")) as {
        user?: { nickname?: unknown; phone?: { countryCode?: unknown; number?: unknown } };
      } | null;
      const nick = me?.user?.nickname;
      const phone = me?.user?.phone;
      const identity =
        typeof nick === "string" && nick
          ? nick
          : typeof phone?.number === "string"
            ? `+${String(phone.countryCode ?? "")} ${phone.number}`
            : (document.querySelector("span.user-name")?.textContent?.trim() || null);
      const stats = (await rpc("kimi.gateway.membership.v2.MembershipService/GetSubscriptionStats")) as {
        subscriptionBalance?: { amountUsedRatio?: unknown; expireTime?: unknown };
      } | null;
      const bal = stats?.subscriptionBalance;
      const pct =
        typeof bal?.amountUsedRatio === "number"
          ? Math.max(0, Math.round((1 - bal.amountUsedRatio) * 1000) / 10)
          : null;
      const resetsAt = typeof bal?.expireTime === "string" ? bal.expireTime : null;
      return { identity: identity === "Log in" ? null : identity, pct, resetsAt };
    });
    const agents = await readAccountBots(page, "kimi");
    return {
      agents,
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
export function createAdapter(): KimiWebAdapter {
  return new KimiWebAdapter();
}

export default createAdapter;
