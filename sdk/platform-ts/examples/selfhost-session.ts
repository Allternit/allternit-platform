// Run one session on your self-hosted Agents Server and print its events.
//
//   export ALLTERNIT_SERVER_URL=http://127.0.0.1:8400
//   export ALLTERNIT_SERVER_API_KEY=ask_...     # from `allternit-agents-server admin setup`
//   export ALLTERNIT_MODEL=...                  # a model the server accepts (a model_map key on a gateway)
//   node examples/selfhost-session.ts           # Node 22.6+ runs .ts directly
//
// A model-only reply needs no worker. If the agent calls a tool, start a worker
// for the environment (`gizzi worker`) or the session waits in `waiting_for_worker`.
import { SelfHostClient, isTurnEnd } from "../src/index.ts";

const model = process.env.ALLTERNIT_MODEL;
if (!model) throw new Error("Set ALLTERNIT_MODEL to a model your server accepts.");

const client = new SelfHostClient(); // ALLTERNIT_SERVER_URL + ALLTERNIT_SERVER_API_KEY

const info = await client.server.getInfo();
console.log(`server ${info.version}, provider ${info.provider}`);

const agent = await client.agents.create({
  name: "SDK example",
  model,
  instructions: "Answer in one short paragraph.",
});
const environment = await client.environments.create({ name: "sdk-example", network: "none" });
const session = await client.sessions.create({ agent_id: agent.id, environment_id: environment.id, title: "SDK example" });

const sent = await client.sessions.sendMessage(session.id, { content: "In two sentences, what is a work queue?" });

for await (const event of client.sessions.streamEvents(session.id, { afterSeq: sent.seq, until: isTurnEnd })) {
  const data = event.data as Record<string, unknown>;
  if (event.type === "message") console.log(`${data.role}: ${data.content}`);
  else if (event.type === "tool_call") console.log(`tool call ${data.name} (${data.tool_call_id})`);
  else if (event.type === "tool_result") console.log(`tool result ${data.tool_call_id}${data.is_error ? " (error)" : ""}`);
  else if (event.type === "status") console.log(`status ${data.status}${data.reason ? ` (${data.reason})` : ""}`);
  else if (event.type === "error") console.log(`error ${data.code}: ${data.message}`);
}

await client.sessions.archive(session.id);
console.log(`done: session ${session.id} archived`);
