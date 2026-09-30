import { describe, expect, test } from "bun:test";
import { SystemOne, configFromEnv } from "../src/engine.ts";
import { OpenAICompatRuntime, type ChatRuntime, type CompletionRequest, type CompletionResult } from "../src/backends/runtime.ts";
import { TypeSafeBackend } from "../src/backends/typesafe.ts";
import { createHandler } from "../src/server.ts";
import { routeModel, RouteModelRefused } from "../src/route-model.ts";
import { SystemOneError } from "../src/types.ts";

const lp = Math.log;

/** Mock runtime: answers by inspecting the reply instruction in the prompt; records every call. */
class MockRuntime implements ChatRuntime {
  name = "mock";
  model = "mock-3b";
  calls: CompletionRequest[] = [];
  constructor(private opts: { logprobs?: boolean; sampleText?: (i: number) => string } = {}) {}
  async complete(req: CompletionRequest): Promise<CompletionResult> {
    this.calls.push(req);
    const prompt = req.messages.map((m) => m.content).join("\n");
    const usage = { input: 100, output: 1 };
    if (this.opts.logprobs === false || !req.topLogprobs) {
      return { text: this.opts.sampleText?.(this.calls.length) ?? "A", top: null, usage };
    }
    if (prompt.includes("A) Yes")) {
      return { text: "A", top: [{ token: "A", logprob: lp(0.8) }, { token: "B", logprob: lp(0.2) }], usage };
    }
    if (prompt.includes("level number")) {
      return { text: "1", top: [{ token: "1", logprob: lp(0.9) }, { token: "2", logprob: lp(0.1) }], usage };
    }
    if (prompt.includes("group containing")) {
      return { text: "B", top: [{ token: "B", logprob: lp(0.75) }, { token: "A", logprob: lp(0.25) }], usage };
    }
    return { text: "A", top: [{ token: "A", logprob: lp(0.7) }, { token: " B", logprob: lp(0.2) }, { token: "C", logprob: lp(0.1) }], usage };
  }
}

const cfg = { ...configFromEnv({}), typesafeKey: undefined, logEnabled: false };
const req = {
  model: "jev-latest",
  state: "I was charged twice. Please refund the duplicate today.",
  questions: {
    refund: { type: "noul", instructions: "Does the message request a refund?" },
    dept: { type: "choice", instructions: "Which team?", criteria: { billing: "Payments", technical: "Bugs", other: null } },
    urgency: { type: "score", instructions: "How time-sensitive?", criteria: ["No deadline", "Within a week", "Today"] },
  },
};

