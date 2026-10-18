// Model-picker chats (POST /v1/tasks) on the RouteDirector: a phone or
// desktop-app route runs the turn through its AAI adapter and the task reads
// exactly like a web-worker task; web / NO_ROUTE keep today's queue path.
import { afterEach, describe, expect, it } from "vitest";
import request from "supertest";
import { createScheduler, type Scheduler } from "../src/queue/scheduler.js";
import { AppRouteTasks, type AppTurnHost, type TurnDirector } from "../src/routes/app-route-tasks.js";
import { issueToken } from "../src/security/tokens.js";
import { listEvents } from "../src/store/queries.js";
import { HUMAN, cleanupDir, makeDeps, tmpStateDir, type TestDeps } from "./helpers.js";

let dir: string;
let deps: TestDeps;
let scheduler: Scheduler;
let runner: AppRouteTasks;

afterEach(async () => {
  await runner.settled();
  deps.cleanup();
  cleanupDir(dir);
});

type Call = { op: string; binding: Record<string, unknown>; input: Record<string, unknown> };

function fakeHost(opts: { reply?: string; messageError?: { code: string; humanMessage: string }; snapshots?: string[] } = {}) {
  const calls: Call[] = [];
  let contexts = 0;
  let snapshots = [...(opts.snapshots ?? [])];
  let release: () => void = () => {};
  const gate = new Promise<void>((r) => (release = r));
  const host: AppTurnHost = {
    async call(op, binding, input) {
      calls.push({ op, binding, input });
      if (op === "agent.list") return { ok: true, value: [{ agentId: "chatgpt-phone" }] };
      if (op === "agent.context.open") return { ok: true, value: { contextId: `ctx-${++contexts}` } };
      if (op === "agent.events") {
        const events = snapshots.map((text) => ({ type: "agent.message.delta", correlation_id: input.correlationId ?? currentTask, payload: { text } }));
        snapshots = [];
        if (events.length === 0) release();
        return { ok: true, value: { events, cursor: "c1" } };
      }
      if (op === "agent.context.message") {
        currentTask = input.correlationId as string;
        if (opts.snapshots) await gate;
        if (opts.messageError) return { ok: false, error: opts.messageError };
        return { ok: true, value: { reply: opts.reply } };
      }
      if (op === "agent.context.cancel") return { ok: true, value: { confirmed: true } };
      return { ok: false, error: { code: "UNSUPPORTED", humanMessage: op } };
    },
  };
  let currentTask = "";
  return { host, calls };
}

function director(route: string | null, adapterId: string | null = "chatgpt-android-app"): TurnDirector & { asked: string[] } {
  const asked: string[] = [];
  return {
    asked,
    async routeForTurn(vendor) {
      asked.push(vendor);
      if (route === null) return { ok: false, error: "NO_ROUTE", reason: "nothing eligible" };
      return { ok: true, route, adapterId, rebound: false, reason: `bound ${route}` };
    },
  };
}

function setup(host: AppTurnHost, dir_: TurnDirector) {
  dir = tmpStateDir();
  scheduler = createScheduler();
  deps = makeDeps(dir, { scheduler, fetchImpl: (async () => new Response("{}", { status: 200 })) as typeof fetch, appRouteTasks: (db, log) => (runner = new AppRouteTasks({ db, log, host, director: dir_, pollMs: 5 })) });
  return issueToken(deps.db, "gizzi", "test", ["tasks:submit", "tasks:read"]).token;
}

async function untilTerminal(t: string, id: string) {
  for (let i = 0; i < 200; i++) {
    const r = await request(deps.app).get(`/v1/tasks/${id}`).set("authorization", `Bearer ${t}`);
    if (["completed", "failed", "cancelled"].includes(r.body.status)) return r.body;
    await new Promise((r) => setTimeout(r, 10));
  }
  throw new Error("task never finished");
}

const submit = (t: string, body: Record<string, unknown>, headers: Record<string, string> = {}) => {
  let r = request(deps.app).post("/v1/tasks").set("authorization", `Bearer ${t}`);
  for (const [k, v] of Object.entries(headers)) r = r.set(k, v);
  return r.send({ initiated_by: HUMAN, capability: "chat.create", prompt: "hello", routing: { provider: "chatgpt" }, ...body });
};

