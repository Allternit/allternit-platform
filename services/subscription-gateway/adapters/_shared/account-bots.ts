// INFERRED: account navigation link shapes, not live-verified. Read only the
// rendered account sidebar / My GPTs collection; never crawl public catalogs,
// call guessed endpoints, open menus, or create/edit/delete vendor objects.
import type { Page } from "playwright";
import type { AccountObservation, Task } from "@allternit/subscription-fabric-contracts";

export type AccountBot = NonNullable<AccountObservation["agents"]>[number];
export const KIND_LABELS: Record<string, string> = {
  project: "Project", gpt: "GPT", gem: "Gem", agent: "Agent", space: "Space", page: "Page",
};
const RULES: Record<string, { kind: string; path: string; target: string }[]> = {
  chatgpt: [
    { kind: "project", path: "^/g/(g-p-[\\w-]+)/project/?$", target: "/g/$id/project" },
    { kind: "gpt", path: "^/g/(g-(?!p-)[\\w-]+)/?$", target: "/g/$id" },
  ],
  google: [{ kind: "gem", path: "^/gem/([\\w-]+)/?$", target: "/gem/$id" }],
  kimi: [{ kind: "agent", path: "^/kimiplus/([\\w-]+)/?$", target: "/kimiplus/$id" }],
  microsoft: [
    { kind: "agent", path: "^/agents/([\\w-]+)/?$", target: "/agents/$id" },
    { kind: "page", path: "^/pages/([\\w-]+)/?$", target: "/pages/$id" },
  ],
};

/** Only allow public raster/HTTPS avatar URLs, never script, SVG, or credentials. */
export function safeAvatar(url: unknown): string | undefined {
  if (typeof url !== "string") return undefined;
  if (/^data:image\/(?:png|jpeg|webp|gif);base64,[a-z0-9+/=]+$/i.test(url)) return url;
  try { const u = new URL(url); return u.protocol === "https:" && !u.username && !u.password && !/\.svg(?:$|[?#])/i.test(url) ? u.href : undefined; }
  catch { return undefined; }
}

export async function readAccountBots(page: Page, provider: string): Promise<AccountBot[]> {
  await page.evaluate("globalThis.__name ??= (fn) => fn");
  const entries = await page.evaluate(({ rules, labels, provider }) => {
    const found: { id: string; name: string; kind: string; kindLabel: string; avatarUrl?: string }[] = [];
    const seen = new Set<string>();
    // INFERRED semantic account navigation containers. My GPTs is a collection
    // page; do not treat Explore GPTs (public catalog) as account ownership.
    const selector = provider === "chatgpt" && location.pathname === "/gpts/mine"
      ? 'nav a[href], aside a[href], main a[href]'
      : 'nav a[href], aside a[href], [data-testid="account-bots"] a[href]';
    for (const a of document.querySelectorAll(selector)) {
      let url: URL;
      try { url = new URL(a.getAttribute("href") ?? "", location.href); } catch { continue; }
      if (url.origin !== location.origin || url.username || url.password) continue;
      for (const rule of rules) {
        const m = new RegExp(rule.path).exec(url.pathname);
        const name = (a.getAttribute("aria-label") ?? a.textContent ?? "").trim();
        if (!m || !name || /^(create|new|explore|discover)\b/i.test(name)) continue;
        const key = `${rule.kind}:${m[1]}`;
        if (seen.has(key)) continue;
        seen.add(key);
        const img = a.querySelector("img");
        const src = img?.getAttribute("src");
        let avatarUrl: string | undefined;
        try { avatarUrl = src ? new URL(src, location.href).href : undefined; } catch { /* unknown */ }
        found.push({ id: m[1], name, kind: rule.kind, kindLabel: labels[rule.kind], ...(avatarUrl ? { avatarUrl } : {}) });
      }
    }
    return found;
  }, { rules: RULES[provider] ?? [], labels: KIND_LABELS, provider });
  return entries.map(({ avatarUrl, ...entry }) => ({ ...entry, ...(safeAvatar(avatarUrl) ? { avatarUrl: safeAvatar(avatarUrl) } : {}) }));
}

/** Construct a same-origin chat target, never navigate a URL supplied by a bot. */
export function accountBotUrl(provider: string, options: Task["options"], origin: string): string | null {
  const bot = options.account_bot as { id?: unknown; kind?: unknown } | undefined;
  if (!bot) return null;
  if (typeof bot.id !== "string" || !/^[\w-]+$/.test(bot.id)) throw new Error("Invalid account bot id");
  const rule = (RULES[provider] ?? []).find((r) => r.kind === bot.kind);
  if (!rule) throw new Error("Unsupported account bot kind");
  const path = rule.target.replace("$id", bot.id);
  if (!new RegExp(rule.path).test(path)) throw new Error("Invalid account bot target");
  return new URL(path, origin).href;
}
