import { Hono } from "hono";
import { cors } from "hono/cors";
import type { Context, Env } from "hono";
import { jwtVerify, createRemoteJWKSet, type JWTPayload } from "jose";
import { encryptWebPush, sanitizeActions, sanitizeData, type NotificationAction } from "./webpush";

interface PushSubscriptionRecord {
  runtimeId: string;
  endpoint: string;
  keys: {
    p256dh: string;
    auth: string;
  };
  label?: string;
  createdAt: number;
}

type NotificationType = "permission" | "question" | "completed" | "error";

interface PendingNotification {
  title: string;
  body: string;
  tag: string;
  type: NotificationType;
  runtimeId: string;
  sessionId?: string;
  /** Extra data for the app's service worker, e.g. `{ kind: 'factory.approval', approvalId, code, actionUrl, openUrl }`. */
  data?: Record<string, string | number | boolean>;
  /** Notification buttons, e.g. Approve / Open. */
  actions?: NotificationAction[];
}

interface WorkerEnv {
  REMOTE_CONTROL_PUSH_KV: KVNamespace;
  VAPID_JWK: string;
  VAPID_PUBLIC_KEY: string;
  NOTIFY_SECRET: string;
  CLERK_JWKS_URL?: string;
  ALLTERNIT_CLOUD_API_URL?: string;
  REMOTE_CONTROL_DASHBOARD_ORIGIN?: string;
}

const SUBSCRIPTION_TTL_SECONDS = 90 * 24 * 60 * 60; // 90 days
const RATE_LIMIT_WINDOW_SECONDS = 60;
const RATE_LIMIT_MAX_PER_RUNTIME = 30;

const DASHBOARD_ORIGINS = [
  // Allternit Mobile (phone PWA); fabrictransport.allternit.com was its old name.
  "https://m.allternit.com",
  "https://ai.allternit.com",
  "https://platform.allternit.com",
];

function getDashboardOrigin(c: Context<{ Bindings: WorkerEnv }>): string {
  return c.env.REMOTE_CONTROL_DASHBOARD_ORIGIN ?? "https://m.allternit.com";
}

function allowedOrigins(origin: string, dashboardOrigin: string): boolean {
  if (origin === dashboardOrigin) return true;
  if (DASHBOARD_ORIGINS.includes(origin)) return true;
  if (origin.startsWith("http://localhost:")) return true;
  if (origin.startsWith("http://127.0.0.1:")) return true;
  return false;
}

async function hashEndpoint(endpoint: string): Promise<string> {
  const encoder = new TextEncoder();
  const digest = await crypto.subtle.digest("SHA-256", encoder.encode(endpoint));
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("")
    .slice(0, 32);
}

let jwksCache: ReturnType<typeof createRemoteJWKSet> | null = null;
let jwksUrlCache: string | null = null;

function getJWKS(jwksUrl: string) {
  if (jwksCache && jwksUrlCache === jwksUrl) return jwksCache;
  jwksUrlCache = jwksUrl;
  jwksCache = createRemoteJWKSet(new URL(jwksUrl), {
    cooldownDuration: 300_000, // 5 min
    cacheMaxAge: 86_400_000, // 24 hours
  });
  return jwksCache;
}

async function verifyClerkSessionToken(
  c: Context<{ Bindings: WorkerEnv }>,
  token: string
): Promise<JWTPayload | null> {
  const jwksUrl = c.env.CLERK_JWKS_URL;
  if (!jwksUrl) return null;
  try {
    const { payload } = await jwtVerify(token, getJWKS(jwksUrl), {
      issuer: undefined, // Clerk session tokens may use varying issuers; rely on signature + expiry
      audience: undefined,
      clockTolerance: 30,
    });
    return payload;
  } catch (err) {
    console.error("Clerk JWT verification failed", err);
    return null;
  }
}

function getBearerToken(c: Context<{ Bindings: WorkerEnv }>): string | null {
  const auth = c.req.header("Authorization") ?? "";
  const match = auth.match(/^Bearer\s+(.+)$/i);
  return match?.[1] ?? null;
}

async function checkNotifyRateLimit(
  kv: KVNamespace,
  runtimeId: string
): Promise<boolean> {
  const key = `ratelimit:notify:${runtimeId}`;
  const now = Math.floor(Date.now() / 1000);
  const windowStart = Math.floor(now / RATE_LIMIT_WINDOW_SECONDS) * RATE_LIMIT_WINDOW_SECONDS;
  const windowKey = `${key}:${windowStart}`;

  const current = await kv.get(windowKey);
  const count = current ? parseInt(current, 10) : 0;
  if (count >= RATE_LIMIT_MAX_PER_RUNTIME) {
    return false;
  }
  await kv.put(windowKey, String(count + 1), { expirationTtl: RATE_LIMIT_WINDOW_SECONDS + 1 });
  return true;
}

