// Wire contract vs allternit-api serde structs (agent_gateway_routes.rs: rename_all = "camelCase").
import { describe, expect, it } from "vitest";
import { AllternitAgents } from "../src";

function mock() {
  const calls: { url: string; method?: string; body?: any }[] = [];
  const fetch = async (url: string, init: any = {}) => {
    calls.push({ url, method: init.method, body: init.body ? JSON.parse(init.body) : undefined });
    return { ok: true, status: 200, text: async () => "{}" };
  };
  return { calls, client: new AllternitAgents({ baseUrl: "http://x", fetch }) };
}

describe("request bodies use the camelCase keys the Rust structs deserialize", () => {
  it("secret", async () => {
    const { calls, client } = mock();
    await client.accounts.setSecret("a1", "sk");
    expect(calls[0]).toMatchObject({ url: "http://x/api/v1/gateway/provider-accounts/a1/secret", body: { apiKey: "sk" } });
  });
  it("execution binding, channel binding, gap, account patch", async () => {
    const { calls, client } = mock();
    await client.bots.bindExecution("b1", { type: "vendor", accountBindingId: "a1", preferredLane: "official", externalAgentId: "x", adapterId: "grok" });
    await client.channels.bind("t1", { provider: "slack", externalConversationId: "C1", readOnly: false, postingIdentityId: "p" });
    await client.vendorPacks.recordGap("grok", { capability: "c", surface: "card", fallbackUsed: true, sampleRef: "s" });
    await client.accounts.update("a1", { displayName: "n", externalAccountId: "e", verifiedAt: "t" });
    expect(calls.map((c) => Object.keys(c.body).sort())).toEqual([
      ["accountBindingId", "adapterId", "externalAgentId", "preferredLane", "type"],
      ["externalConversationId", "postingIdentityId", "provider", "readOnly"],
      ["capability", "fallbackUsed", "sampleRef", "surface"],
      ["displayName", "externalAccountId", "verifiedAt"],
    ]);
  });
});
