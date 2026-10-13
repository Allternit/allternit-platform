// Offline stand-in for the official SDK client, built from the documented event shapes (recorded.ts). It models the
// documented semantics: user events are echoed with ids, a turn ends with idle(end_turn), "always_ask" tool calls pause
// with idle(requires_action), user.tool_confirmation is only accepted for a pending ask (else 400), custom tool calls
// pause until user.custom_tool_result (else 400), interrupt forces idle, unknown sessions are 404, and faults throw the
// SDK's own typed error classes.
// Streaming: `events.stream` behaves like the SSE endpoint: it delivers only events emitted after it opened (no replay),
// is abortable through its controller, and can be told to drop mid-stream (`dropStreamAfter`).
// Manual mode (`manual = true`): sends only echo (user.message queued with processed_at null) and the test drives the
// session with `emit`, re-emitting an id to model the documented queued -> processed second sighting.
import { APIConnectionError, AuthenticationError, InternalServerError, PermissionDeniedError, RateLimitError } from "@anthropic-ai/sdk";
import { NotFoundError, BadRequestError } from "@anthropic-ai/sdk";
import type { MaAgent, MaClient, MaEnvironment, MaEvent, MaEventStream, MaSendEvent, MaSession } from "../client.js";
import { at, ev } from "./recorded.js";

export type Fault = "rate_limit" | "unavailable" | "unauthorized" | "forbidden";
interface Sess extends MaSession { events: MaEvent[]; status: string }
interface Sub { queue: MaEvent[]; wake?: () => void; closed: boolean; dropAfter?: number; delivered: number }

const STATUS: Record<string, string> = {
  "session.status_running": "running", "session.status_idle": "idle", "session.status_terminated": "terminated", "session.status_rescheduled": "rescheduling",
};

export class FakeManagedAgents implements MaClient {
  agents = new Map<string, MaAgent>();
  environments = new Map<string, MaEnvironment>();
  sessions = new Map<string, Sess>();
  files = new Map<string, Array<{ id: string; filename: string }>>();
  fault?: Fault;
  /** sends only echo; the test emits the session's events itself */
  manual = false;
  /** the NEXT opened stream throws APIConnectionError after delivering this many events */
  dropStreamAfter?: number;
  /** every API key a client was built with (for leak checks) */
  readonly apiKey: string;
  calls: string[] = [];
  /** params of every events.list call (to check backfill uses the cursor) */
  listParams: Array<Record<string, unknown> | undefined> = [];
  private subs = new Map<string, Set<Sub>>();
  private streamWaiters: Array<{ id: string; resolve: () => void }> = [];
  private n = 0;
  private clock = 100;

  constructor(opts: { apiKey?: string; fault?: Fault } = {}) {
    this.apiKey = opts.apiKey ?? "sk-ant-fake-key";
    this.fault = opts.fault;
    this.agents.set("agent_01", { id: "agent_01", name: "Research Assistant", model: "claude-opus-5-5", archived_at: null });
    this.agents.set("agent_old", { id: "agent_old", name: "Retired", archived_at: at(0) });
    this.environments.set("env_fake", { id: "env_fake", name: "Fake cloud", archived_at: null });
    this.environments.set("env_old", { id: "env_old", name: "Old", archived_at: at(0) });
  }

  id(p = "sevt") { return `${p}_${++this.n}`; }
  t() { return (this.clock += 1); }
  private guard(name: string) {
    this.calls.push(name);
    switch (this.fault) {
      case "rate_limit": throw new RateLimitError(429, { type: "error" }, "rate limited", new Headers({ "retry-after": "7" }));
      case "unavailable": throw new InternalServerError(529, { type: "error" }, "overloaded", new Headers());
      case "unauthorized": throw new AuthenticationError(401, { type: "error" }, "invalid x-api-key", new Headers());
      case "forbidden": throw new PermissionDeniedError(403, { type: "error" }, "forbidden", new Headers());
    }
  }
  private sess(id: string): Sess {
    const s = this.sessions.get(id);
    if (!s) throw new NotFoundError(404, { type: "error" }, "session not found", new Headers());
    return s;
  }
  /** Seed a session from recorded events (e.g. a run paused for approval). */
  seedSession(id: string, agentId: string, events: MaEvent[], status = "idle"): void {
    this.sessions.set(id, { id, status, agent: { id: agentId }, archived_at: null, created_at: at(0), metadata: {}, title: null, events: [...events] });
    this.n = Math.max(this.n, 1000);
  }

