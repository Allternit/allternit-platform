// P3 phase 2 — adapter registry loading/validation, live /v1/capabilities,
// boot cli-token issuance, artifacts list, account disconnect.
// Provider-agnostic assertions only: no adapter-name literals here.
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import request from "supertest";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { loadAdapterRegistry, defaultAdaptersDir } from "../src/adapters/registry.js";
import { boot, type RunningGateway } from "../src/main.js";
import { issueToken, verifyToken, CLI_TOKEN_ACCOUNT } from "../src/security/tokens.js";
import { openDatabase } from "../src/store/db.js";
import { cleanupDir, fakeKeychain, makeDeps, tmpStateDir, type TestDeps } from "./helpers.js";

let dir: string;

beforeEach(() => {
  dir = tmpStateDir();
});
afterEach(() => {
  cleanupDir(dir);
});

describe("adapter registry", () => {
  it("loads and validates the packaged adapters; exposes a capability view", () => {
    const registry = loadAdapterRegistry(defaultAdaptersDir());
    expect(registry.adapters.length).toBeGreaterThanOrEqual(1);
    const view = registry.capabilities();
    expect(view.length).toBeGreaterThanOrEqual(3);
    for (const entry of view) {
      expect(entry.capability).toMatch(/^[^.]+\.[^.]+$/);
      expect(entry.adapter_id).toBeTruthy();
      expect(entry.pool_id).toBeTruthy();
      expect(["stable", "beta", "disabled"]).toContain(entry.status);
    }
    const first = registry.adapters[0];
    expect(registry.byId(first.manifest.adapter_id)?.manifest).toBe(first.manifest);
  });

  it("missing dir → empty registry; invalid manifest → loud failure", () => {
    expect(loadAdapterRegistry(join(dir, "nope")).capabilities()).toEqual([]);

    const bad = join(dir, "adapters", "broken");
    mkdirSync(bad, { recursive: true });
    writeFileSync(join(bad, "manifest.yaml"), "adapter_id: broken\n");
    expect(() => loadAdapterRegistry(join(dir, "adapters"))).toThrow(/invalid adapter manifest/);
  });
});

describe("boot cli-token (§A6.2)", () => {
  let gateway: RunningGateway | null = null;
  afterEach(async () => {
    if (gateway) await gateway.close();
    gateway = null;
  });

  it("issues a cli-token into the keychain on first boot and reuses it after", async () => {
    const keychain = fakeKeychain();
    gateway = await boot({
      env: { SUBS_GATEWAY_STATE_DIR: dir, SUBS_GATEWAY_ADAPTERS_DIR: join(dir, "no-adapters") },
      keychain,
      logger: () => {},
    });
    const token = keychain.get(CLI_TOKEN_ACCOUNT);
    expect(token).toBeTruthy();
    expect(verifyToken(gateway.db, token!)).not.toBeNull();
    expect(verifyToken(gateway.db, token!)?.caller_id).toBe("cli");
    await gateway.close();

    // second boot over the same state dir: token still verifies, not reissued
    gateway = await boot({
      env: { SUBS_GATEWAY_STATE_DIR: dir, SUBS_GATEWAY_ADAPTERS_DIR: join(dir, "no-adapters") },
      keychain,
      logger: () => {},
    });
    expect(keychain.get(CLI_TOKEN_ACCOUNT)).toBe(token);
  });

  it("reissues when the keychain token no longer verifies (fresh DB)", async () => {
    const keychain = fakeKeychain({ [CLI_TOKEN_ACCOUNT]: "sgw_stale" });
    gateway = await boot({
      env: { SUBS_GATEWAY_STATE_DIR: dir, SUBS_GATEWAY_ADAPTERS_DIR: join(dir, "no-adapters") },
      keychain,
      logger: () => {},
    });
    const token = keychain.get(CLI_TOKEN_ACCOUNT);
    expect(token).not.toBe("sgw_stale");
    expect(verifyToken(gateway.db, token!)).not.toBeNull();
  });
});

describe("phase-2 http surface", () => {
  let deps: TestDeps;
  beforeEach(() => {
    deps = makeDeps(dir, { adapterRegistry: loadAdapterRegistry(defaultAdaptersDir()) });
  });
  afterEach(() => deps.cleanup());

  it("GET /v1/capabilities returns the live registry view (not [])", async () => {
    const t = issueToken(deps.db, "caller-1", "test", ["tasks:read"]).token;
    const res = await request(deps.app)
      .get("/v1/capabilities")
      .set("authorization", `Bearer ${t}`);
    expect(res.status).toBe(200);
    expect(res.body.length).toBeGreaterThanOrEqual(3);
    expect(res.body[0]).toHaveProperty("capability");
    expect(res.body[0]).toHaveProperty("pool_id");
  });

  it("GET /v1/artifacts lists rows; POST /v1/accounts/:id/disconnect disables", async () => {
    const t = issueToken(deps.db, "caller-1", "test", [
      "artifacts:read",
      "accounts:manage",
    ]).token;
    const list = await request(deps.app)
      .get("/v1/artifacts")
      .set("authorization", `Bearer ${t}`);
    expect(list.status).toBe(200);
    expect(list.body).toEqual([]);

    const connect = await request(deps.app)
      .post("/v1/accounts")
      .set("authorization", `Bearer ${t}`)
      .send({ provider: "claude", label: "Fixture account" });
    expect(connect.status).toBe(201);
    const disconnect = await request(deps.app)
      .post(`/v1/accounts/${connect.body.account_id}/disconnect`)
      .set("authorization", `Bearer ${t}`);
    expect(disconnect.status).toBe(200);
    expect(disconnect.body.enabled).toBe(false);
  });

  it("no registry wired → /v1/capabilities stays [] (P1 behavior preserved)", async () => {
    const plain = makeDeps(dir);
    const t = issueToken(plain.db, "caller-1", "test", ["tasks:read"]).token;
    const res = await request(plain.app)
      .get("/v1/capabilities")
      .set("authorization", `Bearer ${t}`);
    expect(res.status).toBe(200);
    expect(res.body).toEqual([]);
    plain.cleanup();
  });
});