interface VerifiedDevice {
  runtimeId: string;
  userId: string;
  name?: string;
  status?: string;
}

async function verifyDeviceToken(
  c: Context<{ Bindings: WorkerEnv }>,
  token: string
): Promise<VerifiedDevice | null> {
  const cloudUrl = c.env.ALLTERNIT_CLOUD_API_URL;
  if (!cloudUrl) return null;
  try {
    const response = await fetch(`${cloudUrl.replace(/\/$/, "")}/api/v1/runtime-devices/verify-token`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
    });
    if (!response.ok) return null;
    return (await response.json()) as VerifiedDevice;
  } catch (err) {
    console.error("Device token verification failed", err);
    return null;
  }
}

async function verifyUserOwnsRuntime(
  c: Context<{ Bindings: WorkerEnv }>,
  clerkToken: string,
  runtimeId: string
): Promise<boolean> {
  const cloudUrl = c.env.ALLTERNIT_CLOUD_API_URL;
  if (!cloudUrl) {
    // When no cloud API is configured (local dev), accept the subscription
    // without ownership proof. Production deployments must set the cloud URL.
    return true;
  }
  try {
    const response = await fetch(`${cloudUrl.replace(/\/$/, "")}/api/v1/runtime-devices`, {
      headers: {
        Authorization: `Bearer ${clerkToken}`,
        Accept: "application/json",
      },
    });
    if (!response.ok) return false;
    const data = (await response.json()) as { runtimes?: Array<{ id: string }> } | Array<{ id: string }>;
    const devices = Array.isArray(data) ? data : data.runtimes ?? [];
    return devices.some((d) => d.id === runtimeId);
  } catch (err) {
    console.error("Runtime ownership check failed", err);
    return false;
  }
}

async function importVapidPrivateKey(jwkJson: string): Promise<CryptoKey> {
  const jwk = JSON.parse(jwkJson);
  if (jwk.kty !== "EC" || jwk.crv !== "P-256") {
    throw new Error("VAPID_JWK must be a P-256 EC JWK");
  }
  return crypto.subtle.importKey(
    "jwk",
    jwk,
    { name: "ECDSA", namedCurve: "P-256" },
    false,
    ["sign"]
  );
}

function base64UrlEncode(buffer: ArrayBuffer | ArrayBufferView): string {
  const bytes =
    buffer instanceof ArrayBuffer ? new Uint8Array(buffer) : new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength);
  let binary = "";
  for (let i = 0; i < bytes.byteLength; i++) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=/g, "");
}

async function signVapidJWT(
  privateKey: CryptoKey,
  publicKey: string,
  audience: string,
  subject: string
): Promise<string> {
  const now = Math.floor(Date.now() / 1000);
  const header = { typ: "JWT", alg: "ES256" };
  const payload = {
    aud: audience,
    exp: now + 12 * 60 * 60,
    sub: subject,
    iat: now,
  };

  const encodedHeader = base64UrlEncode(new TextEncoder().encode(JSON.stringify(header)));
  const encodedPayload = base64UrlEncode(new TextEncoder().encode(JSON.stringify(payload)));
  const signingInput = `${encodedHeader}.${encodedPayload}`;

  const signature = await crypto.subtle.sign(
    { name: "ECDSA", hash: "SHA-256" },
    privateKey,
    new TextEncoder().encode(signingInput)
  );
  const encodedSignature = base64UrlEncode(signature);

  return `${signingInput}.${encodedSignature}`;
}

async function sendPushNotification(
  subscription: PushSubscriptionRecord,
  privateKey: CryptoKey,
  publicKey: string,
  payload: PendingNotification
): Promise<Response> {
  const audience = new URL(subscription.endpoint).origin;
  const jwt = await signVapidJWT(
    privateKey,
    publicKey,
    audience,
    "mailto:remote-control@allternit.com"
  );

  const message = JSON.stringify({
    title: payload.title,
    body: payload.body,
    tag: payload.tag,
    type: payload.type,
    runtimeId: subscription.runtimeId,
    sessionId: payload.sessionId,
    data: {
      ...(payload.data ?? {}),
      runtimeId: subscription.runtimeId,
      type: payload.type,
      sessionId: payload.sessionId,
    },
    ...(payload.actions ? { actions: payload.actions } : {}),
  });
  // Push services only deliver payloads encrypted to the subscription (RFC 8291).
  const body = await encryptWebPush(new TextEncoder().encode(message), subscription.keys.p256dh, subscription.keys.auth);

  return fetch(subscription.endpoint, {
    method: "POST",
    headers: {
      "Content-Type": "application/octet-stream",
      "Content-Encoding": "aes128gcm",
      TTL: "60",
      Urgency: "high",
      Authorization: `vapid t=${jwt}, k=${publicKey}`,
    },
    body,
  });
}