describe("local backend (mocked runtime)", () => {
  test("answers all three types with the documented shapes", async () => {
    const rt = new MockRuntime();
    const res = await new SystemOne(cfg, { runtime: rt }).evaluate(req);
    expect(res.model).toBe("allternit-local/mock-3b");
    expect(res.answers.refund).toEqual({ type: "noul", noul: 0.8 });
    const dept = res.answers.dept as any;
    expect(dept.type).toBe("choice");
    expect(dept.choice).toBe("billing");
    expect(dept.probabilities).toEqual({ billing: 0.7, technical: 0.2, other: 0.1 });
    expect(dept.confidence).toBeCloseTo((3 * 0.7 - 1) / 2, 4);
    const u = res.answers.urgency as any;
    expect(u.legend).toEqual({ "0": "No deadline", "1": "Within a week", "2": "Today" });
    // "0" unseen: floor = min(min shown 0.1, leftover 0 / 1) = 0 → probs 0 / 0.9 / 0.1
    expect(u.probabilities).toEqual({ "0": 0, "1": 0.9, "2": 0.1 });
    expect(u.score).toBeCloseTo(1.1, 4);
    expect(res.usage).toEqual({ input_tokens: 300, output_tokens: 3 });
    expect(res.x_allternit?.methods).toEqual({ refund: "logprobs", dept: "logprobs", urgency: "logprobs" });
    expect(Object.keys(res.answers)).toEqual(["refund", "dept", "urgency"]);
  });

  test("each question is a separate single-token call; the state is sent verbatim and questions can't see each other", async () => {
    const rt = new MockRuntime();
    await new SystemOne(cfg, { runtime: rt }).evaluate(req);
    expect(rt.calls.length).toBe(3);
    for (const c of rt.calls) {
      expect(c.maxTokens).toBe(1);
      expect(c.temperature).toBe(0);
      expect(c.topLogprobs).toBe(20);
      const user = c.messages[1].content;
      expect(user.startsWith(`STATE:\n<<<\n${req.state}\n>>>`)).toBe(true);
      // question ids are never sent to the model
      expect(user).not.toContain("refund\n");
      expect(user).not.toContain("urgency:");
    }
    const refundPrompt = rt.calls.find((c) => c.messages[1].content.includes("request a refund"))!;
    expect(refundPrompt.messages[1].content).not.toContain("time-sensitive");
  });

  test("no logprobs → k-sample voting, marked method: sampled", async () => {
    const texts = ["A", "A) billing", "B", "A", "nonsense", "A", "C", "A"];
    const rt = new MockRuntime({ logprobs: false, sampleText: (i) => texts[(i - 1) % texts.length] });
    const res = await new SystemOne({ ...cfg, samples: 8 }, { runtime: rt }).evaluate({
      ...req,
      questions: { dept: req.questions.dept },
    });
    expect(res.x_allternit?.methods.dept).toBe("sampled");
    const dept = res.answers.dept as any;
    expect(dept.choice).toBe("billing");
    // 1 logprob attempt + 8 samples
    expect(rt.calls.length).toBe(9);
    expect(rt.calls.slice(1).every((c) => c.temperature === 1 && !c.topLogprobs)).toBe(true);
  });

  test(">20 options → hierarchical group then within-group; probabilities still sum to 1", async () => {
    const rt = new MockRuntime();
    const criteria = Object.fromEntries(Array.from({ length: 45 }, (_, i) => [`opt${i}`, null]));
    const res = await new SystemOne(cfg, { runtime: rt }).evaluate({
      ...req,
      questions: { big: { type: "choice", instructions: "Which?", criteria } },
    });
    const a = res.answers.big as any;
    const sum = Object.values(a.probabilities as Record<string, number>).reduce((x, y) => x + y, 0);
    expect(sum).toBeCloseTo(1, 6);
    expect(a.choice).toBe("opt20"); // group B (0.75) × its option A (0.7)
    expect(rt.calls.length).toBe(1 + 3); // group pick + 3 groups (20, 20, 5)
  });

  test("debias: forward + reversed presentation cancels pure position bias; label mapping follows the option", async () => {
    // A runtime that always prefers whatever is shown first (pure position bias).
    const biased: ChatRuntime & { calls: CompletionRequest[] } = {
      name: "biased", model: "m", calls: [],
      async complete(r) {
        this.calls.push(r);
        const digits = r.messages[1].content.includes("level number");
        const first = digits ? (r.messages[1].content.match(/\n(\d): /)?.[1] ?? "0") : "A";
        const second = digits ? (first === "0" ? "1" : String(Number(first) - 1)) : "B";
        return { text: first, top: [{ token: first, logprob: lp(0.6) }, { token: second, logprob: lp(0.4) }], usage: { input: 1, output: 1 } };
      },
    };
    const res = await new SystemOne({ ...cfg, debias: true }, { runtime: biased }).evaluate({
      ...req,
      questions: { refund: req.questions.refund, urgency: { type: "score", instructions: "x", criteria: ["lo", "hi"] } },
    });
    expect((res.answers.refund as any).noul).toBeCloseTo(0.5, 4);
    expect((res.answers.urgency as any).score).toBeCloseTo(0.5, 4);
    expect(biased.calls.length).toBe(4);
    const rev = biased.calls.find((c) => c.messages[1].content.includes("A) No"));
    expect(rev).toBeTruthy();
  });

  test("unknown model → 422", async () => {
    await expect(new SystemOne(cfg, { runtime: new MockRuntime() }).evaluate({ ...req, model: "gpt-4" })).rejects.toBeInstanceOf(SystemOneError);
  });

  test("typesafe: model without key → 401, never silently remote", async () => {
    try {
      await new SystemOne(cfg, { runtime: new MockRuntime() }).evaluate({ ...req, model: "typesafe:jev-latest" });
      throw new Error("expected 401");
    } catch (e) {
      expect((e as SystemOneError).status).toBe(401);
    }
  });
});

