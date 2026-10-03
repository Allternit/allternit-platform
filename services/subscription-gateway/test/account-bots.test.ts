// Offline-only: every browser request is fulfilled or aborted, never forwarded.
import { readFileSync } from "node:fs";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import type { Browser } from "playwright";
import type { Task } from "@allternit/subscription-fabric-contracts";
import { createPageLease } from "@allternit/subscription-adapter-sdk";
import { launchBrowser } from "./helpers.js";
import { ChatGPTWebAdapter } from "../adapters/chatgpt-web/adapter.js";
import { GeminiWebAdapter } from "../adapters/gemini-web/adapter.js";
import { KimiWebAdapter } from "../adapters/kimi-web/adapter.js";
import { CopilotWebAdapter } from "../adapters/copilot-web/adapter.js";
import { ClaudeWebAdapter } from "../adapters/claude-web/adapter.js";
import { SubscriptionAgentProvider, type GatewayTasks } from "../adapters/_shared/subscription-agent.js";
import { accountBotUrl, safeAvatar } from "../adapters/_shared/account-bots.js";
import { SPEC as CHATGPT } from "../adapters/chatgpt-subscription/index.js";
import { SPEC as GEMINI } from "../adapters/gemini-subscription/index.js";
import { SPEC as KIMI } from "../adapters/kimi-subscription/index.js";
import { SPEC as COPILOT } from "../adapters/copilot-subscription/index.js";
import { GrokBotProvider, ReplayGrokDriver } from "../adapters/grok-bot/index.js";
import { HermesProvider } from "../adapters/hermes/provider.js";
import { startFakeHermes } from "../adapters/hermes/fixtures/fake-server.js";
import { accountSchema } from "@allternit/subscription-fabric-contracts";
import { openDatabase } from "../src/store/db.js";
import { upsertAccount, getAccount } from "../src/store/queries.js";

vi.setConfig({ testTimeout: 30000 });

const cases = [
  { vendor: "chatgpt", spec: CHATGPT, make: () => new ChatGPTWebAdapter(), origin: "https://chatgpt.com", entries: [["gpt", "g-research", "Research GPT", "/g/g-research"], ["project", "g-p-brain", "Brain Project", "/g/g-p-brain/project"]] },
  { vendor: "gemini", spec: GEMINI, make: () => new GeminiWebAdapter(), origin: "https://gemini.google.com", entries: [["gem", "tutor", "Tutor Gem", "/gem/tutor"]] },
  { vendor: "kimi", spec: KIMI, make: () => new KimiWebAdapter(), origin: "https://www.kimi.ai", entries: [["agent", "reviewer", "Reviewer", "/kimiplus/reviewer"]] },
  { vendor: "copilot", spec: COPILOT, make: () => new CopilotWebAdapter(), origin: "https://copilot.microsoft.com", entries: [["agent", "research", "Research agent", "/agents/research"], ["page", "plan", "Plan page", "/pages/plan"]] },
];
let browser: Browser;
beforeAll(async () => { browser = await launchBrowser(); }, 30000);
afterAll(async () => { await browser?.close(); });
const fixture = (vendor: string, name: string) => readFileSync(new URL(`../adapters/${vendor}-web/fixtures/${name}.html`, import.meta.url), "utf8");

describe.each(cases)("$vendor account bots", (c) => {
  it("reads the adapter's rendered account list without vendor writes or network", async () => {
    const page = await browser.newPage();
    const requests: string[] = [];
    await page.route("**/*", async (route) => {
      const req = route.request(); requests.push(req.url());
      if (req.isNavigationRequest()) await route.fulfill({ contentType: "text/html", body: fixture(c.vendor, "account-bots") });
      else if (req.url().endsWith("/api/auth/session")) await route.fulfill({ json: { user: { email: "fixture@example.invalid" } } });
      else if (req.url().includes("GetCurrentUser")) await route.fulfill({ json: { user: { nickname: "Fixture" } } });
      else if (req.url().includes("GetSubscriptionStats")) await route.fulfill({ json: {} });
      else await route.abort();
    });
    try {
      await page.goto(c.origin);
      const adapter = c.make(); await adapter.attach(createPageLease(page));
      const read = await adapter.readAccount(new AbortController().signal);
      expect(read.agents?.map((a) => [a.kind, a.id, a.name])).toEqual(c.entries.map((a) => a.slice(0, 3)));
      expect(read.agents?.every((a) => a.kindLabel)).toBe(true);
      if (["chatgpt", "gemini"].includes(c.vendor)) expect(read.agents?.[0].avatarUrl).toMatch(/^data:image\/png/);
      // The existing account RPCs only; discovery adds zero network calls.
      expect(requests.filter((url) => ![c.origin + "/", c.origin + "/api/auth/session"].includes(url) && !url.includes("GetCurrentUser") && !url.includes("GetSubscriptionStats"))).toEqual([]);
      await page.setContent(fixture(c.vendor, "account-bots-empty"));
      expect((await adapter.readAccount(new AbortController().signal)).agents).toEqual([]);
    } finally { await page.close(); }
  });

  it.each(c.entries)("routes %s %s through list, identity, context and restart", async (kind, id, name, path) => {
    const submitted: Record<string, unknown>[] = [];
    const tasks: GatewayTasks = {
      accountState: async () => ({ health: "ready", agents: [{ kind, id, name, avatarUrl: "https://icons.invalid/avatar.png" }] }),
      submit: async (body) => { submitted.push(body); return body.capability === "chat.continue" ? { status: 409, body: { error: "thread_not_mapped" } } : { status: 202, body: { task_id: "t", status: "completed", result: { text: "fixture reply" } } }; },
      get: async () => ({ status: 200, body: {} }),
    };
    const make = () => new SubscriptionAgentProvider({ spec: c.spec, tasks, sleep: async () => {} });
    const provider = make(); const agentId = `${c.spec.agentId}:${kind}:${id}`;
    expect(await provider.list()).toMatchObject({ ok: true, value: [{ agentId: c.spec.agentId }, { agentId, displayName: name, kind, kindLabel: expect.any(String), avatarUrl: "https://icons.invalid/avatar.png" }] });
    expect(await provider.get(agentId)).toMatchObject({ ok: true, value: { remoteIds: { [kind]: id } } });
    expect(await provider.identity(agentId)).toMatchObject({ ok: true, value: { displayName: name } });
    expect((await provider.capabilities(agentId)).ok).toBe(true);
    expect((await provider.contextOpen({ agentId: `${c.spec.agentId}:${kind}:missing` })).ok).toBe(false);
    const opened = await provider.contextOpen({ agentId }); if (!opened.ok) throw new Error("contextOpen");
    expect((await provider.contextOpen({ agentId: c.spec.agentId, adoptContextId: opened.value.contextId })).ok).toBe(false);
    await provider.contextMessage({ contextId: opened.value.contextId, correlationId: "first", text: "offline" });
    await make().contextMessage({ contextId: opened.value.contextId, correlationId: "revived", text: "offline" });
    for (const body of submitted.filter((b) => b.capability === "chat.create")) {
      expect(body.options).toEqual({ account_bot: { id, kind } });
      const page = await browser.newPage();
      try {
        await page.route("**/*", (r) => r.fulfill({ body: "<!doctype html>" }));
        const adapter = c.make();
        // Exercise the actual navigation seam without submitting a vendor turn.
        await (adapter as any).openFreshChat({ page: createPageLease(page) }, { options: body.options } as Task);
        expect(page.url()).toBe(c.origin + path);
      } finally { await page.close(); }
    }
  });
});

