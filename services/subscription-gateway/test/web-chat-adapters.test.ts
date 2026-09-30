// claude-web + kimi-web (shared web-chat base): SDK conformance over the 6
// canonical fixtures, chat.create e2e (fresh chat first), chat.continue
// divergence policy, reconcile outcome mapping, thread-URL patterns, and the
// registry loading both manifests. Fixtures only — no live provider contact.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { Browser, Page } from "playwright";
import {
  createExecutionContext,
  createPacer,
  createPageLease,
  createResolver,
  runConformance,
  threadIdFromUrl,
} from "@allternit/subscription-adapter-sdk";
import type { AdapterEvent, Task, TaskAttempt } from "@allternit/subscription-fabric-contracts";
import { WebChatAdapter, userTurnFingerprint } from "../adapters/_shared/web-chat.js";
import {
  ClaudeWebAdapter,
  THREAD_URL_PATTERN as CLAUDE_THREAD,
  createAdapter as createClaude,
} from "../adapters/claude-web/adapter.js";
import {
  KimiWebAdapter,
  THREAD_URL_PATTERN as KIMI_THREAD,
  createAdapter as createKimi,
} from "../adapters/kimi-web/adapter.js";
import { loadAdapterRegistry } from "../src/adapters/registry.js";
import { launchBrowser } from "./helpers.js";

const FAST = {
  authSettleMs: 300,
  completion: { stabilityMs: 150, pollIntervalMs: 25, timeoutMs: 5000 },
  heartbeatIntervalMs: 200,
  stallTimeoutS: 5,
};

interface Case {
  id: string;
  make: (opts?: { freshChat?: boolean }) => WebChatAdapter;
  newChatUrl: string;
  threadUrl: (id: string) => string;
  routeGlob: string;
  threadPattern: RegExp;
  goodThreadUrls: Array<[string, string]>;
  badThreadUrls: string[];
}

const CASES: Case[] = [
  {
    id: "claude-web",
    make: (opts = {}) => new ClaudeWebAdapter(opts, FAST),
    newChatUrl: "https://claude.ai/new",
    threadUrl: (id) => `https://claude.ai/chat/${id}`,
    routeGlob: "https://claude.ai/**",
    threadPattern: CLAUDE_THREAD,
    goodThreadUrls: [
      [
        "https://claude.ai/chat/0f6f1c2e-6a3b-4c1d-9e8f-123456789abc",
        "0f6f1c2e-6a3b-4c1d-9e8f-123456789abc",
      ],
    ],
    badThreadUrls: ["https://claude.ai/new", "https://claude.ai/project/0f6f1c2e-6a3b-4c1d-9e8f-123456789abc"],
  },
  {
    id: "kimi-web",
    make: (opts = {}) => new KimiWebAdapter(opts, FAST),
    newChatUrl: "https://www.kimi.ai/",
    threadUrl: (id) => `https://www.kimi.ai/chat/${id}`,
    routeGlob: "https://www.kimi.ai/**",
    threadPattern: KIMI_THREAD,
    goodThreadUrls: [
      ["https://www.kimi.com/chat/d3k5a1b2c3d4e5f6g7h8", "d3k5a1b2c3d4e5f6g7h8"],
      ["https://kimi.ai/chat/abc-123", "abc-123"],
    ],
    badThreadUrls: ["https://www.kimi.com/", "https://www.kimi.com/kimiplus/abc"],
  },
];

const fixturesDir = (id: string) => fileURLToPath(new URL(`../adapters/${id}/fixtures/`, import.meta.url));
const fixtureHtml = (id: string, name: string) => readFileSync(join(fixturesDir(id), `${name}.html`), "utf8");

let browser: Browser;
beforeAll(async () => {
  browser = await launchBrowser();
}, 30000);
afterAll(async () => {
  await browser.close();
}, 30000);

async function fixturePage(id: string, name: string): Promise<Page> {
  const page = await browser.newPage();
  await page.setContent(fixtureHtml(id, name));
  return page;
}