const app = new Hono<{ Bindings: WorkerEnv }>();

app.use("*", async (c, next) => {
  const dashboardOrigin = getDashboardOrigin(c);
  return cors({
    origin: (origin) => (allowedOrigins(origin, dashboardOrigin) ? origin : null),
    allowMethods: ["GET", "POST", "OPTIONS"],
    allowHeaders: ["Content-Type", "Authorization"],
    credentials: true,
  })(c, next);
});

app.get("/vapid-public-key", async (c) => {
  const publicKey = c.env.VAPID_PUBLIC_KEY;
  if (!publicKey) {
    return c.text("VAPID public key is not configured", 500);
  }
  return c.text(publicKey);
});

app.post("/subscribe", async (c) => {
  const token = getBearerToken(c);
  if (!token) {
    return c.json({ error: "Authorization required" }, 401);
  }
  const claims = await verifyClerkSessionToken(c, token);
  if (!claims) {
    return c.json({ error: "Invalid or expired session token" }, 401);
  }

  const body = await c.req.json<PushSubscriptionRecord>();
  if (!body.runtimeId || !body.endpoint || !body.keys?.p256dh || !body.keys?.auth) {
    return c.json({ error: "Missing required subscription fields" }, 400);
  }

  const ownsRuntime = await verifyUserOwnsRuntime(c, token, body.runtimeId);
  if (!ownsRuntime) {
    return c.json({ error: "Runtime not found or not owned by this user" }, 403);
  }

  const record: PushSubscriptionRecord = {
    runtimeId: body.runtimeId,
    endpoint: body.endpoint,
    keys: body.keys,
    label: body.label,
    createdAt: Date.now(),
  };

  const keyHash = await hashEndpoint(body.endpoint);
  await c.env.REMOTE_CONTROL_PUSH_KV.put(
    `sub:${body.runtimeId}:${keyHash}`,
    JSON.stringify(record),
    { expirationTtl: SUBSCRIPTION_TTL_SECONDS }
  );
  return c.json({ ok: true, userId: claims.sub });
});

app.post("/unsubscribe", async (c) => {
  const body = await c.req.json<{ runtimeId?: string; endpoint?: string }>();
  if (!body.runtimeId || !body.endpoint) {
    return c.json({ error: "runtimeId and endpoint are required" }, 400);
  }
  const keyHash = await hashEndpoint(body.endpoint);
  await c.env.REMOTE_CONTROL_PUSH_KV.delete(`sub:${body.runtimeId}:${keyHash}`);
  await c.env.REMOTE_CONTROL_PUSH_KV.delete(`pending:${keyHash}`);
  return c.json({ ok: true });
});

app.get("/subscriptions", async (c) => {
  const token = getBearerToken(c);
  if (!token) {
    return c.json({ error: "Authorization required" }, 401);
  }
  const claims = await verifyClerkSessionToken(c, token);
  if (!claims) {
    return c.json({ error: "Invalid or expired session token" }, 401);
  }

  const endpoint = c.req.query("endpoint");
  if (!endpoint) {
    return c.json({ error: "endpoint is required" }, 400);
  }

  // Only return runtimeIds the authenticated user actually owns.
  const cloudUrl = c.env.ALLTERNIT_CLOUD_API_URL;
  let ownedRuntimeIds: Set<string> | undefined;
  if (cloudUrl) {
    const response = await fetch(`${cloudUrl.replace(/\/$/, "")}/api/v1/runtime-devices`, {
      headers: {
        Authorization: `Bearer ${token}`,
        Accept: "application/json",
      },
    });
    if (!response.ok) {
      return c.json({ error: "Ownership check failed" }, 500);
    }
    const data = (await response.json()) as { runtimes?: Array<{ id: string }> } | Array<{ id: string }>;
    const devices = Array.isArray(data) ? data : data.runtimes ?? [];
    ownedRuntimeIds = new Set(devices.map((d) => d.id));
  }

  const runtimeIds: string[] = [];
  const prefix = "sub:";
  const list = await c.env.REMOTE_CONTROL_PUSH_KV.list({ prefix });
  for (const key of list.keys) {
    const raw = await c.env.REMOTE_CONTROL_PUSH_KV.get(key.name);
    if (!raw) continue;
    const record: PushSubscriptionRecord = JSON.parse(raw);
    if (record.endpoint === endpoint && (!ownedRuntimeIds || ownedRuntimeIds.has(record.runtimeId))) {
      runtimeIds.push(record.runtimeId);
    }
  }

  return c.json({ runtimeIds });
});