it("reads My GPTs main collection but excludes public Explore GPTs", async () => {
  const page = await browser.newPage();
  try {
    await page.route("**/*", (r) => r.request().isNavigationRequest() ? r.fulfill({ contentType: "text/html", body: fixture("chatgpt", "my-gpts") }) : r.fulfill({ json: {} }));
    const adapter = new ChatGPTWebAdapter(); await adapter.attach(createPageLease(page));
    await page.goto("https://chatgpt.com/gpts/mine");
    expect((await adapter.readAccount(new AbortController().signal)).agents?.[0].id).toBe("g-owned");
    await page.goto("https://chatgpt.com/gpts");
    expect((await adapter.readAccount(new AbortController().signal)).agents).toEqual([]);
  } finally { await page.close(); }
});

it("keeps Claude Project navigation unchanged", async () => {
  const page = await browser.newPage();
  try {
    await page.route("**/*", (r) => r.fulfill({ body: "<!doctype html>" }));
    const adapter = new ClaudeWebAdapter();
    await (adapter as any).openFreshChat({ page: createPageLease(page) }, { options: { project_id: "0f6f1c2e-6a3b-4c1d-9e8f-123456789abc" } });
    expect(page.url()).toBe("https://claude.ai/project/0f6f1c2e-6a3b-4c1d-9e8f-123456789abc");
  } finally { await page.close(); }
});

it("rejects untrusted destinations and unsafe avatars", () => {
  for (const id of ["../escape", "x?evil=1", "https://evil.invalid"]) expect(() => accountBotUrl("google", { account_bot: { kind: "gem", id } }, "https://gemini.google.com")).toThrow();
  expect(() => accountBotUrl("google", { account_bot: { kind: "gpt", id: "x" } }, "https://gemini.google.com")).toThrow();
  for (const url of ["javascript:alert(1)", "data:image/svg+xml,x", "https://u:p@evil.invalid/x", "https://icons.invalid/x.svg"]) expect(safeAvatar(url)).toBeUndefined();
});

it("Grok Bot and Hermes profile lists share kind labels", async () => {
  const grok = new GrokBotProvider({ driver: new ReplayGrokDriver({ bots: ["Fixture Bot"] }), pacing: false });
  const bots = await grok.list();
  expect(bots.ok && bots.value.slice(1).every((a) => a.kind === "bot" && a.kindLabel === "Bot")).toBe(true);
  expect(bots.ok && bots.value.length).toBeGreaterThan(1);
  const fake = await startFakeHermes();
  try {
    const hermes = new HermesProvider({ baseUrl: fake.url });
    expect(await hermes.list()).toMatchObject({ ok: true, value: [expect.objectContaining({ kind: "profile", kindLabel: "Profile" })] });
  } finally { await fake.close(); }
});

it("persists bot labels and avatars through the account contract and store", () => {
  const db = openDatabase(":memory:");
  try {
    const account = accountSchema.parse({ account_id: "fixture", provider: "chatgpt", label: "Fixture", plan: null, plan_observed_at: null, profile_ref: "profiles/fixture", session_health: "ready", enabled: true, agents: [{ id: "g-owned", name: "Owned GPT", kind: "gpt", kindLabel: "GPT", avatarUrl: "https://icons.invalid/owned.png" }] });
    upsertAccount(db, account);
    expect(getAccount(db, "fixture")?.agents).toEqual(account.agents);
  } finally { db.close(); }
});
