// Offline stand-in for the official SDK client, built from the documented event shapes (recorded.ts). It models the
// documented semantics: user events are echoed with ids, a turn ends with idle(end_turn), "always_ask" tool calls pause
// with idle(requires_action), user.tool_confirmation is only accepted for a pending ask (else 400), interrupt forces idle,
// unknown sessions are 404, and faults throw the SDK's own typed error classes.
import { AuthenticationError, InternalServerError, PermissionDeniedError, RateLimitError } from "@anthropic-ai/sdk";
import { NotFoundError, BadRequestError } from "@anthropic-ai/sdk";
import type { MaAgent, MaClient, MaEvent, MaSendEvent, MaSession } from "../client.js";
import { at, ev } from "./recorded.js";

export type Fault = "rate_limit" | "unavailable" | "unauthorized" | "forbidden";
interface Sess extends MaSession { events: MaEvent[]; status: string }

export class FakeManagedAgents implements MaClient {
  agents = new Map<string, MaAgent>();
  sessions = new Map<string, Sess>();
  files = new Map<string, Array<{ id: string; filename: string }>>();
  fault?: Fault;
  /** every API key a client was built with (for leak checks) */
  readonly apiKey: string;
  calls: string[] = [];
  private n = 0;
  private clock = 100;

  constructor(opts: { apiKey?: string; fault?: Fault } = {}) {
    this.apiKey = opts.apiKey ?? "sk-ant-fake-key";
    this.fault = opts.fault;
    this.agents.set("agent_01", { id: "agent_01", name: "Research Assistant", model: "claude-opus-5-5", archived_at: null });
    this.agents.set("agent_old", { id: "agent_old", name: "Retired", archived_at: at(0) });
  }

  private id(p = "sevt") { return `${p}_${++this.n}`; }
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
    this.sessions.set(id, { id, status, agent: { id: agentId }, archived_at: null, events: [...events] });
    this.n = Math.max(this.n, 1000);
  }
  private t() { return (this.clock += 1); }

  private respond(s: Sess, userText: string) {
    s.events.push(ev.running(this.id(), this.t()));
    if (/run tool/i.test(userText)) {
      const tid = this.id();
      s.events.push(ev.toolUse(tid, this.t(), "bash", { command: "echo hi" }, "ask"));
      s.events.push(ev.idle(this.id(), this.t(), "requires_action", [tid]));
      s.status = "idle";
      return;
    }
    s.events.push(ev.agentMessage(this.id(), this.t(), `Echo: ${userText}`));
    s.events.push(ev.idle(this.id(), this.t(), "end_turn"));
    s.status = "idle";
  }

  beta: MaClient["beta"] = {
    agents: {
      list: (_p) => { const self = this; return (async function* () { self.guard("agents.list"); for (const a of self.agents.values()) yield a; })(); },
      retrieve: async (id) => { this.guard("agents.retrieve"); const a = this.agents.get(id); if (!a) throw new NotFoundError(404, { type: "error" }, "agent not found", new Headers()); return a; },
    },
    sessions: {
      create: async (params) => {
        this.guard("sessions.create");
        const agent = (params.agent as { id: string }).id;
        if (!this.agents.has(agent)) throw new NotFoundError(404, { type: "error" }, "agent not found", new Headers());
        const id = this.id("sesn");
        const s: Sess = { id, status: "idle", agent: { id: agent }, archived_at: null, events: [], ...(params.resources ? { resources: params.resources } : {}) } as Sess;
        this.sessions.set(id, s);
        return s;
      },
      retrieve: async (id) => { this.guard("sessions.retrieve"); const s = this.sess(id); return { ...s, events: undefined } as MaSession; },
      archive: async (id) => { this.guard("sessions.archive"); const s = this.sess(id); s.archived_at = at(this.t()); return s; },
      events: {
        list: (id) => { const self = this; return (async function* () { self.guard("events.list"); for (const e of [...self.sess(id).events]) yield e; })(); },
        send: async (id, { events }: { events: MaSendEvent[] }) => {
          this.guard("events.send");
          const s = this.sess(id);
          const echoes: MaEvent[] = [];
          for (const e of events) {
            if (e.type === "user.message") {
              const text = e.content.map((b) => b.text).join("");
              const echo = ev.userMessage(this.id(), this.t(), text); s.events.push(echo); echoes.push(echo);
              this.respond(s, text);
            } else if (e.type === "user.interrupt") {
              s.status = "idle"; echoes.push({ id: "", type: "user.interrupt" });
            } else {
              const pending = s.events.find((x) => x.id === e.tool_use_id && x.evaluated_permission === "ask");
              const resolved = s.events.some((x) => x.type === "user.tool_confirmation" && x.tool_use_id === e.tool_use_id);
              if (!pending || resolved) throw new BadRequestError(400, { type: "error" }, "tool_use_id is not awaiting confirmation", new Headers());
              const echo = ev.confirmation(this.id(), this.t(), e.tool_use_id, e.result); s.events.push(echo); echoes.push(echo);
              s.events.push(ev.toolResult(this.id(), this.t(), e.tool_use_id), ev.agentMessage(this.id(), this.t(), e.result === "allow" ? "Tool finished." : "Understood, not running it."), ev.idle(this.id(), this.t(), "end_turn"));
              s.status = "idle";
            }
          }
          return { data: echoes };
        },
      },
    },
    files: { list: async ({ scope_id }) => { this.guard("files.list"); this.sess(scope_id); return { data: this.files.get(scope_id) ?? [] }; } },
  };
}