describe("POST /v1/tasks on the RouteDirector", () => {
  it("runs a chat on the phone route through its AAI adapter and completes like a worker task", async () => {
    const { host, calls } = fakeHost({ reply: "Hi there" });
    const d = director("android-app");
    const t = setup(host, d);
    const res = await submit(t, {});
    expect(res.status).toBe(201);
    expect(scheduler.size()).toBe(0);
    expect(d.asked).toEqual(["chatgpt"]);
    const done = await untilTerminal(t, res.body.task_id);
    expect(done.status).toBe("completed");
    expect(done.result).toEqual({ artifact_ids: [], text: "Hi there" });
    // The host must not pick a route again: the binding carries the adapter, no vendor.
    const message = calls.find((c) => c.op === "agent.context.message")!;
    expect(message.binding.adapterId).toBe("chatgpt-android-app");
    expect(message.binding.vendor).toBeUndefined();
    expect(message.input).toMatchObject({ text: "hello", correlationId: res.body.task_id });
    const kinds = listEvents(deps.db, res.body.task_id).map((e) => e.kind);
    expect(kinds).toContain("task.created");
    expect(kinds.filter((k) => k === "task.status")).toHaveLength(2); // running, completed
    expect(kinds).toContain("reply");
  });

  it("streams the app's reply snapshots as reply.text.delta growth", async () => {
    const { host } = fakeHost({ reply: "Hello world", snapshots: ["Hel", "Hello", "Hello world"] });
    const t = setup(host, director("iphone-app-ictrl", "chatgpt-iphone-app"));
    const res = await submit(t, {});
    await untilTerminal(t, res.body.task_id);
    const deltas = listEvents(deps.db, res.body.task_id)
      .filter((e) => e.kind === "reply")
      .map((e) => (e.payload as { event: { type: string; delta?: string } }).event)
      .filter((e) => e.type === "reply.text.delta")
      .map((e) => e.delta);
    expect(deltas.join("")).toBe("Hello world");
  });

  it("continues a thread in the same app conversation", async () => {
    const { host, calls } = fakeHost({ reply: "ok" });
    const t = setup(host, director("android-app"));
    const first = await submit(t, { thread_id: "th-1" });
    await untilTerminal(t, first.body.task_id);
    const second = await submit(t, { capability: "chat.continue", thread_id: "th-1" });
    expect(second.status).toBe(201);
    await untilTerminal(t, second.body.task_id);
    expect(calls.filter((c) => c.op === "agent.context.open")).toHaveLength(1);
    expect(calls.filter((c) => c.op === "agent.context.message").map((c) => c.input.contextId)).toEqual(["ctx-1", "ctx-1"]);
  });

  it("remembers the thread's app conversation across a gateway restart", async () => {
    const { host, calls } = fakeHost({ reply: "ok" });
    const t = setup(host, director("android-app"));
    const first = await submit(t, { thread_id: "th-r" });
    await untilTerminal(t, first.body.task_id);
    // A new runner on the same database: what a restarted gateway builds.
    const restarted = new AppRouteTasks({ db: deps.db, log: deps.log, host, director: director("android-app"), pollMs: 5 });
    expect(restarted.holdsThread("th-r")).toBe(true);
    expect(restarted.holdsThread("th-other")).toBe(false);
    expect(calls.filter((c) => c.op === "agent.context.open")).toHaveLength(1);
  });

  it("maps an adapter error to a failed task with a typed error", async () => {
    const { host } = fakeHost({ messageError: { code: "ADAPTER_DRIFT", humanMessage: "ChatGPT's composer on your phone wasn't found." } });
    const t = setup(host, director("android-app"));
    const res = await submit(t, {});
    const done = await untilTerminal(t, res.body.task_id);
    expect(done.status).toBe("failed");
    expect(done.error).toMatchObject({ class: "provider_ui_changed", fallback_eligible: true, detail: "ChatGPT's composer on your phone wasn't found." });
  });

  it("a reply that never finishes fails as a timeout", async () => {
    const { host } = fakeHost({ reply: undefined });
    const t = setup(host, director("desktop-app-ax", "chatgpt-desktop"));
    const res = await submit(t, {});
    const done = await untilTerminal(t, res.body.task_id);
    expect(done.error).toMatchObject({ class: "timeout" });
  });

  it("keeps the web path for a web route, NO_ROUTE, no provider, and turns vendor adapters submit", async () => {
    const { host, calls } = fakeHost({ reply: "x" });
    const web = director("web", "chatgpt-web");
    const t = setup(host, web);
    expect((await submit(t, {})).body.status).toBe("queued");
    expect((await submit(t, { routing: undefined })).body.status).toBe("queued");
    expect((await submit(t, {}, { "x-subs-route-chosen": "1" })).body.status).toBe("queued");
    expect(web.asked).toEqual(["chatgpt"]); // no provider and internal turns never ask
    expect(scheduler.size()).toBe(3);
    expect(calls).toHaveLength(0);
  });

  it("NO_ROUTE falls back to the FabricRouter path, which records its own reasons", async () => {
    const { host, calls } = fakeHost({ reply: "x" });
    const t = setup(host, director(null));
    const res = await submit(t, {});
    expect(res.body.status).toBe("queued");
    expect(calls).toHaveLength(0);
  });

  it("cancel presses Stop on a running app turn and marks it cancelled", async () => {
    const { host, calls } = fakeHost({ reply: "late", snapshots: ["la"] });
    const t = setup(host, director("android-app"));
    const res = await submit(t, {});
    // wait until it is running
    for (let i = 0; i < 100 && !calls.some((c) => c.op === "agent.context.message"); i++) await new Promise((r) => setTimeout(r, 5));
    const cancel = await request(deps.app).post(`/v1/tasks/${res.body.task_id}/cancel`).set("authorization", `Bearer ${t}`).send({});
    expect(cancel.status).toBe(200);
    expect(cancel.body.status).toBe("cancelled");
    expect(calls.some((c) => c.op === "agent.context.cancel")).toBe(true);
    const done = await untilTerminal(t, res.body.task_id);
    expect(done.status).toBe("cancelled");
  });
});
