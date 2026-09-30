# @allternit/aai-sdk

TypeScript client for the Allternit Agent Interface (AAI) REST surface. Endpoint reference: [docs/gateway/AAI_REST.md](../../../docs/gateway/AAI_REST.md). Python twin: `platform/python/allternit-aai`.

```ts
import { AllternitAgents, ApprovalRequiredError } from "@allternit/aai-sdk";

const aai = new AllternitAgents({ baseUrl: "https://api.allternit.com", token });

const { account } = await aai.accounts.create({ vendor: "grok", authType: "api_key" });
await aai.accounts.setSecret(account.id, apiKey);
const { agents } = await aai.accounts.discoverAgents(account.id);
await aai.bots.bindExecution(botId, { vendor: "grok", accountBindingId: account.id, externalAgentId: agents[0].externalAgentId });

try {
  await aai.threads.sendTurn(sessionId, "delete the report");
} catch (e) {
  if (e instanceof ApprovalRequiredError) console.log("needs a person to approve", e.approvalId); // 428
  else throw e; // ConflictError (409), RateLimitedError (429), AaiHttpError
}

// Cursor-based event stream with idle backoff
for await (const ev of aai.threads.streamEvents(threadId, { after: 0 })) console.log(ev.sequence, ev.type);

// Approvals are answered by a person only; the SDK refuses without explicit intent
await aai.approvals.respond(approvalId, "approve", { humanIntent: true });

await aai.vendorPacks.parity("grok");
```

`fetch` is injectable (`new AllternitAgents({ baseUrl, fetch })`) for tests. Run tests: `pnpm test`.