function makeAttempt(adapter: WebChatAdapter, over: Partial<TaskAttempt> = {}): TaskAttempt {
  return {
    attempt_no: 1,
    adapter_id: adapter.manifest.adapter_id,
    adapter_version: adapter.manifest.adapter_version,
    account_id: "acct-1",
    pool_key: `${adapter.manifest.provider}:acct-1:chat-msgs`,
    submission_state: "not_sent",
    prompt_fingerprint: "fp",
    provider_thread_id: null,
    requested_model_class: null,
    observed_model: null,
    started_at: new Date().toISOString(),
    ended_at: null,
    outcome: "failed",
    error: null,
    ...over,
  };
}

function makeCtx(page: Page, adapter: WebChatAdapter, attempt: TaskAttempt) {
  const marks: string[] = [];
  const ctx = createExecutionContext({
    page: createPageLease(page),
    sink: { begin: async () => "a", write: async () => {}, commit: async () => {}, fail: async () => {} },
    pacer: createPacer(
      { min_action_gap_ms: [1, 2], min_task_gap_s: 0, max_tasks_per_hour: 1000, max_tasks_per_day: 5000 },
      { rng: () => 0 }
    ),
    resolver: createResolver(page, adapter.pack),
    logger: { debug: () => {}, info: () => {}, warn: () => {}, error: () => {} },
    attempt,
    onMarkSubmitted: async (_t, state) => {
      marks.push(state);
    },
  });
  return { ctx, marks };
}

function makeTask(capability: Task["capability"], options: Record<string, unknown> = {}): Task {
  const now = new Date().toISOString();
  return {
    task_id: "task-wc-1",
    idempotency_key: null,
    capability,
    capability_version: 1,
    requester: { kind: "user", id: "user-1" },
    initiated_by: { kind: "human", user_id: "user-1", action_id: "action-1" },
    thread_id: null,
    project_id: null,
    parent_task_id: null,
    prompt: "Summarize the migration plan.",
    inputs: [],
    options,
    routing: { mode: "auto", allow_fallback: true, allow_metered: false, allow_thread_migration: false },
    constraints: { sensitivity: "internal", deadline_at: null, max_metered_usd: null, required_export_format: null },
    approval_id: null,
    priority: "interactive",
    status: "queued",
    status_detail: null,
    route_decision: null,
    attempts: [],
    result: null,
    error: null,
    created_at: now,
    updated_at: now,
    completed_at: null,
  };
}

async function drain(adapter: WebChatAdapter, task: Task, page: Page, attempt?: TaskAttempt) {
  const { ctx, marks } = makeCtx(page, adapter, attempt ?? makeAttempt(adapter));
  const events: AdapterEvent[] = [];
  for await (const e of adapter.execute(task, ctx)) events.push(e);
  return { events, marks };
}

describe("registry", () => {
  it("loads claude-web and kimi-web beside chatgpt-web; the shared dir is not an adapter", () => {
    const registry = loadAdapterRegistry(fileURLToPath(new URL("../adapters/", import.meta.url)));
    const ids = registry.adapters.map((a) => a.manifest.adapter_id);
    expect(ids).toEqual(expect.arrayContaining(["chatgpt-web", "claude-web", "kimi-web"]));
    expect(ids).not.toContain("_shared");
    expect(registry.byId("claude-web")?.manifest.provider).toBe("claude");
    expect(registry.byId("kimi-web")?.manifest.provider).toBe("kimi");
  });

  it("createAdapter() builds each adapter with its own manifest", () => {
    expect(createClaude().manifest.adapter_id).toBe("claude-web");
    expect(createKimi().manifest.adapter_id).toBe("kimi-web");
  });
});