describe("OpenAI-compatible runtime parsing (mocked HTTP)", () => {
  test("reads first-token top_logprobs and usage; sends logprobs params", async () => {
    let sent: any;
    const fetchImpl = async (_u: string, init?: RequestInit) => {
      sent = JSON.parse(String(init!.body));
      return new Response(JSON.stringify({
        choices: [{ message: { content: "B" }, logprobs: { content: [{ token: "B", logprob: -0.1, top_logprobs: [{ token: "B", logprob: -0.1 }, { token: "A", logprob: -2.4 }] }] } }],
        usage: { prompt_tokens: 42, completion_tokens: 1 },
      }));
    };
    const rt = new OpenAICompatRuntime("http://127.0.0.1:11434/v1", "llama3.2:latest", fetchImpl);
    const r = await rt.complete({ messages: [{ role: "user", content: "x" }], maxTokens: 1, temperature: 0, topLogprobs: 20 });
    expect(sent.logprobs).toBe(true);
    expect(sent.top_logprobs).toBe(20);
    expect(r.top).toEqual([{ token: "B", logprob: -0.1 }, { token: "A", logprob: -2.4 }]);
    expect(r.usage).toEqual({ input: 42, output: 1 });
  });
  test("runtime down → 529 overloaded through the engine", async () => {
    const fetchImpl = async () => { throw new Error("ECONNREFUSED"); };
    const eng = new SystemOne(cfg, { fetchImpl });
    try {
      await eng.evaluate(req);
      throw new Error("expected 529");
    } catch (e) {
      expect((e as SystemOneError).status).toBe(529);
    }
  });
});

describe("typesafe passthrough (mocked HTTP; no network)", () => {
  test("sends exactly model/state/questions with bearer auth, strips prefix", async () => {
    let url = "", init: RequestInit | undefined;
    const fetchImpl = async (u: string, i?: RequestInit) => {
      url = u; init = i;
      return new Response(JSON.stringify({ model: "jev-1.13.0", answers: { refund: { type: "noul", noul: 0.95 } }, usage: { input_tokens: 10, output_tokens: 2 } }));
    };
    const b = new TypeSafeBackend("ts_test_key", fetchImpl, "https://api.typesafe.ai/v1/systemone");
    const res = await b.evaluate({ ...req, model: "typesafe:jev-latest" } as any);
    expect(url).toBe("https://api.typesafe.ai/v1/systemone");
    expect((init!.headers as any).authorization).toBe("Bearer ts_test_key");
    const body = JSON.parse(String(init!.body));
    expect(Object.keys(body).sort()).toEqual(["model", "questions", "state"]);
    expect(body.model).toBe("jev-latest");
    expect(body.state).toBe(req.state);
    expect(res.x_allternit?.methods.refund).toBe("remote");
  });
  test("maps upstream 429 to rate_limit_error without echoing the key", async () => {
    const b = new TypeSafeBackend("ts_secret", async () => new Response("slow down", { status: 429 }), "https://x");
    try {
      await b.evaluate(req as any);
      throw new Error("expected throw");
    } catch (e) {
      const err = e as SystemOneError;
      expect(err.status).toBe(429);
      expect(err.body.error.type).toBe("rate_limit_error");
      expect(JSON.stringify(err.body)).not.toContain("ts_secret");
    }
  });
});