  /** Append (or, for a known id, update in place) a session event and deliver it to every open stream. */
  emit(sessionId: string, e: MaEvent): void {
    const s = this.sess(sessionId);
    const i = s.events.findIndex((x) => x.id === e.id);
    if (i >= 0) s.events[i] = e; else s.events.push(e);
    if (STATUS[e.type]) s.status = STATUS[e.type];
    for (const sub of this.subs.get(sessionId) ?? []) { sub.queue.push(e); sub.wake?.(); }
  }
  /** Resolves once a stream is open on the session (now or later). */
  streamOpen(sessionId: string): Promise<void> {
    if (this.subs.get(sessionId)?.size) return Promise.resolve();
    return new Promise((resolve) => this.streamWaiters.push({ id: sessionId, resolve }));
  }
  openStreams(sessionId: string): number { return this.subs.get(sessionId)?.size ?? 0; }

  private respond(s: Sess, userText: string) {
    this.emit(s.id, ev.running(this.id(), this.t()));
    if (/custom tool/i.test(userText)) {
      const tid = this.id();
      this.emit(s.id, ev.customToolUse(tid, this.t(), "lookup_order", { order: "1234" }));
      this.emit(s.id, ev.idle(this.id(), this.t(), "requires_action", [tid]));
      return;
    }
    if (/run tool/i.test(userText)) {
      const tid = this.id();
      this.emit(s.id, ev.toolUse(tid, this.t(), "bash", { command: "echo hi" }, "ask"));
      this.emit(s.id, ev.idle(this.id(), this.t(), "requires_action", [tid]));
      return;
    }
    this.emit(s.id, ev.agentMessage(this.id(), this.t(), `Echo: ${userText}`));
    this.emit(s.id, ev.idle(this.id(), this.t(), "end_turn"));
  }

  private openStream(id: string): MaEventStream {
    const sub: Sub = { queue: [], closed: false, delivered: 0, dropAfter: this.dropStreamAfter };
    this.dropStreamAfter = undefined;
    const set = this.subs.get(id) ?? new Set<Sub>(); set.add(sub); this.subs.set(id, set);
    for (const w of this.streamWaiters.filter((x) => x.id === id)) w.resolve();
    this.streamWaiters = this.streamWaiters.filter((x) => x.id !== id);
    const controller = new AbortController();
    controller.signal.addEventListener("abort", () => { sub.closed = true; sub.wake?.(); });
    const self = this;
    const iter = (async function* () {
      try {
        for (;;) {
          if (sub.closed) return;
          if (sub.dropAfter !== undefined && sub.delivered >= sub.dropAfter) throw new APIConnectionError({ message: "stream dropped (fake)" });
          const next = sub.queue.shift();
          if (next) { sub.delivered += 1; yield next; continue; }
          await new Promise<void>((r) => { sub.wake = r; });
          sub.wake = undefined;
        }
      } finally { self.subs.get(id)?.delete(sub); }
    })();
    return Object.assign(iter, { controller });
  }

