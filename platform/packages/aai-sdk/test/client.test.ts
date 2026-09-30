import { describe, expect, it } from "vitest";
import { AllternitAgents, ApprovalRequiredError, ConflictError, HumanIntentRequiredError, RateLimitedError } from "../src";

type Call = { url: string; method?: string; headers?: any; body?: any };
function mock(responses: Array<[number, unknown]>) {
  const calls: Call[] = [];
  const fetch = async (url: string, init: any = {}) => {
    calls.push({ url, method: init.method, headers: init.headers, body: init.body ? JSON.parse(init.body) : undefined });
    const [status, body] = responses.shift() ?? [200, {}];
    return { ok: status < 400, status, text: async () => JSON.stringify(body) };
  };
  return { calls, client: new AllternitAgents({ baseUrl: "http://x/", token: "t", fetch }) };
}

describe("AllternitAgents", () => {
  it("creates accounts with camelCase body and bearer auth", async () => {
    const { calls, client } = mock([[201, { account: { id: "a1" } }]]);
    const r = await client.accounts.create({ vendor: "grok", authType: "api_key", displayName: "G" });
    expect(r.account.id).toBe("a1");
    expect(calls[0].url).toBe("http://x/api/v1/gateway/provider-accounts");
    expect(calls[0].body).toEqual({ vendor: "grok", authType: "api_key", displayName: "G" });
    expect(calls[0].headers.authorization).toBe("Bearer t");
  });

  it("maps 428 and 409 and 429 to typed errors", async () => {
    const { client } = mock([
      [428, { error: "needs approval", code: "APPROVAL_REQUIRED", approvalId: "ap1" }],
      [409, { error: "closed", code: "REMOTE_CLOSED" }],
      [429, { error: "slow", code: "RATE_LIMITED", retryAfterMs: 500 }],
    ]);
    const e1 = await client.threads.sendTurn("s", "hi").catch((e) => e);
    expect(e1).toBeInstanceOf(ApprovalRequiredError);
    expect(e1.approvalId).toBe("ap1");
    const e2 = await client.threads.sendTurn("s", "hi").catch((e) => e);
    expect(e2).toBeInstanceOf(ConflictError);
    expect(e2.code).toBe("REMOTE_CLOSED");
    const e3 = await client.threads.sendTurn("s", "hi").catch((e) => e);
    expect(e3).toBeInstanceOf(RateLimitedError);
    expect(e3.retryAfterMs).toBe(500);
  });

  it("refuses to respond to approvals without human intent", () => {
    const { client } = mock([]);
    expect(() => client.approvals.respond("ap1", "approve", { humanIntent: false })).toThrow(HumanIntentRequiredError);
  });

  it("responds as a user when intent is explicit", async () => {
    const { calls, client } = mock([[200, { approvalId: "ap1", state: "approved" }]]);
    await client.approvals.respond("ap1", "approve", { humanIntent: true });
    expect(calls[0].body).toEqual({ decision: "approve", actor: { type: "user" } });
  });

  it("streams events with after cursor, then backs off until idle limit", async () => {
    const ev = (n: number) => ({ id: "e" + n, sequence: n, type: "t", actor: { type: "bot", id: "b" }, payload: null, sessionId: null, occurredAt: "" });
    const { calls, client } = mock([[200, { events: [ev(1), ev(2)] }], [200, { events: [ev(3)] }], [200, { events: [] }], [200, { events: [] }]]);
    const sleeps: number[] = [];
    const got: number[] = [];
    for await (const e of client.threads.streamEvents("th", { maxIdlePolls: 2, sleep: async (ms) => { sleeps.push(ms); } })) got.push(e.sequence);
    expect(got).toEqual([1, 2, 3]);
    expect(calls.map((c) => c.url.split("?")[1])).toEqual(["after=0", "after=2", "after=3", "after=3"]);
    expect(sleeps.length).toBe(1);
  });

  it("retries 5xx while streaming but surfaces 4xx", async () => {
    const { client } = mock([[502, { error: "bad" }], [404, { error: "thread not found" }]]);
    const it = client.threads.streamEvents("th", { sleep: async () => {} });
    await expect(it.next()).rejects.toMatchObject({ status: 404 });
  });

  it("covers vendor packs, bindings, sync", async () => {
    const { calls, client } = mock([[200, {}], [200, {}], [200, {}], [200, {}]]);
    await client.vendorPacks.parity("grok");
    await client.bots.bindExecution("b1", { vendor: "grok", accountBindingId: "a1" });
    await client.threads.sync("t1");
    await client.channels.bind("t1", { provider: "slack", externalConversationId: "c" });
    expect(calls.map((c) => `${c.method} ${c.url.replace("http://x/api/v1", "")}`)).toEqual([
      "GET /gateway/vendor-packs/grok/parity",
      "PUT /gateway/bots/b1/execution-binding",
      "POST /threads/t1/gateway/sync",
      "POST /gateway/threads/t1/channel-bindings",
    ]);
    expect(calls[1].body).toEqual({ vendor: "grok", accountBindingId: "a1" });
  });
});
