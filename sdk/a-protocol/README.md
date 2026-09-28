# @allternit/a-sdk

Typed client for Allternit's A:// protocol surface on `allternit-api`:
principals, intents, jobs and leases, approvals, delegation rules, the DAG
and principal-scoped memory with grants. Types mirror the Rust sources
(`allternit-cowork-runtime/src/transport.rs`, `rails/fabric_transport_routes.rs`,
`cowork_routes.rs`).

```ts
import { AClient } from "@allternit/a-sdk"

const a = new AClient({ baseUrl: "http://127.0.0.1:8013", token: process.env.ALLTERNIT_TOKEN })
await a.intents.submit({
  version: "1", intent_id: crypto.randomUUID(), workspace: "ws",
  initiator: "a://workspace/ws/principal/al", target: "a://workspace/ws/bot/ledger",
  action: { action_type: "price.compute", description: "Price H100 from unit costs" },
  compute: "local",
})

// A worker principal:
const grant = await a.jobs.claim({ wait_secs: 20 })
if (grant) await a.jobs.complete(grant.job_id, { lease_id: grant.lease_id, lease_generation: grant.lease_generation, success: true })
```
