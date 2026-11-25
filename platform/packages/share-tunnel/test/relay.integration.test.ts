/**
 * Against a real, patched opentunnel-server on loopback (`infra/opentunnel/scripts/local-relay.sh` prints the
 * env). Skipped when OPENTUNNEL_TEST_API is unset.
 */
import { createHmac, randomBytes } from "node:crypto";
import { readFileSync } from "node:fs";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { connect as tlsConnect } from "node:tls";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { createShareTunnel, type ShareGate } from "../src/index.js";

const env = process.env;
const API = env.OPENTUNNEL_TEST_API ?? "";
const run = Boolean(API && env.OPENTUNNEL_TEST_ADMISSION_SECRET && env.OPENTUNNEL_TEST_TLS);

const b64 = (data: Buffer | string) => Buffer.from(data).toString("base64url");
function mint(secret: string, sub: string, lid = "shl_test", exp = Math.floor(Date.now() / 1000) + 600) {
  const payload = b64(JSON.stringify({ sub, lid, exp }));
  return `ota1.${payload}.${b64(createHmac("sha256", secret).update(`ota1.${payload}`).digest())}`;
}
const newName = () => `t${randomBytes(8).toString("hex")}`;

function listen(server: Server): Promise<number> {
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve((server.address() as AddressInfo).port)));
}

function visit(hostname: string): Promise<string> {
  const [host, port] = (env.OPENTUNNEL_TEST_TLS ?? "").split(":");
  return new Promise((resolve, reject) => {
    const socket = tlsConnect({
      host,
      port: Number(port),
      servername: hostname,
      ca: env.OPENTUNNEL_TEST_CA_FILE ? readFileSync(env.OPENTUNNEL_TEST_CA_FILE) : undefined,
    });
    let data = "";
    socket.on("secureConnect", () => socket.write(`GET / HTTP/1.1\r\nHost: ${hostname}\r\nConnection: close\r\n\r\n`));
    socket.on("data", (chunk) => (data += chunk.toString()));
    socket.on("end", () => resolve(data));
    socket.on("error", reject);
  });
}

describe.skipIf(!run)("relay admission + share tunnel (local opentunnel-server)", () => {
  const secret = env.OPENTUNNEL_TEST_ADMISSION_SECRET ?? "";
  let app: Server;
  let cloud: Server;
  let appPort = 0;
  let cloudApi = "";
  let heartbeat = { status: 200, body: { status: "active" } as Record<string, string> };

  beforeAll(async () => {
    app = createServer((_req, res) => res.end("hello from the shared app"));
    appPort = await listen(app);
    cloud = createServer((_req, res) => {
      res.writeHead(heartbeat.status, { "content-type": "application/json" });
      res.end(JSON.stringify(heartbeat.body));
    });
    cloudApi = `http://127.0.0.1:${await listen(cloud)}`;
  });
  afterAll(() => {
    app?.close();
    cloud?.close();
  });

  test("a tunnel create without a valid credential is refused", async () => {
    const bare = await fetch(`${API}/api/tunnel`, { method: "POST", body: "{}" });
    expect(bare.status).toBe(400);
    expect(((await bare.json()) as { message: string }).message).toMatch(/admission refused: credential required/);
    const forged = await fetch(`${API}/api/tunnel`, {
      method: "POST",
      body: "{}",
      headers: { authorization: `Bearer ${mint("not-the-secret", newName())}` },
    });
    expect(forged.status).toBe(400);
    expect(((await forged.json()) as { message: string }).message).toMatch(/bad signature/);
  });

  test("a valid credential names the tunnel and works once", async () => {
    const name = newName();
    const credential = mint(secret, name);
    const first = await fetch(`${API}/admit/${credential}/api/tunnel`, { method: "POST", body: "{}" });
    expect(first.status).toBe(201);
    expect(((await first.json()) as { tunnel: { id: string } }).tunnel.id).toBe(name);
    const replay = await fetch(`${API}/api/tunnel`, {
      method: "POST",
      body: "{}",
      headers: { authorization: `Bearer ${credential}` },
    });
    expect(replay.status).toBe(409);
  });

  test("start serves a local port, admin delete revokes it, heartbeat 410 stops it", async () => {
    heartbeat = { status: 200, body: { status: "active" } };
    const name = newName();
    const gateClosed: string[] = [];
    const gate: ShareGate = {
      open: async () => ({ id: "gate_1", target: `127.0.0.1:${appPort}`, token: "ast_t1", authMode: "token" as const, paused: false }),
      close: async (id) => void gateClosed.push(id),
    };
    const tunnel = createShareTunnel({
      relayApi: API,
      cloudApi,
      getAuthToken: async () => "test-session",
      gate,
      heartbeatMs: 200,
      dataDir: `${env.TMPDIR ?? "/tmp"}/share-tunnel-test-${name}`,
    });
    const share = await tunnel.start({ link: { id: "shl_it", subdomain: name }, port: appPort, admission: mint(secret, name) });
    expect(share.url).toBe(`https://${name}.${env.OPENTUNNEL_TEST_DOMAIN ?? "opentunnel.test"}`);
    expect(await visit(new URL(share.url).hostname)).toContain("hello from the shared app");

    if (env.OPENTUNNEL_TEST_ADMIN_TOKEN) {
      const admin = { authorization: `Bearer ${env.OPENTUNNEL_TEST_ADMIN_TOKEN}` };
      const kill = await fetch(`${API}/api/admin/tunnel/${name}/delete`, { method: "POST", headers: admin });
      expect(kill.status).toBe(204);
      const again = await fetch(`${API}/api/admin/tunnel/${name}/delete`, { method: "POST", headers: admin });
      expect(again.status).toBe(404);
    }
    heartbeat = { status: 410, body: { error: "killed" } };
    await expect(share.stopped).resolves.toBe("killed");
    expect(gateClosed).toEqual(["gate_1"]);
  });
});