describe("HTTP handler", () => {
  const handler = createHandler({ engine: new SystemOne(cfg, { runtime: new MockRuntime() }), token: "" });
  const post = (body: unknown) => handler(new Request("http://127.0.0.1:7717/v1/systemone", { method: "POST", body: JSON.stringify(body) }));
  test("200 on a valid request", async () => {
    const r = await post(req);
    expect(r.status).toBe(200);
    expect((await r.json()).answers.refund.noul).toBe(0.8);
  });
  test("422 with details on a malformed question", async () => {
    const r = await post({ ...req, questions: { q: { type: "score", instructions: "x", criteria: ["one"] } } });
    expect(r.status).toBe(422);
    const b = await r.json();
    expect(b.error.type).toBe("invalid_request_error");
    expect(b.error.details[0].path).toBe("questions.q.criteria");
  });
  test("401 when a token is configured and missing", async () => {
    const h = createHandler({ engine: new SystemOne(cfg, { runtime: new MockRuntime() }), token: "t0k" });
    const r = await h(new Request("http://127.0.0.1:7717/v1/models"));
    expect(r.status).toBe(401);
    expect((await r.json()).error.type).toBe("authentication_error");
  });
  test("429 when in-flight limit is exceeded", async () => {
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    const slow: ChatRuntime = {
      name: "slow", model: "m",
      async complete() { await gate; return { text: "Yes", top: [{ token: "Yes", logprob: 0 }], usage: { input: 1, output: 1 } }; },
    };
    const h = createHandler({ engine: new SystemOne(cfg, { runtime: slow }), token: "", maxInflight: 1 });
    const body = JSON.stringify({ ...req, questions: { refund: req.questions.refund } });
    const first = h(new Request("http://x/v1/systemone", { method: "POST", body }));
    await Bun.sleep(5);
    const second = await h(new Request("http://x/v1/systemone", { method: "POST", body }));
    expect(second.status).toBe(429);
    release();
    expect((await first).status).toBe(200);
  });
  test("GET /v1/models lists local aliases", async () => {
    const r = await handler(new Request("http://127.0.0.1:7717/v1/models"));
    const ids = (await r.json()).data.map((m: any) => m.id);
    expect(ids).toContain("jev-latest");
    expect(ids).toContain("local:mock-3b");
    expect(ids.some((i: string) => i.startsWith("typesafe:"))).toBe(false);
  });
});

describe("route-model helper (mocked HTTP — no paid call)", () => {
  test("refuses without allowPaid", async () => {
    await expect(routeModel({ task: "x", apiKey: "k" })).rejects.toBeInstanceOf(RouteModelRefused);
  });
  test("refuses without a key", async () => {
    await expect(routeModel({ task: "x", apiKey: "", allowPaid: true, fetchImpl: async () => new Response("{}") })).rejects.toBeInstanceOf(RouteModelRefused);
  });
  test("calls typesafe/jev-router and returns the routed model", async () => {
    let sent: any, url = "";
    const fetchImpl = async (u: string, i?: RequestInit) => {
      url = u; sent = JSON.parse(String(i!.body));
      return new Response(JSON.stringify({ model: "anthropic/claude-haiku", choices: [{ message: { content: "ok" } }], usage: { prompt_tokens: 12, completion_tokens: 3 } }));
    };
    const r = await routeModel({ task: "fix a typo", apiKey: "or-key", allowPaid: true, fetchImpl });
    expect(url).toBe("https://openrouter.ai/api/v1/chat/completions");
    expect(sent.model).toBe("typesafe/jev-router");
    expect(r.routed_model).toBe("anthropic/claude-haiku");
    expect(r.usage).toEqual({ input_tokens: 12, output_tokens: 3 });
  });
});
