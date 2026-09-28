// P3 deliverable 8 — POST /v1/tasks enqueues into the scheduler (status stays
// queued with zero registered adapters; the static router still no-routes).
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import request from "supertest";
import { createScheduler, type Scheduler } from "../src/queue/scheduler.js";
import { issueToken } from "../src/security/tokens.js";
import { HUMAN, cleanupDir, makeDeps, tmpStateDir, type TestDeps } from "./helpers.js";

let dir: string;
let deps: TestDeps;
let scheduler: Scheduler;

beforeEach(() => {
  dir = tmpStateDir();
  scheduler = createScheduler();
  deps = makeDeps(dir, { scheduler });
});

afterEach(() => {
  deps.cleanup();
  cleanupDir(dir);
});

describe("task enqueue wiring", () => {
  it("POST /v1/tasks lands in the scheduler and stays queued with no adapters", async () => {
    const t = issueToken(deps.db, "caller-1", "test", ["tasks:submit", "tasks:read"]).token;
    const res = await request(deps.app)
      .post("/v1/tasks")
      .set("authorization", `Bearer ${t}`)
      .send({ initiated_by: HUMAN, capability: "chat.create", prompt: "hello", priority: "interactive" });
    expect(res.status).toBe(201);
    expect(res.body.status).toBe("queued");
    expect(scheduler.size()).toBe(1);

    // No adapters registered → unrouted lane; cancel removes it from the queue.
    const cancel = await request(deps.app)
      .post(`/v1/tasks/${res.body.task_id}/cancel`)
      .set("authorization", `Bearer ${t}`);
    expect(cancel.status).toBe(200);
    expect(cancel.body.status).toBe("cancelled");
    expect(scheduler.size()).toBe(0);
  });
});
