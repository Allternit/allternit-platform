// AAI context persistence: contexts outlive a gateway restart (a new AaiHost + fresh providers on the same state.db).
// openclaw/hermes restore their client-side history, event log and cursor; a provider that cannot resume answers a
// clean lost-context error (CONTEXT_NOT_FOUND, details.reason CONTEXT_LOST) instead of "no such conversation".
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { join } from "node:path";
import { MemoryProvider, type AaiProvider, type AaiResult, type AgentCapabilityManifest } from "@allternit/agent-gateway";
import { AaiHost } from "../src/aai/registry.js";
import { AaiContextStore } from "../src/aai/context-store.js";
import { openDatabase, type Db } from "../src/store/db.js";
import { OpenClawProvider } from "../adapters/openclaw/index.js";
import { HermesProvider } from "../adapters/hermes/index.js";
import { startFakeOpenClaw, type FakeOpenClaw } from "../adapters/openclaw/fixtures/fake-server.js";
import { cleanupDir, tmpStateDir } from "./helpers.js";

const val = <T>(r: AaiResult<T>): T => { if (!r.ok) throw new Error(`${r.error.code}: ${r.error.humanMessage}`); return r.value; };
const bindingFor = (adapterId: string, agent: string) => ({ id: "b1", botId: "bot-1", type: "vendor" as const, mode: "hosted" as const, adapterId, externalAgentId: agent, state: "BOUND" as const });

let fake: FakeOpenClaw, hermesFake: FakeOpenClaw;
beforeAll(async () => { [fake, hermesFake] = await Promise.all([startFakeOpenClaw(), startFakeOpenClaw({ models: ["hermes-agent"] })]); });
afterAll(async () => { await Promise.all([fake.close(), hermesFake.close()]); });

let dir: string;
let dbs: Db[];
const open = () => { const db = openDatabase(join(dir, "state.db")); dbs.push(db); return db; };
/** A gateway process: its own db handle, host and provider instances. */
const boot = (make: () => AaiProvider) => new AaiHost([], { contexts: new AaiContextStore(open()) }).register({ provider: make() });
beforeEach(() => { dir = tmpStateDir(); dbs = []; });
afterEach(() => { for (const d of dbs) d.close(); cleanupDir(dir); });

describe.each([
  ["openclaw", "openclaw:openclaw", () => new OpenClawProvider({ baseUrl: fake.url }), () => fake],
  ["hermes", "hermes:hermes-agent", () => new HermesProvider({ baseUrl: hermesFake.url }), () => hermesFake],
] as const)("%s context survives a restart", (adapterId, agent, make, server) => {
  const b = bindingFor(adapterId, agent);

  it("history, event cursor and finished turns come back", async () => {
    const before = boot(make);
    const { contextId } = val(await before.call("agent.context.open", b, { agentId: agent })) as { contextId: string };
    const first = val(await before.call("agent.context.message", b, { contextId, correlationId: "c1", text: "hi" })) as { messageId: string; reply: string };
    expect(first.reply).toBe("echo[1]: hi");
    const read = val(await before.call("agent.events", b, { contextId })) as { events: Array<{ cursor: string }>; cursor: string };
    expect(read.events.length).toBeGreaterThan(0);
    const cursor = read.cursor;

    // Restart: new host, new provider (empty in-memory maps), same state.db.
    const after = boot(make);
    const sentBefore = server().requests.filter((r) => r.path === "/v1/chat/completions").length;
    // A retried correlationId is answered from the stored turn, not sent again.
    const replay = val(await after.call("agent.context.message", b, { contextId, correlationId: "c1", text: "hi" })) as { messageId: string };
    expect(replay.messageId).toBe(first.messageId);
    expect(server().requests.filter((r) => r.path === "/v1/chat/completions").length).toBe(sentBefore);

    const second = val(await after.call("agent.context.message", b, { contextId, correlationId: "c2", text: "again" })) as { reply: string };
    expect(second.reply).toBe("echo[2]: again"); // the first turn was replayed as history
    const last = server().requests.filter((r) => r.path === "/v1/chat/completions").at(-1)!;
    expect(last.body!.messages).toEqual([
      { role: "user", content: "hi" }, { role: "assistant", content: "echo[1]: hi" }, { role: "user", content: "again" },
    ]);
    // Reading on from the cursor held before the restart yields only the new turn's events, with later cursors.
    const more = val(await after.call("agent.events", b, { contextId, cursor })) as { events: Array<{ cursor: string; type: string; payload?: { reply?: string } }> };
    expect(more.events.length).toBeGreaterThan(0);
    expect(more.events.every((e) => Number(e.cursor) > Number(cursor))).toBe(true);
    expect(more.events.find((e) => e.type === "agent.message.completed")?.payload?.reply).toBe("echo[2]: again");
    // Adopting the context by id works after the restart too.
    expect(val(await boot(make).call("agent.context.open", b, { agentId: agent, adoptContextId: contextId }))).toMatchObject({ contextId, resumed: true });
  });

  it("a closed context stays closed after a restart", async () => {
    const before = boot(make);
    const { contextId } = val(await before.call("agent.context.open", b, { agentId: agent })) as { contextId: string };
    val(await before.call("agent.context.close", b, { contextId }));
    const r = await boot(make).call("agent.context.message", b, { contextId, correlationId: "c1", text: "hi" });
    expect(r).toMatchObject({ ok: false, error: { code: "CONTEXT_NOT_FOUND", details: { closed: true } } });
  });
});

/** Stands in for a UI-bridge adapter (grok-bot, chatgpt-dots, claude-desktop): contexts live only in the app's window. */
class NoResumeProvider extends MemoryProvider {
  constructor() { super("ui-agent", 1, {}, "ui-test"); }
  async capabilities(id: string): Promise<AaiResult<AgentCapabilityManifest>> {
    const m = await super.capabilities(id);
    return m.ok ? { ok: true, value: { ...m.value, context: { ...m.value.context, resume: false, isolation: "shared" } } } : m;
  }
}

describe("a provider that cannot resume", () => {
  const b = bindingFor("ui-test", "ui-agent");
  it("answers a restored context with a clean CONTEXT_LOST, and unknown ids as before", async () => {
    const before = boot(() => new NoResumeProvider());
    const { contextId } = val(await before.call("agent.context.open", b, { agentId: "ui-agent" })) as { contextId: string };
    val(await before.call("agent.events", b, { contextId }));
    const after = boot(() => new NoResumeProvider());
    for (const [op, input] of [["agent.context.message", { contextId, correlationId: "c", text: "x" }], ["agent.events", { contextId }]] as const) {
      const r = await after.call(op, b, input);
      expect(r).toMatchObject({ ok: false, error: { code: "CONTEXT_NOT_FOUND", retryable: false, details: { reason: "CONTEXT_LOST", contextId } } });
      expect(!r.ok && r.error.humanMessage).toMatch(/gateway restarted/);
    }
    // An id the gateway never handed out is still the provider's own answer.
    expect(await after.call("agent.events", b, { contextId: "never-seen" })).toMatchObject({ ok: false, error: { code: "CONTEXT_NOT_FOUND", humanMessage: "no such context" } });
    // A new context opens normally after the restart.
    expect((await after.call("agent.context.open", b, { agentId: "ui-agent" })).ok).toBe(true);
  });
});
