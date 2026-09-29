import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium, type Browser } from "playwright";
import { DeclarativeChatAdapter, type DeclarativeChatConfig } from "@allternit/subscription-adapter-sdk";
import type {
  AdapterEvent,
  CapabilityId,
  ExecutionContext,
  PageLease,
  ProbeResult,
  ProviderId,
  ReconcileResult,
  SelectorResolver,
  SubscriptionAdapter,
  Task,
} from "@allternit/subscription-fabric-contracts";
import type { Config } from "../src/config.js";
import { loadConfig } from "../src/config.js";
import { EventLog } from "../src/events/log.js";
import { Notifier } from "../src/events/notify.js";
import { CallerOutbox } from "../src/events/outbox.js";
import { SseHub } from "../src/events/sse.js";
import { createServer, type GatewayDeps } from "../src/http/server.js";
import type { AdapterRegistry } from "../src/adapters/registry.js";
import type { Scheduler } from "../src/queue/scheduler.js";
import type { WorkerPool } from "../src/worker/pool.js";
import type { LoginBrowser } from "../src/worker/login_browser.js";
import type { AccountsRouterOptions } from "../src/http/routes_accounts.js";
import type { CapabilityRouter } from "@allternit/subscription-fabric-contracts";
import type { KeychainBackend } from "../src/security/keychain.js";
import { openDatabase, type Db } from "../src/store/db.js";

export function tmpStateDir(): string {
  return mkdtempSync(join(tmpdir(), "subs-gw-test-"));
}

export function cleanupDir(dir: string): void {
  rmSync(dir, { recursive: true, force: true });
}

export function fakeKeychain(initial: Record<string, string> = {}): KeychainBackend {
  const items = { ...initial };
  return {
    available: () => true,
    get: (account) => items[account] ?? null,
    set: (account, value) => {
      items[account] = value;
    },
  };
}

export interface TestDeps extends GatewayDeps {
  cleanup(): void;
}

export function makeDeps(
  stateDir: string,
  opts: { fetchImpl?: typeof fetch; scheduler?: Scheduler; adapterRegistry?: AdapterRegistry; router?: CapabilityRouter; pool?: WorkerPool; loginBrowser?: LoginBrowser; accountsOptions?: AccountsRouterOptions } = {}
): TestDeps {
  const config: Config = loadConfig({ SUBS_GATEWAY_STATE_DIR: stateDir });
  const db: Db = openDatabase(":memory:");
  const hub = new SseHub();
  const outbox = new CallerOutbox(db);
  const log = new EventLog(db, hub);
  const notifier = new Notifier({
    apiBase: config.apiBase,
    notificationsDir: join(stateDir, "notifications"),
    fetchImpl: opts.fetchImpl,
    log,
  });
  log.setNotifier(notifier);
  const keychain = fakeKeychain();
  const app = createServer({
    db,
    config,
    keychain,
    log,
    outbox,
    hub,
    notifier,
    router: opts.router,
    scheduler: opts.scheduler,
    adapterRegistry: opts.adapterRegistry,
    pool: opts.pool,
    loginBrowser: opts.loginBrowser,
    accountsOptions: opts.accountsOptions,
    version: "0.1.0-test",
  });
  return {
    db,
    config,
    keychain,
    log,
    outbox,
    hub,
    notifier,
    app,
    cleanup() {
      db.close();
    },
  };
}

// D16 — the human stamp every POST /v1/tasks must carry.
export const HUMAN = { kind: "human", user_id: "user-1", action_id: "action-1" } as const;

export function sampleTask(overrides: Partial<Task> = {}): Task {
  const now = new Date().toISOString();
  return {
    task_id: "task-1",
    idempotency_key: null,
    capability: "chat.create",
    capability_version: 1,
    requester: { kind: "bot", id: "bot-1", bot_id: "bot-1" },
    initiated_by: { ...HUMAN },
    thread_id: null,
    project_id: null,
    parent_task_id: null,
    prompt: "hello",
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
    priority: "normal",
    status: "queued",
    status_detail: null,
    route_decision: null,
    attempts: [],
    result: null,
    error: null,
    created_at: now,
    updated_at: now,
    completed_at: null,
    ...overrides,
  } as Task;
}

export interface UdsResponse {
  status: number;
  body: unknown;
}