describe.each(CASES)("$id", (c) => {
  it("passes the shared conformance suite against its fixtures", async () => {
    const report = await runConformance(
      () => {
        const adapter = c.make({ freshChat: false });
        const cfg = (adapter as unknown as { cfg: { banners: never; sampleThreadUrl: string; sampleThreadId: string } })
          .cfg;
        return {
          pack: adapter.pack,
          banners: cfg.banners,
          threadUrlPattern: c.threadPattern,
          sampleThreadUrl: cfg.sampleThreadUrl,
          sampleThreadId: cfg.sampleThreadId,
          probeInput: adapter.probeInput(),
        };
      },
      fixturesDir(c.id),
      { browser }
    );
    expect(report.failures).toEqual([]);
    expect(report.ok).toBe(true);
  }, 60000);

  it("thread URL pattern: matches thread pages only", () => {
    for (const [url, id] of c.goodThreadUrls) expect(threadIdFromUrl(url, c.threadPattern)).toBe(id);
    for (const url of c.badThreadUrls) expect(threadIdFromUrl(url, c.threadPattern)).toBeNull();
  });

  it("chat.create: opens a fresh chat first, sends, acknowledges on evidence, ends done with the reply", async () => {
    const adapter = c.make();
    const html = fixtureHtml(c.id, "complete");
    const page = await browser.newPage();
    const served: string[] = [];
    await page.route(c.routeGlob, (route) => {
      served.push(route.request().url());
      return route.fulfill({ contentType: "text/html", body: html });
    });
    // The lane page as a previous chat.continue left it.
    await page.goto(c.threadUrl("prev-thread-1"));
    const { events, marks } = await drain(adapter, makeTask("chat.create"), page);

    expect(served).toEqual([c.threadUrl("prev-thread-1"), c.newChatUrl]);
    expect(await page.evaluate(() => document.body.dataset.submitted)).toBe("true");
    expect(marks).toEqual(["sent_unconfirmed", "acknowledged"]);
    const kinds = events.map((e) => e.t);
    expect(kinds[0]).toBe("submitted");
    const done = events[events.length - 1];
    expect(done.t).toBe("done");
    expect(done.t === "done" && done.text).toContain("three stages");
    await page.close();
  }, 30000);

  it("chat.create on a challenge page → needs_user, nothing sent", async () => {
    const adapter = c.make({ freshChat: false });
    const page = await fixturePage(c.id, "challenge");
    const { events, marks } = await drain(adapter, makeTask("chat.create"), page);
    expect(events).toEqual([expect.objectContaining({ t: "needs_user", reason: "challenge" })]);
    expect(marks).toEqual([]);
    await page.close();
  }, 30000);

  it("chat.create at the usage limit → stops before typing, limit error, nothing sent", async () => {
    const adapter = c.make({ freshChat: false });
    const page = await fixturePage(c.id, "limit-banner");
    const { events, marks } = await drain(adapter, makeTask("chat.create"), page);
    expect(events.map((e) => (e as { t: string }).t)).not.toContain("submitted");
    expect(events).toContainEqual(expect.objectContaining({ t: "error", error: expect.objectContaining({ class: "quota_exhausted", fallback_eligible: true }) }));
    expect(marks).toEqual([]);
    await page.close();
  }, 30000);

  it("chat.create logged out → needs_user auth, nothing sent", async () => {
    const adapter = c.make({ freshChat: false });
    const page = await fixturePage(c.id, "logged-out");
    const { events, marks } = await drain(adapter, makeTask("chat.create"), page);
    expect(events).toEqual([expect.objectContaining({ t: "needs_user", reason: "auth" })]);
    expect(marks).toEqual([]);
    await page.close();
  }, 30000);

  it("chat.continue: opens the mapped thread; fingerprint match sends, mismatch (fail) errors, fork asks", async () => {
    const adapter = c.make({ freshChat: false });
    const html = fixtureHtml(c.id, "complete");
    const probe = await fixturePage(c.id, "complete");
    const snapshot = await adapter.readThread("t-1", makeCtx(probe, adapter, makeAttempt(adapter)).ctx);
    await probe.close();

    const open = async () => {
      const page = await browser.newPage();
      const served: string[] = [];
      await page.route(c.routeGlob, (route) => {
        served.push(route.request().url());
        return route.fulfill({ contentType: "text/html", body: html });
      });
      return { page, served };
    };

    const ok = await open();
    const match = await drain(
      adapter,
      makeTask("chat.continue", { provider_thread_id: "t-1", last_turn_fingerprint: snapshot.last_turn_fingerprint }),
      ok.page
    );
    expect(ok.served).toEqual([c.threadUrl("t-1")]);
    expect(match.events[match.events.length - 1].t).toBe("done");
    await ok.page.close();

    const bad = await open();
    const fail = await drain(
      adapter,
      makeTask("chat.continue", { provider_thread_id: "t-1", last_turn_fingerprint: "deadbeef" }),
      bad.page
    );
    expect(fail.events).toEqual([
      expect.objectContaining({ t: "error", error: expect.objectContaining({ class: "user_intervention_required" }) }),
    ]);
    expect(await bad.page.evaluate(() => document.body.dataset.submitted)).toBeUndefined();
    await bad.page.close();

    const fk = await open();
    const fork = await drain(
      adapter,
      makeTask("chat.continue", { provider_thread_id: "t-1", last_turn_fingerprint: "deadbeef", on_divergence: "fork" }),
      fk.page
    );
    expect(fork.events).toEqual([expect.objectContaining({ t: "needs_user", reason: "confirm_dialog" })]);
    await fk.page.close();
  }, 60000);

  it("chat.continue without a provider thread → user_intervention_required, nothing sent", async () => {
    const adapter = c.make({ freshChat: false });
    const page = await fixturePage(c.id, "complete");
    const { events } = await drain(adapter, makeTask("chat.continue"), page);
    expect(events).toEqual([
      expect.objectContaining({ t: "error", error: expect.objectContaining({ detail: "chat.continue requires a provider thread" }) }),
    ]);
    await page.close();
  }, 30000);

  it("reconcile: matching last user turn → acknowledged; different → ambiguous; empty → not_found", async () => {
    const adapter = c.make({ freshChat: false });
    const fp = userTurnFingerprint("Summarize the migration plan.");

    const page = await fixturePage(c.id, "complete");
    const ack = await adapter.reconcile(
      makeAttempt(adapter, { prompt_fingerprint: fp }),
      makeCtx(page, adapter, makeAttempt(adapter)).ctx
    );
    expect(ack.outcome).toBe("acknowledged");
    const amb = await adapter.reconcile(
      makeAttempt(adapter, { prompt_fingerprint: "deadbeef" }),
      makeCtx(page, adapter, makeAttempt(adapter)).ctx
    );
    expect(amb.outcome).toBe("ambiguous");
    await page.close();

    const empty = await fixturePage(c.id, "logged-out");
    const gone = await adapter.reconcile(
      makeAttempt(adapter, { prompt_fingerprint: fp }),
      makeCtx(empty, adapter, makeAttempt(adapter)).ctx
    );
    expect(gone.outcome).toBe("not_found");
    await empty.close();
  }, 30000);
});

describe("claude-web readAccount", () => {
  it("reads the signed-in email from claude.ai's account endpoint, never a token", async () => {
    const adapter = new ClaudeWebAdapter({ freshChat: false }, FAST);
    const page = await browser.newPage();
    await page.route("https://claude.ai/**", (route) =>
      route.request().url().endsWith("/api/account")
        ? route.fulfill({
            contentType: "application/json",
            body: JSON.stringify({ email_address: "eoj@example.com", session_token: "sk-secret" }),
          })
        : route.fulfill({ contentType: "text/html", body: fixtureHtml("claude-web", "idle") })
    );
    await page.goto("https://claude.ai/new");
    await adapter.attach({ page } as never);
    const read = await adapter.readAccount(new AbortController().signal);
    expect(read).toEqual({ identity: "eoj@example.com", usage: null });
    expect(JSON.stringify(read)).not.toContain("sk-secret");
    await page.close();
  }, 30000);
});
