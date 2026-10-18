// /v1/phone/handover* relays a sealed Phone Link pairing to Desktop's
// loopback phone gateway on this Sessions computer (routes_phone_handover.ts).
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import request from "supertest";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer as createHttpServer, type Server } from "node:http";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { createServer } from "../src/http/server.js";
import { issueToken } from "../src/security/tokens.js";
import type { PhoneHandoverRelay } from "../src/http/routes_phone_handover.js";
import { DesktopPhoneGateway } from "../src/phone/gateway.js";
import { cleanupDir, makeDeps, tmpStateDir, type TestDeps } from "./helpers.js";

describe("phone handover relay", () => {
  let state: string;
  let deps: TestDeps;
  let calls: Array<{ method: string; path: string; body?: unknown }>;
  let answer: { status: number; body: unknown };
  const relay: PhoneHandoverRelay = {
    request: async (method, path, body) => {
      calls.push({ method, path, body });
      return answer;
    },
  };

  beforeEach(() => {
    state = tmpStateDir();
    deps = makeDeps(state);
    calls = [];
    answer = { status: 200, body: { v: 1, publicKey: "pk", keyId: "kid", owner: null } };
  });
  afterEach(() => { deps.cleanup(); cleanupDir(state); });

  it("needs the Settings scope", async () => {
    const app = createServer({ ...deps, phoneHandover: relay });
    const weak = issueToken(deps.db, "api", "test", ["tasks:read"]).token;
    expect((await request(app).get("/v1/phone/handover/key").set("authorization", `Bearer ${weak}`)).status).toBe(403);
    expect(calls).toHaveLength(0);
  });

  it("relays the key and the sealed pairing unchanged", async () => {
    const app = createServer({ ...deps, phoneHandover: relay });
    const tok = issueToken(deps.db, "api", "test", ["accounts:manage"]).token;
    const key = await request(app).get("/v1/phone/handover/key").set("authorization", `Bearer ${tok}`);
    expect(key.status).toBe(200);
    expect(key.body).toMatchObject({ keyId: "kid" });
    answer = { status: 200, body: { ok: true, deviceId: "moto-1", platform: "android" } };
    const sealed = { v: 1, keyId: "kid", owner: "u", epk: "e", iv: "i", ct: "c", tag: "t" };
    const res = await request(app).post("/v1/phone/handover").set("authorization", `Bearer ${tok}`).send({ sealed });
    expect(res.body).toMatchObject({ ok: true, deviceId: "moto-1" });
    expect(calls[1]).toEqual({ method: "POST", path: "/phone/handover", body: { sealed } });
    expect((await request(app).post("/v1/phone/handover").set("authorization", `Bearer ${tok}`).send({})).status).toBe(400);
  });

  it("answers 503 when Desktop isn't running on this computer", async () => {
    const app = createServer({ ...deps, phoneHandover: relay });
    const tok = issueToken(deps.db, "api", "test", ["accounts:manage"]).token;
    answer = { status: 0, body: { ok: false, error: "desktop_unavailable" } };
    const res = await request(app).get("/v1/phone/handover/key").set("authorization", `Bearer ${tok}`);
    expect(res.status).toBe(503);
    expect(res.body).toMatchObject({ error: "desktop_unavailable" });
  });
});

describe("DesktopPhoneGateway.request", () => {
  let dir: string;
  let server: Server;
  afterEach(() => { server?.close(); rmSync(dir, { recursive: true, force: true }); });

  it("sends the per-launch token from the gateway file", async () => {
    dir = mkdtempSync(join(tmpdir(), "phone-gw-"));
    let seen: string | undefined;
    server = createHttpServer((req, res) => {
      seen = req.headers["x-allternit-phone-token"] as string;
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ keyId: "k" }));
    });
    await new Promise<void>((r) => server.listen(0, "127.0.0.1", () => r()));
    const port = (server.address() as { port: number }).port;
    const file = join(dir, "phone-gateway.json");
    writeFileSync(file, JSON.stringify({ url: `http://127.0.0.1:${port}`, token: "tok-1" }));
    const r = await new DesktopPhoneGateway({ file }).request("GET", "/phone/handover/key");
    expect(r).toEqual({ status: 200, body: { keyId: "k" } });
    expect(seen).toBe("tok-1");
    expect((await new DesktopPhoneGateway({ file: join(dir, "missing.json") }).request("GET", "/x")).status).toBe(0);
  });
});