export function udsRequest(
  socketPath: string,
  opts: {
    method?: string;
    path: string;
    token?: string;
    headers?: Record<string, string>;
    body?: unknown;
  }
): Promise<UdsResponse> {
  return new Promise((resolve, reject) => {
    const req = http.request(
      {
        socketPath,
        path: opts.path,
        method: opts.method ?? "GET",
        headers: {
          ...(opts.token ? { authorization: `Bearer ${opts.token}` } : {}),
          ...(opts.body !== undefined ? { "content-type": "application/json" } : {}),
          ...opts.headers,
        },
      },
      (res) => {
        let data = "";
        res.on("data", (chunk) => (data += chunk));
        res.on("end", () => {
          let body: unknown = data;
          try {
            body = JSON.parse(data);
          } catch {
            // plain text body
          }
          resolve({ status: res.statusCode ?? 0, body });
        });
      }
    );
    req.on("error", reject);
    if (opts.body !== undefined) req.write(JSON.stringify(opts.body));
    req.end();
  });
}

export interface SseEvent {
  id: string | null;
  event: string;
  data: string;
}

export function parseSseChunk(buffer: string): { events: SseEvent[]; rest: string } {
  const events: SseEvent[] = [];
  const parts = buffer.split("\n\n");
  const rest = parts.pop() ?? "";
  for (const part of parts) {
    const lines = part.split("\n").filter((l) => l.length > 0);
    if (lines.every((l) => l.startsWith(":"))) continue; // heartbeat comment
    const ev: SseEvent = { id: null, event: "message", data: "" };
    for (const line of lines) {
      if (line.startsWith("id: ")) ev.id = line.slice(4);
      else if (line.startsWith("event: ")) ev.event = line.slice(7);
      else if (line.startsWith("data: ")) ev.data = line.slice(6);
    }
    if (ev.data !== "" || ev.id !== null) events.push(ev);
  }
  return { events, rest };
}

export function openSse(
  port: number,
  path: string,
  token: string
): {
  received: SseEvent[];
  close(): void;
  waitFor(count: number, timeoutMs?: number): Promise<SseEvent[]>;
} {
  const received: SseEvent[] = [];
  let buffer = "";
  const req = http.get(
    {
      host: "127.0.0.1",
      port,
      path,
      headers: { authorization: `Bearer ${token}` },
    },
    (res) => {
      res.on("data", (chunk) => {
        buffer += chunk.toString();
        const parsed = parseSseChunk(buffer);
        buffer = parsed.rest;
        received.push(...parsed.events);
      });
    }
  );
  return {
    received,
    close() {
      req.destroy();
    },
    waitFor(count: number, timeoutMs = 3000): Promise<SseEvent[]> {
      const start = Date.now();
      return new Promise((resolve, reject) => {
        const tick = () => {
          if (received.length >= count) return resolve(received);
          if (Date.now() - start > timeoutMs)
            return reject(new Error(`timed out waiting for ${count} SSE events (got ${received.length})`));
          setTimeout(tick, 20);
        };
        tick();
      });
    },
  };
}

// ---------------------------------------------------------------------------
// P3 worker-layer helpers: fixture-web fake adapter (config-only, drives the
// SDK fixture pages) + scripted adapters that never touch a browser.
// ---------------------------------------------------------------------------

export const SDK_FIXTURES_DIR = fileURLToPath(
  new URL("../../../platform/packages/subscription-adapter-sdk/test/fixtures/", import.meta.url)
);

export function sdkFixture(name: string): string {
  return readFileSync(join(SDK_FIXTURES_DIR, name), "utf8");
}

// Bundled chromium first; fall back to installed system Chrome (the SDK test
// pattern — never run `playwright install` on this machine).
export async function launchBrowser(): Promise<Browser> {
  try {
    return await chromium.launch();
  } catch {
    return await chromium.launch({ channel: "chrome" });
  }
}

const FIXTURE_WEB_EXTRA_SELECTORS = `
banner:
  critical: false
  strategies:
    - { css: ".fw-banner" }
challenge:
  critical: false
  strategies:
    - { testid: fw-challenge }
capability:chat:
  critical: false
  strategies:
    - { testid: fw-composer }
`;

