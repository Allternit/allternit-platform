import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AddressInfo } from "node:net";
import type { Server } from "node:http";
import request from "supertest";
import { issueToken } from "../src/security/tokens.js";
import { closeServer, listenTcp } from "../src/http/server.js";
import { HUMAN,
  cleanupDir,
  makeDeps,
  openSse,
  tmpStateDir,
  type TestDeps,
} from "./helpers.js";

let dir: string;
let deps: TestDeps;
let server: Server | null = null;
let port = 0;

beforeEach(async () => {
  dir = tmpStateDir();
  deps = makeDeps(dir);
  server = await listenTcp(deps.app, "127.0.0.1", 0);
  port = (server.address() as AddressInfo).port;
});

afterEach(async () => {
  if (server) await closeServer(server);
  server = null;
  deps.cleanup();
  cleanupDir(dir);
});

async function waitUntil(fn: () => boolean, timeoutMs = 2000): Promise<void> {
  const start = Date.now();
  while (!fn()) {
    if (Date.now() - start > timeoutMs) throw new Error("waitUntil timed out");
    await new Promise((r) => setTimeout(r, 20));
  }
}

describe("D12 outbox replay gate", () => {
  it("a disconnected caller receives exactly the missed events, exactly once; ack stops further replay", async () => {
    const t = issueToken(deps.db, "bot-1", "bot", ["tasks:submit", "tasks:read"]).token;

    const created = await request(deps.app)
      .post("/v1/tasks")
      .set("authorization", `Bearer ${t}`)
      .send({ initiated_by: HUMAN, capability: "chat.create", prompt: "hello" });
    const taskId = created.body.task_id;

    // connect — replay delivers the missed task.created
    const conn1 = openSse(port, `/v1/tasks/${taskId}/events`, t);
    await conn1.waitFor(1);
    expect(conn1.received[0].event).toBe("task.created");

    // two live events while connected
    deps.log.append({ task_id: taskId, kind: "progress", payload: { label: "e1" }, callers: ["bot-1"] });
    deps.log.append({ task_id: taskId, kind: "progress", payload: { label: "e2" }, callers: ["bot-1"] });
    await conn1.waitFor(3);
    expect(conn1.received.map((e) => e.event)).toEqual(["task.created", "progress", "progress"]);

    // disconnect mid-stream, then two more events fire while away
    conn1.close();
    await waitUntil(() => deps.hub.subscriberCount(taskId) === 0);
    const e3 = deps.log.append({ task_id: taskId, kind: "progress", payload: { label: "e3" }, callers: ["bot-1"] });
    const e4 = deps.log.append({ task_id: taskId, kind: "done", payload: { outcome: "success" }, callers: ["bot-1"] });

    // reconnect — exactly the missed events, exactly once (idempotent on event_id)
    const conn2 = openSse(port, `/v1/tasks/${taskId}/events`, t);
    await conn2.waitFor(2);
    expect(conn2.received).toHaveLength(2);
    expect(conn2.received.map((e) => e.id)).toEqual([e3.event_id, e4.event_id]);

    // ack them, disconnect, reconnect — nothing left to replay
    const ack = await request(deps.app)
      .post("/v1/events/ack")
      .set("authorization", `Bearer ${t}`)
      .send({ event_ids: [e3.event_id, e4.event_id] });
    expect(ack.status).toBe(200);
    expect(ack.body).toEqual({ acked: 2 });

    conn2.close();
    await waitUntil(() => deps.hub.subscriberCount(taskId) === 0);
    const conn3 = openSse(port, `/v1/tasks/${taskId}/events`, t);
    await new Promise((r) => setTimeout(r, 300));
    expect(conn3.received).toHaveLength(0);
    conn3.close();

    // re-ack is a no-op
    const reack = await request(deps.app)
      .post("/v1/events/ack")
      .set("authorization", `Bearer ${t}`)
      .send({ event_ids: [e3.event_id] });
    expect(reack.status).toBe(200);
  });

  it("replay can resume after a cursor event (sinceEventId)", async () => {
    const t = issueToken(deps.db, "bot-1", "bot", ["tasks:submit", "tasks:read"]).token;
    const created = await request(deps.app)
      .post("/v1/tasks")
      .set("authorization", `Bearer ${t}`)
      .send({ initiated_by: HUMAN, capability: "chat.create", prompt: "hello" });
    const taskId = created.body.task_id;
    const e1 = deps.log.append({ task_id: taskId, kind: "progress", payload: { n: 1 }, callers: ["bot-1"] });
    const e2 = deps.log.append({ task_id: taskId, kind: "progress", payload: { n: 2 }, callers: ["bot-1"] });

    const afterCursor = deps.outbox.deliverUndelivered("bot-1", { sinceEventId: e1.event_id });
    expect(afterCursor.map((e) => e.event_id)).toEqual([e2.event_id]);
    expect(deps.outbox.undeliveredCount("bot-1")).toBe(0);
  });

  it("pruneAckedOlderThan removes old acked rows only", async () => {
    const ev = deps.log.append({ task_id: "t1", kind: "done", payload: {}, callers: ["bot-1"] });
    deps.outbox.ack("bot-1", ev.event_id);
    deps.db
      .prepare("UPDATE caller_outbox SET acked_at = ? WHERE event_id = ?")
      .run(new Date(Date.now() - 8 * 24 * 60 * 60 * 1000).toISOString(), ev.event_id);
    const pending = deps.log.append({ task_id: "t1", kind: "progress", payload: {}, callers: ["bot-1"] });

    expect(deps.outbox.pruneAckedOlderThan(7)).toBe(1);
    expect(deps.outbox.undeliveredCount("bot-1")).toBe(1); // pending row survives
    const rows = deps.db
      .prepare("SELECT COUNT(*) AS n FROM caller_outbox WHERE event_id = ?")
      .get(pending.event_id) as { n: number };
    expect(rows.n).toBe(1);
  });
});
