// Every adapter that registers with the AAI host (adapters/<id>/aai.ts, plus loopback) must pass runConformance
// through POST /aai/conformance/:adapterId using its OFFLINE fixtures (adapters/<id>/fixtures/offline.ts).
import { existsSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import request from "supertest";
import { AaiHost } from "../src/aai/registry.js";
import { defaultAdaptersDir } from "../src/adapters/registry.js";
import { issueToken } from "../src/security/tokens.js";
import { createServer } from "../src/http/server.js";
import { cleanupDir, makeDeps, tmpStateDir, type TestDeps } from "./helpers.js";

const dir = defaultAdaptersDir();
const ids = readdirSync(dir).sort().filter((id) => existsSync(join(dir, id, "aai.ts")) || existsSync(join(dir, id, "aai.js")));

let state: string, deps: TestDeps, app: ReturnType<typeof createServer>, tok: string;
const closers: Array<() => Promise<void>> = [];
beforeAll(async () => {
  state = tmpStateDir(); deps = makeDeps(state);
  tok = issueToken(deps.db, "api", "test", ["tasks:submit", "tasks:read"]).token;
  const host = new AaiHost();
  for (const id of ids) {
    const file = ["offline.ts", "offline.js"].map((n) => join(dir, id, "fixtures", n)).find((f) => existsSync(f));
    if (!file) continue; // reported by the guard test below
    const mod = await import(pathToFileURL(file).href);
    const { registration, close } = await mod.createOfflineAaiRegistration();
    host.register(registration);
    if (close) closers.push(close);
  }
  app = createServer({ ...deps, aai: host });
});
afterAll(async () => { await Promise.all(closers.map((c) => c())); deps.cleanup(); cleanupDir(state); });

describe("AAI conformance route (offline fixtures)", () => {
  it("guard: every registering adapter ships adapters/<id>/fixtures/offline.ts", () => {
    const missing = ids.filter((id) => !["offline.ts", "offline.js"].some((n) => existsSync(join(dir, id, "fixtures", n))));
    expect(missing).toEqual([]);
    expect(ids.length).toBeGreaterThanOrEqual(5);
  });
  for (const id of ids) {
    it(`${id} passes conformance via POST /aai/conformance/${id}`, async () => {
      const r = await request(app).post(`/aai/conformance/${id}`).set("Authorization", `Bearer ${tok}`).expect(200);
      const failed = r.body.areas.flatMap((a: any) => a.checks.filter((c: any) => c.status === "fail").map((c: any) => `${a.area}: ${c.name}: ${c.reason}`));
      expect(failed).toEqual([]);
      expect(r.body.ok).toBe(true);
      expect(r.body.summary.pass).toBeGreaterThan(0);
    }, 60_000);
  }
});

// A real boot registers the live providers (no fixtures). `?offline=1` runs the shipped fixtures instead,
// so QUICKSTART step 2 works with no vendor running, and the live registration is never replaced.
describe("AAI conformance route ?offline=1 on a live-shaped host", () => {
  let live: ReturnType<typeof createServer>;
  beforeAll(async () => {
    const { OpenClawProvider } = await import("../adapters/openclaw/index.js");
    const host = new AaiHost();
    host.register({ provider: new OpenClawProvider({ baseUrl: "http://127.0.0.1:9", replyTimeoutMs: 500 }) });
    live = createServer({ ...deps, aai: host });
  });
  it("offline passes although the vendor is unreachable; the live run does not", async () => {
    const off = await request(live).post("/aai/conformance/openclaw?offline=1").set("Authorization", `Bearer ${tok}`).expect(200);
    expect(off.body.ok).toBe(true);
    const on = await request(live).post("/aai/conformance/openclaw").set("Authorization", `Bearer ${tok}`).expect(200);
    expect(on.body.ok).toBe(false);
  }, 60_000);
  it("unknown adapter is 404 in both modes", async () => {
    await request(live).post("/aai/conformance/nope?offline=1").set("Authorization", `Bearer ${tok}`).expect(404);
    await request(live).post("/aai/conformance/nope").set("Authorization", `Bearer ${tok}`).expect(404);
  });
});