export function fixtureWebConfig(
  overrides: Partial<DeclarativeChatConfig> = {}
): DeclarativeChatConfig {
  return {
    manifest: {
      adapter_id: "fixture-web",
      adapter_version: "0.1.0",
      provider: "fixture-web" as ProviderId,
      interface: "ui_bridge_web",
      origins: ["https://fixture-web.test"],
      auth: {
        login_url: "https://fixture-web.test/login",
        logged_in_probe: "logged_in_probe",
      },
      plans: [{ plan_id: "free", label: "Free" }],
      capabilities: [
        {
          id: "chat.create" as CapabilityId,
          min_capability_version: 1,
          plans: ["free"],
          pool_id: "fixture-pool",
          detachable: false,
          export_formats: [],
          status: "stable",
        },
      ],
      pacing: {
        min_action_gap_ms: [1, 2],
        min_task_gap_s: 0,
        max_tasks_per_hour: 1000,
        max_tasks_per_day: 5000,
      },
      selectors_version: "v1",
    },
    selectorsYaml: sdkFixture("selectors.v1.yaml") + FIXTURE_WEB_EXTRA_SELECTORS,
    threadUrlPattern: /^https:\/\/fixture-web\.test\/c\/([\w-]+)/,
    banners: [{ kind: "limit_banner", pattern: /limit reached/i }],
    criticalKeys: ["composer", "send_button", "logged_in_probe"],
    submitFallbackEnter: true,
    completion: { stabilityMs: 150, pollIntervalMs: 25, timeoutMs: 5000 },
    heartbeatIntervalMs: 200,
    stallTimeoutS: 5,
    ...overrides,
  };
}

export function fakeLease(url = "about:blank"): PageLease {
  return {
    lease_id: `lease-${Math.random().toString(36).slice(2)}`,
    url: () => url,
    release: async () => {},
  };
}

export function dummyResolver(): SelectorResolver {
  return {
    resolve: async () => {
      throw new Error("no page wired");
    },
    lastMatchedStrategy: () => null,
  };
}

// Minimal ExecutionContext for reconcile-only paths (watch-page ctx stand-in).
export function dummyCtx(): ExecutionContext {
  return {
    signal: new AbortController().signal,
    page: fakeLease(),
    artifacts: {
      begin: async () => "artifact-x",
      write: async () => {},
      commit: async () => {},
      fail: async () => {},
    },
    pacing: { beforeAction: async () => {}, beforeTask: async () => {} },
    selectors: dummyResolver(),
    log: { debug: () => {}, info: () => {}, warn: () => {}, error: () => {} },
    attempt: {
      attempt_no: 1,
      adapter_id: "fixture-web",
      adapter_version: "0.1.0",
      account_id: "acct-fw-1",
      pool_key: "fixture-web:acct-fw-1:fixture-pool",
      submission_state: "not_sent",
      prompt_fingerprint: "fp",
      provider_thread_id: null,
      requested_model_class: null,
      observed_model: null,
      started_at: new Date().toISOString(),
      ended_at: null,
      outcome: "failed",
      error: null,
    },
    markSubmitted: async () => {
      throw new Error("watch pages are read-only");
    },
  };
}

export interface ScriptedAdapterOpts {
  execute: (task: Task, ctx: ExecutionContext) => AsyncIterable<AdapterEvent>;
  reconcileResult?: ReconcileResult;
  resume?: (ctx: ExecutionContext) => AsyncIterable<AdapterEvent>;
}

export interface ScriptedAdapter extends SubscriptionAdapter {
  submissionCount: number;
  executeCalls: number;
  reconcileCalls: number;
}

// Counting fake adapter: submissionCount proves the never-resubmit rule.
export function scriptedAdapter(opts: ScriptedAdapterOpts): ScriptedAdapter {
  const adapter: ScriptedAdapter = {
    manifest: fixtureWebConfig().manifest,
    submissionCount: 0,
    executeCalls: 0,
    reconcileCalls: 0,
    attach: async () => {},
    detach: async () => {},
    probe: async (): Promise<ProbeResult> => ({
      ok: true,
      checks: [],
      observed_at: new Date().toISOString(),
    }),
    execute(task, ctx) {
      adapter.executeCalls += 1;
      return opts.execute(task, ctx);
    },
    async reconcile() {
      adapter.reconcileCalls += 1;
      return opts.reconcileResult ?? { outcome: "ambiguous", detail: "no scripted outcome" };
    },
  };
  if (opts.resume) {
    const resume = opts.resume;
    adapter.resume = (_token, ctx) => resume(ctx);
  }
  return adapter;
}