  beta: MaClient["beta"] = {
    agents: {
      list: (_p) => { const self = this; return (async function* () { self.guard("agents.list"); for (const a of self.agents.values()) yield a; })(); },
      retrieve: async (id) => { this.guard("agents.retrieve"); const a = this.agents.get(id); if (!a) throw new NotFoundError(404, { type: "error" }, "agent not found", new Headers()); return a; },
    },
    environments: {
      list: (_p) => { const self = this; return (async function* () { self.guard("environments.list"); for (const e of self.environments.values()) yield e; })(); },
    },
    sessions: {
      create: async (params) => {
        this.guard("sessions.create");
        const agent = (params.agent as { id: string }).id;
        if (!this.agents.has(agent)) throw new NotFoundError(404, { type: "error" }, "agent not found", new Headers());
        const env = this.environments.get(String(params.environment_id));
        if (!env || env.archived_at) throw new NotFoundError(404, { type: "error" }, "environment not found", new Headers());
        const id = this.id("sesn");
        const s: Sess = { id, status: "idle", agent: { id: agent }, archived_at: null, created_at: at(this.t()), title: (params.title as string | undefined) ?? null,
          metadata: (params.metadata as Record<string, string> | undefined) ?? {}, events: [], ...(params.resources ? { resources: params.resources } : {}) } as Sess;
        this.sessions.set(id, s);
        return s;
      },
      retrieve: async (id) => { this.guard("sessions.retrieve"); const s = this.sess(id); return { ...s, events: undefined } as MaSession; },
      list: (p) => {
        const self = this;
        return (async function* () {
          self.guard("sessions.list");
          const all = [...self.sessions.values()].filter((s) => (!p?.agent_id || s.agent?.id === p.agent_id) && (p?.include_archived || !s.archived_at)).reverse();
          for (const s of all) yield { ...s, events: undefined } as MaSession;
        })();
      },
      archive: async (id) => { this.guard("sessions.archive"); const s = this.sess(id); s.archived_at = at(this.t()); return s; },
      events: {
        list: (id, params) => {
          const self = this;
          self.listParams.push(params as Record<string, unknown> | undefined);
          return (async function* () {
            self.guard("events.list");
            const gte = params?.["created_at[gte]"];
            for (const e of [...self.sess(id).events]) if (!gte || e.processed_at == null || e.processed_at >= gte) yield e;
          })();
        },
        stream: async (id) => { this.guard("events.stream"); this.sess(id); return this.openStream(id); },
        send: async (id, { events }: { events: MaSendEvent[] }) => {
          this.guard("events.send");
          const s = this.sess(id);
          const echoes: MaEvent[] = [];
          for (const e of events) {
            if (e.type === "user.message") {
              const text = e.content.map((b) => b.text).join("");
              const echo = ev.userMessage(this.id(), this.t(), text, !this.manual); this.emit(id, echo); echoes.push(echo);
              if (!this.manual) this.respond(s, text);
            } else if (e.type === "user.interrupt") {
              s.status = "idle"; echoes.push({ id: "", type: "user.interrupt" });
            } else if (e.type === "user.custom_tool_result") {
              const pending = s.events.find((x) => x.id === e.custom_tool_use_id && x.type === "agent.custom_tool_use");
              const answered = s.events.some((x) => x.type === "user.custom_tool_result" && x.custom_tool_use_id === e.custom_tool_use_id);
              if (!pending || answered) throw new BadRequestError(400, { type: "error" }, "custom_tool_use_id is not awaiting a result", new Headers());
              const text = (e.content ?? []).map((b) => b.text).join("");
              const echo = ev.customToolResult(this.id(), this.t(), e.custom_tool_use_id, text, e.is_error); this.emit(id, echo); echoes.push(echo);
              if (!this.manual) {
                this.emit(id, ev.running(this.id(), this.t()));
                this.emit(id, ev.agentMessage(this.id(), this.t(), `Tool said: ${text}`));
                this.emit(id, ev.idle(this.id(), this.t(), "end_turn"));
              }
            } else {
              const pending = s.events.find((x) => x.id === e.tool_use_id && x.evaluated_permission === "ask");
              const resolved = s.events.some((x) => x.type === "user.tool_confirmation" && x.tool_use_id === e.tool_use_id);
              if (!pending || resolved) throw new BadRequestError(400, { type: "error" }, "tool_use_id is not awaiting confirmation", new Headers());
              const echo = ev.confirmation(this.id(), this.t(), e.tool_use_id, e.result); this.emit(id, echo); echoes.push(echo);
              this.emit(id, ev.toolResult(this.id(), this.t(), e.tool_use_id));
              this.emit(id, ev.agentMessage(this.id(), this.t(), e.result === "allow" ? "Tool finished." : "Understood, not running it."));
              this.emit(id, ev.idle(this.id(), this.t(), "end_turn"));
            }
          }
          return { data: echoes };
        },
      },
    },
    files: { list: async ({ scope_id }) => { this.guard("files.list"); this.sess(scope_id); return { data: this.files.get(scope_id) ?? [] }; } },
  };
}