app.post("/notify", async (c) => {
  const token = getBearerToken(c);
  if (!token) {
    return c.json({ error: "Authorization required" }, 401);
  }

  // Accept either the service secret (production, cloud-api → worker) or a
  // valid paired runtime device token (local dev, gizzi → worker).
  let notifyRuntimeId: string | null = null;
  if (token === c.env.NOTIFY_SECRET) {
    notifyRuntimeId = null; // derive from payload below
  } else {
    const device = await verifyDeviceToken(c, token);
    if (!device) {
      return c.json({ error: "Unauthorized" }, 401);
    }
    notifyRuntimeId = device.runtimeId;
  }

  const body = await c.req.json<{
    runtimeId?: string;
    title?: string;
    body?: string;
    tag?: string;
    sessionId?: string;
    type?: NotificationType;
    data?: unknown;
    actions?: unknown;
  }>();
  if (!body.runtimeId) {
    return c.json({ error: "runtimeId is required" }, 400);
  }

  // When authenticated with a device token, the runtimeId in the payload must
  // match the token's runtime. This prevents a leaked token from spamming
  // another user's machine.
  if (notifyRuntimeId && notifyRuntimeId !== body.runtimeId) {
    return c.json({ error: "Runtime mismatch" }, 403);
  }

  const allowed = await checkNotifyRateLimit(c.env.REMOTE_CONTROL_PUSH_KV, body.runtimeId);
  if (!allowed) {
    return c.json({ error: "Rate limit exceeded" }, 429);
  }

  const vapidJwk = c.env.VAPID_JWK;
  const vapidPublicKey = c.env.VAPID_PUBLIC_KEY;
  if (!vapidJwk || !vapidPublicKey) {
    return c.json({ error: "VAPID is not configured" }, 500);
  }

  const privateKey = await importVapidPrivateKey(vapidJwk);
  const pending: PendingNotification = {
    title: body.title ?? "Allternit Remote Control",
    body: body.body ?? "One of your machines needs input.",
    tag: body.tag ?? `remote-control:${body.sessionId ?? body.runtimeId}`,
    type: body.type ?? "permission",
    runtimeId: body.runtimeId,
    sessionId: body.sessionId,
    data: sanitizeData(body.data),
    actions: sanitizeActions(body.actions),
  };

  const prefix = `sub:${body.runtimeId}:`;
  const list = await c.env.REMOTE_CONTROL_PUSH_KV.list({ prefix });
  const results = await Promise.all(
    list.keys.map(async (key) => {
      const raw = await c.env.REMOTE_CONTROL_PUSH_KV.get(key.name);
      if (!raw) return { ok: false, status: "missing" };
      const subscription: PushSubscriptionRecord = JSON.parse(raw);
      const endpointHash = await hashEndpoint(subscription.endpoint);
      await c.env.REMOTE_CONTROL_PUSH_KV.put(
        `pending:${endpointHash}`,
        JSON.stringify(pending),
        { expirationTtl: 300 }
      );

      try {
        const response = await sendPushNotification(subscription, privateKey, vapidPublicKey, pending);
        if (response.status === 404 || response.status === 410) {
          await c.env.REMOTE_CONTROL_PUSH_KV.delete(key.name);
          return { ok: false, status: response.status };
        }
        return { ok: response.ok, status: response.status };
      } catch {
        return { ok: false, status: "exception" };
      }
    })
  );

  return c.json({ ok: true, delivered: results.filter((r) => r.ok).length, total: results.length });
});

app.get("/pending", async (c) => {
  const endpoint = c.req.query("endpoint");
  if (!endpoint) {
    return c.json({ error: "endpoint is required" }, 400);
  }
  const endpointHash = await hashEndpoint(endpoint);
  const raw = await c.env.REMOTE_CONTROL_PUSH_KV.get(`pending:${endpointHash}`);
  if (!raw) {
    return c.json({ title: "Allternit Remote Control", body: "One of your machines needs input.", tag: "remote-control" });
  }
  await c.env.REMOTE_CONTROL_PUSH_KV.delete(`pending:${endpointHash}`);
  return c.json(JSON.parse(raw));
});

app.get("/health", (c) => c.json({ ok: true }));

export default app;
