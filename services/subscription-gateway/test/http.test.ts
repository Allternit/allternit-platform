import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { statSync } from "node:fs";
import { join } from "node:path";
import request from "supertest";
import { createHash } from "node:crypto";
import { createArtifactStore, type ArtifactSourceContext } from "../src/artifacts/store.js";
import { issueToken } from "../src/security/tokens.js";
import { listenTcp, listenUds, closeServer } from "../src/http/server.js";
import {
  cleanupDir,
  makeDeps,
  tmpStateDir,
  udsRequest,
  type TestDeps,
} from "./helpers.js";
import type { Server } from "node:http";

let dir: string;
let deps: TestDeps;
let server: Server | null = null;

beforeEach(() => {
  dir = tmpStateDir();
  deps = makeDeps(dir);
});

afterEach(async () => {
  if (server) await closeServer(server);
  server = null;
  deps.cleanup();
  cleanupDir(dir);
});

function token(scopes: Parameters<typeof issueToken>[3], caller = "caller-1"): string {
  return issueToken(deps.db, caller, "test", scopes).token;
}

describe("transport guards (§A6.1)", () => {
  it("health is unauthenticated", async () => {
    const res = await request(deps.app).get("/v1/health");
    expect(res.status).toBe(200);
    expect(res.body).toEqual({ ok: true, name: "subscription-gateway", version: "0.1.0-test" });
  });

  it("TCP without token → 401; with valid token → 200", async () => {
    const unauth = await request(deps.app).get("/v1/capabilities");
    expect(unauth.status).toBe(401);
    const authed = await request(deps.app)
      .get("/v1/capabilities")
      .set("authorization", `Bearer ${token(["tasks:read"])}`);
    expect(authed.status).toBe(200);
    expect(authed.body).toEqual([]);
  });

  it("bad Host → 403 (DNS-rebinding defense)", async () => {
    const res = await request(deps.app)
      .get("/v1/health")
      .set("Host", "evil.example.com");
    expect(res.status).toBe(403);
    expect(res.body.error).toBe("forbidden_host");
  });

  it("any Origin with the default (empty) allowlist → 403", async () => {
    const res = await request(deps.app)
      .get("/v1/health")
      .set("Origin", "https://anything.example");
    expect(res.status).toBe(403);
    expect(res.body.error).toBe("forbidden_origin");
  });

  it("never sends CORS *", async () => {
    const res = await request(deps.app).get("/v1/health");
    expect(res.headers["access-control-allow-origin"]).toBeUndefined();
  });
});

describe("UDS transport", () => {
  it("serves over a real unix socket with mode 0600", async () => {
    const sock = join(dir, "gateway.sock");
    server = await listenUds(deps.app, sock);
    expect(statSync(sock).mode & 0o777).toBe(0o600);

    const t = token(["tasks:read"]);
    const res = await udsRequest(sock, { path: "/v1/capabilities", token: t });
    expect(res.status).toBe(200);
    expect(res.body).toEqual([]);
  });

  it("replaces a stale socket file on bind", async () => {
    const sock = join(dir, "gateway.sock");
    server = await listenUds(deps.app, sock);
    await closeServer(server);
    server = await listenUds(deps.app, sock); // stale file present — must not EADDRINUSE
    const res = await udsRequest(sock, { path: "/v1/health" });
    expect(res.status).toBe(200);
  });
});

describe("scope enforcement (§A6.2)", () => {
  it("tasks:read-only token POSTing /v1/tasks → 403", async () => {
    const res = await request(deps.app)
      .post("/v1/tasks")
      .set("authorization", `Bearer ${token(["tasks:read"])}`)
      .send({ capability: "chat.create", prompt: "hi" });
    expect(res.status).toBe(403);
    expect(res.body.error).toBe("forbidden_scope");
  });

  it("artifacts:read is required for artifact metadata", async () => {
    const res = await request(deps.app)
      .get("/v1/artifacts/a-1")
      .set("authorization", `Bearer ${token(["tasks:read"])}`);
    expect(res.status).toBe(403);
  });
});

describe("tasks routes", () => {
  it("POST /v1/tasks persists queued and appends task.created", async () => {
    const t = token(["tasks:submit", "tasks:read"], "bot-1");
    const created = await request(deps.app)
      .post("/v1/tasks")
      .set("authorization", `Bearer ${t}`)
      .send({ capability: "chat.create", prompt: "hello", inputs: [{ type: "text", name: "n", content: "c" }] });
    expect(created.status).toBe(201);
    expect(created.body.status).toBe("queued");
    expect(created.body.requester).toEqual({ kind: "bot", id: "bot-1" });

    const fetched = await request(deps.app)
      .get(`/v1/tasks/${created.body.task_id}`)
      .set("authorization", `Bearer ${t}`);
    expect(fetched.status).toBe(200);
    expect(fetched.body.prompt).toBe("hello");

    expect(deps.outbox.undeliveredCount("bot-1")).toBe(1); // task.created
  });

  it("idempotency_key replays return the existing task", async () => {
    const t = token(["tasks:submit"], "bot-1");
    const body = { capability: "chat.create", prompt: "hello", idempotency_key: "k-1" };
    const first = await request(deps.app).post("/v1/tasks").set("authorization", `Bearer ${t}`).send(body);
    const second = await request(deps.app).post("/v1/tasks").set("authorization", `Bearer ${t}`).send(body);
    expect(first.status).toBe(201);
    expect(second.status).toBe(200);
    expect(second.body.task_id).toBe(first.body.task_id);
  });

  it("invalid body → 400", async () => {
    const res = await request(deps.app)
      .post("/v1/tasks")
      .set("authorization", `Bearer ${token(["tasks:submit"])}`)
      .send({ capability: "not-dot-named", prompt: "" });
    expect(res.status).toBe(400);
  });

  it("cancel from queued works; cancel from running → 409", async () => {
    const t = token(["tasks:submit", "tasks:read"], "bot-1");
    const created = await request(deps.app)
      .post("/v1/tasks")
      .set("authorization", `Bearer ${t}`)
      .send({ capability: "chat.create", prompt: "hello" });
    const id = created.body.task_id;

    const cancelled = await request(deps.app)
      .post(`/v1/tasks/${id}/cancel`)
      .set("authorization", `Bearer ${t}`);
    expect(cancelled.status).toBe(200);
    expect(cancelled.body.status).toBe("cancelled");

    const again = await request(deps.app)
      .post(`/v1/tasks/${id}/cancel`)
      .set("authorization", `Bearer ${t}`);
    expect(again.status).toBe(409);
  });
});

