import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { Browser, Page } from "playwright";
import type {
  AdapterEvent,
  CapabilityId,
  Task,
  TaskAttempt,
  TaskError,
} from "@allternit/subscription-fabric-contracts";
import {
  DeclarativeChatAdapter,
  createExecutionContext,
  createPageLease,
  createPacer,
  createResolver,
  type DeclarativeChatConfig,
} from "../src/index";
import { fixturePage, launchBrowser } from "./helpers";
import { fixtureWebConfig } from "./fixture-web";

let browser: Browser;
beforeAll(async () => {
  browser = await launchBrowser();
}, 30000);
afterAll(async () => {
  await browser.close();
}, 30000);

function makeTask(): Task {
  const now = new Date().toISOString();
  return {
    task_id: "task-fw-1",
    idempotency_key: null,
    capability: "chat.create" as CapabilityId,
    capability_version: 1,
    requester: { kind: "user", id: "fixture-user" },
    initiated_by: { kind: "human", user_id: "fixture-user", action_id: "action-1" },
    thread_id: null,
    project_id: null,
    parent_task_id: null,
    prompt: "hello fixture",
    inputs: [],
    options: {},
    routing: {
      mode: "auto",
      allow_fallback: true,
      allow_metered: false,
      allow_thread_migration: false,
    },
    constraints: {
      sensitivity: "internal",
      deadline_at: null,
      max_metered_usd: null,
      required_export_format: null,
    },
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

function makeAttempt(): TaskAttempt {
  return {
    attempt_no: 1,
    adapter_id: "fixture-web",
    adapter_version: "0.1.0",
    account_id: "acct-fw-1",
    pool_key: "fixture-free-pool",
    submission_state: "not_sent",
    prompt_fingerprint: "fixture-fingerprint",
    provider_thread_id: null,
    requested_model_class: null,
    observed_model: null,
    started_at: new Date().toISOString(),
    ended_at: null,
    outcome: "failed",
    error: null,
  };
}

interface RunResult {
  events: AdapterEvent[];
  marks: Array<{ threadId: string | null; state: string; submittedDomFlag: string | undefined }>;
  attempt: TaskAttempt;
  page: Page;
}

async function runAdapter(
  fixture: string,
  overrides: Partial<DeclarativeChatConfig> = {},
  prepare?: (page: Page) => Promise<void>
): Promise<RunResult> {
  const config = fixtureWebConfig(overrides);
  const adapter = new DeclarativeChatAdapter(config);
  const page = await fixturePage(browser, fixture);
  if (prepare) await prepare(page);
  const marks: RunResult["marks"] = [];
  const attempt = makeAttempt();
  const ctx = createExecutionContext({
    page: createPageLease(page),
    sink: {
      begin: async () => "artifact-fw-1",
      write: async () => {},
      commit: async () => {},
      fail: async () => {},
    },
    pacer: createPacer(config.manifest.pacing, { rng: () => 0 }),
    resolver: createResolver(page, adapter.pack),
    logger: { debug: () => {}, info: () => {}, warn: () => {}, error: () => {} },
    attempt,
    onMarkSubmitted: async (threadId, state) => {
      const submittedDomFlag = await page.evaluate(() => document.body.dataset.fwSubmitted);
      marks.push({ threadId, state, submittedDomFlag });
    },
  });
  const events: AdapterEvent[] = [];
  for await (const event of adapter.execute(makeTask(), ctx)) events.push(event);
  return { events, marks, attempt, page };
}

function types(events: AdapterEvent[]): string[] {
  return events.map((e) => (e as { t: string }).t);
}

describe("DeclarativeChatAdapter end-to-end (§A3.3, P2 verify)", () => {
  it("idle: submitted → done, markSubmitted(sent_unconfirmed) before Send click", async () => {
    const { events, marks, attempt, page } = await runAdapter("idle.html");
    expect(types(events)[0]).toBe("submitted");
    expect(types(events)[types(events).length - 1]).toBe("done");
    // §A1 two-write ordering: first durable write precedes the Send click.
    expect(marks.map((m) => m.state)).toEqual(["sent_unconfirmed", "acknowledged"]);
    expect(marks[0].submittedDomFlag).toBeUndefined();
    expect(marks[1].submittedDomFlag).toBe("click");
    expect(attempt.submission_state).toBe("acknowledged");
    await page.close();
  });

  it("composer drift: only hidden composers match → provider_ui_changed, nothing sent, never marks", async () => {
    const { events, marks, page } = await runAdapter("composer-drift.html");
    expect(types(events)).toEqual(["error"]);
    const error = (events[0] as { error: TaskError }).error;
    expect(error.class).toBe("provider_ui_changed");
    expect(error.retryable).toBe(true);
    expect(error.detail).toContain("no visible match");
    expect(marks).toEqual([]);
    expect(await page.evaluate(() => document.body.dataset.fwSubmitted)).toBeUndefined();
    await page.close();
  });

  it("no provider evidence after Send: attempt stays sent_unconfirmed, never acknowledged", async () => {
    const { marks, attempt, page } = await runAdapter("unacked.html", { ackTimeoutMs: 300 });
    expect(marks.map((m) => m.state)).toEqual(["sent_unconfirmed"]);
    expect(attempt.submission_state).toBe("sent_unconfirmed");
    await page.close();
  });

  it("a replaced composer is not evidence of a send: stays sent_unconfirmed", async () => {
    const { marks, attempt, page } = await runAdapter("swap-on-send.html", { ackTimeoutMs: 300 });
    expect(marks.map((m) => m.state)).toEqual(["sent_unconfirmed"]);
    expect(attempt.submission_state).toBe("sent_unconfirmed");
    await page.close();
  });

  it("complete: full event sequence with reply events and markdown text", async () => {
    const { events, page } = await runAdapter("complete.html");
    const ts = types(events);
    expect(ts[0]).toBe("submitted");
    expect(ts).toContain("reply");
    expect(ts[ts.length - 1]).toBe("done");
    const done = events[events.length - 1] as Extract<AdapterEvent, { t: "done" }>;
    expect(done.outcome).toBe("success");
    expect(done.text).toContain("```ts");
    expect(done.text).toContain("[[1] example reference](https://example.com/ref-1)");
    await page.close();
  });

  it("streaming: heartbeats with growing elapsed_s, then stalled error (§A8 not retryable once acknowledged)", async () => {
    let t = 1_700_000_000_000;
    const { events, page } = await runAdapter("streaming.html", {
      stallTimeoutS: 1,
      heartbeatIntervalMs: 200,
      completion: {
        now: () => t,
        sleep: async (ms) => {
          t += ms;
        },
        stabilityMs: 150,
        pollIntervalMs: 25,
        timeoutMs: 60_000,
      },
    });
    const ts = types(events);
    expect(ts[0]).toBe("submitted");
    const heartbeats = events.filter(
      (e) => (e as { t: string }).t === "progress.heartbeat"
    ) as unknown as Array<{ elapsed_s: number }>;
    expect(heartbeats.length).toBeGreaterThanOrEqual(2);
    expect(heartbeats[heartbeats.length - 1].elapsed_s).toBeGreaterThan(heartbeats[0].elapsed_s);
    const last = events[events.length - 1] as Extract<AdapterEvent, { t: "error" }>;
    expect(last.t).toBe("error");
    expect(last.error.class).toBe("stalled");
    expect(last.error.retryable).toBe(false); // submission_state = acknowledged
    // The detail says how the send went, so live failures carry evidence.
    expect(last.error.detail).toMatch(/sent by (click|enter), acknowledged by \w+/);
    await page.close();
  });

  it("a provider prompt in place of the answer ends the turn as needs_user, not a stall", async () => {
    let t = 1_700_000_000_000;
    const { events, page } = await runAdapter("idle.html", {
      stallTimeoutS: 5,
      interrupts: [{ pattern: /do you like this personality\?/i, message: "Answer ChatGPT's question in its window." }],
      completion: { now: () => t, sleep: async (ms) => { t += ms; }, stabilityMs: 150, pollIntervalMs: 25, timeoutMs: 60_000 },
    }, async (pg) => {
      // After Send, the provider shows a survey and no reply text.
      await pg.evaluate(() => {
        document.addEventListener("click", () => setTimeout(() => {
          const d = document.createElement("div"); d.textContent = "Do you like this personality?"; document.body.appendChild(d);
          const s = document.createElement("button"); s.setAttribute("aria-label", "Stop"); s.textContent = "Stop"; document.body.appendChild(s);
        }, 10), { once: true });
      });
    });
    const last = events[events.length - 1] as Extract<AdapterEvent, { t: "needs_user" }>;
    expect(last).toMatchObject({ t: "needs_user", reason: "confirm_dialog", message: "Answer ChatGPT's question in its window." });
    await page.close();
  });

  it("stall watchdog gate: a heartbeating, growing fixture does NOT trip stalled", async () => {
    let t = 1_700_000_000_000;
    const page = await fixturePage(browser, "streaming.html");
    const config = fixtureWebConfig({
      stallTimeoutS: 1,
      heartbeatIntervalMs: 100,
      completion: {
        now: () => t,
        // DOM grows on every poll tick → last_change_at stays fresh.
        sleep: async (ms) => {
          t += ms;
          await page.evaluate(() => {
            const el = document.querySelector("[data-testid='fw-response'] p");
            if (el) el.textContent = `${el.textContent}•`;
          });
        },
        stabilityMs: 150,
        pollIntervalMs: 25,
        timeoutMs: 600_000,
      },
    });
    const adapter = new DeclarativeChatAdapter(config);
    const attempt = makeAttempt();
    const ctx = createExecutionContext({
      page: createPageLease(page),
      sink: { begin: async () => "a", write: async () => {}, commit: async () => {}, fail: async () => {} },
      pacer: createPacer(config.manifest.pacing, { rng: () => 0 }),
      resolver: createResolver(page, adapter.pack),
      logger: { debug: () => {}, info: () => {}, warn: () => {}, error: () => {} },
      attempt,
      onMarkSubmitted: async () => {},
    });
    const events: AdapterEvent[] = [];
    for await (const event of adapter.execute(makeTask(), ctx)) {
      events.push(event);
      if (events.length >= 12) break;
    }
    expect(events.some((e) => (e as { t: string }).t === "error")).toBe(false);
    expect(events.some((e) => (e as { t: string }).t === "progress.heartbeat")).toBe(true);
    expect(events.some((e) => (e as { t: string }).t === "progress")).toBe(true);
    await page.close();
  });

  it("live reply: the new turn streams as text deltas that add up to done.text; the previous turn never streams", async () => {
    let t = 1_700_000_000_000;
    let tick = 0;
    const words = ["Hello", " there,", " this", " reply", " grows", " word", " by", " word."];
    const page = await fixturePage(browser, "streaming.html");
    const config = fixtureWebConfig({
      stallTimeoutS: 30,
      heartbeatIntervalMs: 60_000,
      completion: {
        now: () => t,
        sleep: async (ms) => {
          t += ms;
          tick++;
          await page.evaluate(
            ({ tick, words }) => {
              const transcript = document.getElementById("fw-transcript")!;
              let live = document.getElementById("fw-live");
              if (tick === 2) {
                live = document.createElement("div");
                live.id = "fw-live";
                live.dataset.testid = "fw-response";
                live.className = "fw-response";
                live.innerHTML = "<p></p>";
                transcript.insertBefore(live, document.querySelector("[data-testid='fw-streaming']"));
              }
              const i = tick - 2;
              if (live && i >= 0 && i < words.length) {
                live.querySelector("p")!.textContent += words[i];
              }
              if (i === words.length + 2) {
                document.querySelector("[data-testid='fw-stop']")?.remove();
                document.querySelector("[data-testid='fw-streaming']")?.remove();
                document.getElementById("fw-send")!.removeAttribute("disabled");
              }
            },
            { tick, words }
          );
        },
        stabilityMs: 150,
        pollIntervalMs: 150,
        timeoutMs: 600_000,
      },
    });
    const adapter = new DeclarativeChatAdapter(config);
    const ctx = createExecutionContext({
      page: createPageLease(page),
      sink: { begin: async () => "a", write: async () => {}, commit: async () => {}, fail: async () => {} },
      pacer: createPacer(config.manifest.pacing, { rng: () => 0 }),
      resolver: createResolver(page, adapter.pack),
      logger: { debug: () => {}, info: () => {}, warn: () => {}, error: () => {} },
      attempt: makeAttempt(),
      onMarkSubmitted: async () => {},
    });
    const events: AdapterEvent[] = [];
    for await (const event of adapter.execute(makeTask(), ctx)) events.push(event);

    const deltas = events
      .filter((e) => e.t === "reply" && (e as { event: { type: string } }).event.type === "reply.text.delta")
      .map((e) => (e as unknown as { event: { delta: string } }).event.delta);
    const done = events[events.length - 1] as Extract<AdapterEvent, { t: "done" }>;
    expect(done.t).toBe("done");
    expect(done.text).toBe("Hello there, this reply grows word by word.");
    // Streamed live, not in one piece at the end.
    expect(deltas.length).toBeGreaterThan(2);
    expect(deltas.join("")).toBe(done.text);
    expect(deltas.join("")).not.toContain("The answer so far is");
    const started = events.filter(
      (e) => e.t === "reply" && (e as { event: { type: string } }).event.type === "reply.started"
    );
    expect(started).toHaveLength(1);
    await page.close();
  });

  it("a limit that blocks sending stops the task before anything is typed (fallback-eligible)", async () => {
    const { events, marks, page } = await runAdapter("limit-banner.html", {
      banners: [{ kind: "limit_banner", pattern: /limit reached/i, blocksSend: true }],
    });
    expect(types(events)).not.toContain("submitted");
    expect(marks).toEqual([]);
    const err = events.find((e) => (e as { t: string }).t === "error") as Extract<AdapterEvent, { t: "error" }>;
    expect(err.error).toMatchObject({ class: "quota_exhausted", fallback_eligible: true, retryable: true });
    expect(await page.evaluate(() => document.body.dataset.fwSubmitted)).toBeUndefined();
    await page.close();
  });

  it("limit-banner: quota.signal emitted for the limit banners", async () => {
    const { events, page } = await runAdapter("limit-banner.html");
    const signals = events.filter(
      (e): e is Extract<AdapterEvent, { t: "quota.signal" }> =>
        (e as { t: string }).t === "quota.signal"
    );
    expect(signals.length).toBeGreaterThanOrEqual(1);
    expect(signals[0].signal.kind).toBe("limit_banner");
    expect(signals[0].pool_id).toBe("fixture-free-pool");
    expect(signals[0].signal.task_id).toBe("task-fw-1");
    expect(types(events)).toContain("submitted");
    await page.close();
  });

  it("challenge: needs_user(challenge), never submits, never marks", async () => {
    const { events, marks, page } = await runAdapter("challenge.html");
    expect(events).toHaveLength(1);
    expect(events[0]).toMatchObject({ t: "needs_user", reason: "challenge" });
    expect(marks).toHaveLength(0);
    await page.close();
  });

  it("a single-page app that draws its signed-in UI late is waited for, not judged logged out", async () => {
    const { events, page } = await runAdapter("idle.html", {}, async (p) => {
      await p.evaluate(() => {
        const menu = document.querySelector("[data-testid=fw-user-menu], .fw-user-menu");
        const parent = menu?.parentElement;
        if (!menu || !parent) throw new Error("fixture has no user menu");
        menu.remove();
        setTimeout(() => parent.appendChild(menu), 400);
      });
    });
    expect(types(events)).not.toContain("needs_user");
    expect(types(events)).toContain("submitted");
    await page.close();
  });

  it("logged-out: needs_user(auth), never submits", async () => {
    const { events, marks, page } = await runAdapter("logged-out.html", { authSettleMs: 300 });
    expect(events).toHaveLength(1);
    expect(events[0]).toMatchObject({ t: "needs_user", reason: "auth" });
    expect(marks).toHaveLength(0);
    await page.close();
  });

  it("readThread returns a snapshot with a content fingerprint", async () => {
    const config = fixtureWebConfig();
    const adapter = new DeclarativeChatAdapter(config);
    const page = await fixturePage(browser, "complete.html");
    const ctx = createExecutionContext({
      page: createPageLease(page),
      sink: { begin: async () => "a", write: async () => {}, commit: async () => {}, fail: async () => {} },
      pacer: createPacer(config.manifest.pacing),
      resolver: createResolver(page, adapter.pack),
      logger: { debug: () => {}, info: () => {}, warn: () => {}, error: () => {} },
      attempt: makeAttempt(),
      onMarkSubmitted: async () => {},
    });
    const snapshot = await adapter.readThread("fw-thread-1", ctx);
    expect(snapshot.provider_thread_id).toBe("fw-thread-1");
    expect(snapshot.turn_count).toBe(1);
    expect(snapshot.last_turn_fingerprint).toMatch(/^[0-9a-f]{64}$/);
    await page.close();
  });

  it("reconcile: not_found with no response, ambiguous with unconfirmed thread", async () => {
    const config = fixtureWebConfig();
    const adapter = new DeclarativeChatAdapter(config);
    const mkCtx = (page: Page) =>
      createExecutionContext({
        page: createPageLease(page),
        sink: { begin: async () => "a", write: async () => {}, commit: async () => {}, fail: async () => {} },
        pacer: createPacer(config.manifest.pacing),
        resolver: createResolver(page, adapter.pack),
        logger: { debug: () => {}, info: () => {}, warn: () => {}, error: () => {} },
        attempt: makeAttempt(),
        onMarkSubmitted: async () => {},
      });
    const idlePage = await fixturePage(browser, "idle.html");
    const notFound = await adapter.reconcile(makeAttempt(), mkCtx(idlePage));
    expect(notFound.outcome).toBe("not_found");
    await idlePage.close();

    const completePage = await fixturePage(browser, "complete.html");
    const ambiguous = await adapter.reconcile(makeAttempt(), mkCtx(completePage));
    expect(ambiguous.outcome).toBe("ambiguous");
    await completePage.close();
  });
});
