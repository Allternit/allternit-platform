import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { Browser } from "playwright";
import type { RedactingLogger } from "@allternit/subscription-fabric-contracts";
import {
  createExecutionContext,
  createPageLease,
  createRedactingLogger,
  createResolver,
} from "../src/index";
import { fixturePage, launchBrowser, loadPack } from "./helpers";

let browser: Browser;
beforeAll(async () => {
  browser = await launchBrowser();
}, 30000);
afterAll(async () => {
  await browser.close();
}, 30000);

describe("PageLease (§A1)", () => {
  it("throws on use after release", async () => {
    const page = await fixturePage(browser, "idle.html");
    const lease = createPageLease(page);
    expect(typeof lease.lease_id).toBe("string");
    expect(lease.url()).toBe("about:blank");
    await lease.release();
    expect(() => lease.url()).toThrow(/release/);
    expect(() => lease.page).toThrow(/release/);
    await page.close();
  });
});

describe("markSubmitted ordering (§A1 two-write rule)", () => {
  it("flips not_sent → sent_unconfirmed → acknowledged, durable write awaited before return", async () => {
    const page = await fixturePage(browser, "idle.html");
    const writes: Array<{ threadId: string | null; state: string }> = [];
    let durable = false;
    const attempt = {
      attempt_no: 1,
      adapter_id: "fixture-web",
      adapter_version: "0.1.0",
      account_id: "acct",
      pool_key: "pool",
      submission_state: "not_sent" as const,
      prompt_fingerprint: "fp",
      provider_thread_id: null,
      requested_model_class: null,
      observed_model: null,
      started_at: new Date().toISOString(),
      ended_at: null,
      outcome: "failed" as const,
      error: null,
    };
    const ctx = createExecutionContext({
      page: createPageLease(page),
      sink: { begin: async () => "a", write: async () => {}, commit: async () => {}, fail: async () => {} },
      pacer: { beforeAction: async () => {}, beforeTask: async () => {} },
      resolver: createResolver(page, loadPack()),
      logger: { debug: () => {}, info: () => {}, warn: () => {}, error: () => {} },
      attempt,
      onMarkSubmitted: async (threadId, state) => {
        await new Promise((r) => setTimeout(r, 5));
        writes.push({ threadId, state });
        durable = true;
      },
    });

    durable = false;
    await ctx.markSubmitted(null);
    expect(durable).toBe(true); // write completed before markSubmitted returned
    expect(attempt.submission_state).toBe("sent_unconfirmed");

    durable = false;
    await ctx.markSubmitted("fw-thread-9");
    expect(durable).toBe(true);
    expect(attempt.submission_state).toBe("acknowledged");
    expect(attempt.provider_thread_id).toBe("fw-thread-9");
    expect(writes.map((w) => w.state)).toEqual(["sent_unconfirmed", "acknowledged"]);
    await page.close();
  });
});

describe("redacting logger (§A6.8)", () => {
  it("masks emails, JWT-shaped strings, and long digit runs in messages and fields", () => {
    const seen: Array<{ level: string; message: string; fields?: Record<string, unknown> }> = [];
    const base: RedactingLogger = {
      debug: (message, fields) => seen.push({ level: "debug", message, fields }),
      info: (message, fields) => seen.push({ level: "info", message, fields }),
      warn: (message, fields) => seen.push({ level: "warn", message, fields }),
      error: (message, fields) => seen.push({ level: "error", message, fields }),
    };
    const log = createRedactingLogger(base);
    log.info("login for bob@example.com failed", {
      token: "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV",
      order: "order 123456789 shipped",
      count: 42,
    });
    expect(seen[0].message).toBe("login for [redacted:email] failed");
    expect(seen[0].fields?.token).toBe("[redacted:token]");
    expect(seen[0].fields?.order).toBe("order [redacted:number] shipped");
    expect(seen[0].fields?.count).toBe(42);
  });
});