describe("accounts routes", () => {
  it("connect creates an auth_required account and a needs_user entry", async () => {
    const t = token(["accounts:manage"], "admin-1");
    const res = await request(deps.app)
      .post("/v1/accounts")
      .set("authorization", `Bearer ${t}`)
      .send({ provider: "provider-x", label: "Personal" });
    expect(res.status).toBe(201);
    expect(res.body.session_health).toBe("auth_required");

    const status = await request(deps.app)
      .get(`/v1/accounts/${res.body.account_id}/status`)
      .set("authorization", `Bearer ${t}`);
    expect(status.status).toBe(200);
    expect(status.body.session_health).toBe("auth_required");

    expect(deps.outbox.undeliveredCount("admin-1")).toBe(1); // needs_user entry

    const list = await request(deps.app).get("/v1/accounts").set("authorization", `Bearer ${t}`);
    expect(list.body).toHaveLength(1);
  });

  it("account status is readable with tasks:read", async () => {
    const admin = token(["accounts:manage"], "admin-1");
    const created = await request(deps.app)
      .post("/v1/accounts")
      .set("authorization", `Bearer ${admin}`)
      .send({ provider: "provider-x", label: "Personal" });
    const reader = token(["tasks:read"], "bot-1");
    const res = await request(deps.app)
      .get(`/v1/accounts/${created.body.account_id}/status`)
      .set("authorization", `Bearer ${reader}`);
    expect(res.status).toBe(200);
  });
});

describe("artifacts + capabilities", () => {
  it("GET /v1/artifacts/:id → 404 with clear body when absent", async () => {
    const res = await request(deps.app)
      .get("/v1/artifacts/missing")
      .set("authorization", `Bearer ${token(["artifacts:read"])}`);
    expect(res.status).toBe(404);
    expect(res.body).toEqual({ error: "artifact_not_found", artifact_id: "missing" });
  });

  const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 7, 7, 7, 7]);
  async function storedPng(): Promise<string> {
    const source: ArtifactSourceContext = {
      task_id: "task-dl-1",
      attempt_no: 1,
      capability: "image.generate",
      provider: "fixture-web" as ArtifactSourceContext["provider"],
      account_id: "acct-1",
      adapter_id: "fixture-web",
      adapter_version: "0.1.0",
      thread_id: null,
      project_id: null,
      bot_id: null,
      sensitivity: "internal",
    };
    const sink = createArtifactStore(deps.db, { artifactsDir: deps.config.artifactsDir, source });
    const id = await sink.begin(
      { provider: "fixture-web", provider_artifact_id: "p-1", provider_url: "https://x.test/", provider_url_expires_at: null } as never,
      { mime_type: "image/png", format: "png", title: "img" }
    );
    await sink.write(id, PNG);
    await sink.commit(id, {
      data: PNG,
      mime_type: "image/png",
      format: "png",
      sha256: createHash("sha256").update(PNG).digest("hex"),
      size_bytes: PNG.length,
    });
    return id;
  }

  it("GET /v1/artifacts/:id/download streams the bytes as a non-rendering attachment (D6)", async () => {
    const id = await storedPng();
    const res = await request(deps.app)
      .get(`/v1/artifacts/${id}/download`)
      .set("authorization", `Bearer ${token(["artifacts:read"])}`)
      .buffer(true)
      .parse((r, cb) => {
        const chunks: Buffer[] = [];
        r.on("data", (c: Buffer) => chunks.push(c));
        r.on("end", () => cb(null, Buffer.concat(chunks)));
      });
    expect(res.status).toBe(200);
    expect(Buffer.from(res.body as Buffer).equals(Buffer.from(PNG))).toBe(true);
    expect(res.headers["content-type"]).toBe("image/png");
    expect(res.headers["content-disposition"]).toBe(`attachment; filename="${id}.png"`);
    expect(res.headers["x-content-type-options"]).toBe("nosniff");
    expect(res.headers["content-security-policy"]).toContain("sandbox");
    expect(res.headers["x-artifact-sha256"]).toBe(createHash("sha256").update(PNG).digest("hex"));
  });

  it("download needs artifacts:read; unknown → 404", async () => {
    const id = await storedPng();
    const denied = await request(deps.app)
      .get(`/v1/artifacts/${id}/download`)
      .set("authorization", `Bearer ${token(["tasks:read"])}`);
    expect(denied.status).toBe(403);
    const missing = await request(deps.app)
      .get("/v1/artifacts/nope/download")
      .set("authorization", `Bearer ${token(["artifacts:read"])}`);
    expect(missing.status).toBe(404);
  });

  it("GET /v1/capabilities returns [] (P1 smoke behavior)", async () => {
    const res = await request(deps.app)
      .get("/v1/capabilities")
      .set("authorization", `Bearer ${token(["tasks:submit"])}`);
    expect(res.status).toBe(200);
    expect(res.body).toEqual([]);
  });
});
