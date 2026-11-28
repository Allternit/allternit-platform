# Allternit Cloud — Donor Handoff to AllternitOS

**Branch:** `session/desktop-cloud-mvp`  
**Base commit:** `68361ae2c7cf65a4ed5d54cad52d15e79bacfd44`  
**Date:** 2026-08-29  
**Donor agent scope:** Cloud-side commercial/product layer. No AllternitOS source code was modified.  
**Architecture v2 docs:** `~/Desktop/AllternitCloud-Architecture/AllternitCloud-Architecture-v2` (MD + HTML + PDF + SVG + PNG + generator script + tracker snapshot).  
**Handoff completion file:** `~/Desktop/allternit-workspace/allternit-session-desktop-cloud-mvp/ALLTERNIT_CLOUD_HANDOFF_COMPLETE.md`

---

## Completion Attestation

**Status:** COMPLETE — 2026-08-30.

The final Allternit Cloud donor handoff architecture document set has been generated and is ready for the AllternitOS integration session.

- **Architecture v2 folder:** `~/Desktop/AllternitCloud-Architecture/AllternitCloud-Architecture-v2`
- **Handoff completion file:** `~/Desktop/allternit-workspace/allternit-session-desktop-cloud-mvp/ALLTERNIT_CLOUD_HANDOFF_COMPLETE.md`
- **Deliverables:** `allternit-cloud-architecture-v2.md`, `.html`, `.pdf`, `.svg`, `.png`, additional diagram SVGs/PNGs, `generate-architecture-v2.py`, `MASTER_TRACKING_snapshot.md`, and `ALLTERNIT_CLOUD_HANDOFF_COMPLETE.md`.
- **Test results:** `cargo check -p allternit-api` clean; `cargo test -p allternit-api --lib` 601 passed, 0 failed.
- **Confirmation:** No competing generic Fabric authority was created.

---

## 1. Exact files changed/created

### Created

- `cmd/allternit-api/src/fabric/os_mapping.rs`
  - JSON-friendly canonical-contract view structs: `NodeCapabilityRecord`, `ResourceClass`, `Offer`, `Placement`, `Assignment`, `UsageEvent`, `ModelRequest`.
  - ID prefix helpers (`node_`, `res_`, `plc_`, `asg_`, `uev_`, `off_`).
  - Conversion functions from Cloud types to canonical OS views.
  - Honest `missing` fields documenting deviations from AllternitOS contracts.

- `cmd/allternit-api/src/fabric/hardening.rs`
  - Stub structs: `OrganizationLimit`, `ProviderHealth`, `ModelGatewayRateLimit`.
  - `OrphanCleanupJob` stub describing stale `provisioning` resource cleanup.

- `cmd/allternit-api/src/fabric/product_lanes.rs`
  - `ManagedInferenceLane`, `ManagedHarnessLane`, `CloudComputerUseLane` structs/enums.
  - `LaneStatus` enum and `current()` constructors that report the real, partial status of each lane.

- `ALLTERNIT_CLOUD_DONOR_HANDOFF.md` (this file)

### Modified

- `cmd/allternit-api/src/fabric/mod.rs`
  - Declared new modules (`os_mapping`, `hardening`, `product_lanes`).
  - Rewrote module-level docs to clarify that this crate is the Cloud commercial supply layer, not the canonical OS control plane.

- `cmd/allternit-api/src/fabric/scheduler.rs`
  - Reframed module docs: this is the **Cloud Supply Optimizer**, not the global AllternitOS resource scheduler.

- `cmd/allternit-api/src/fabric/cost.rs`
  - Reframed module docs as the Cloud Supply Optimizer cost engine.

- `cmd/allternit-api/src/fabric/price_cache.rs`
  - Reframed module docs as the Cloud Supply Optimizer price cache.

- `cmd/allternit-api/src/fabric_model_routes.rs`
  - Made `ResponsesRequest` and `Message` `pub(crate)` so `os_mapping.rs` can build the canonical `ModelRequest` view without changing the external API.

---

## 2. Production hardening completed

Hardening was added as **compile-ready stubs and boundaries**, not as fully wired production automation:

- `OrganizationLimit` — spend-cap/budget structure for org/project scope.
- `ProviderHealth` — health snapshot with circuit-breaker-style `is_blocked()`.
- `ModelGatewayRateLimit` — per-org/per-model rate-limit envelope.
- `OrphanCleanupJob` — description of the reconciliation loop that scans stale `provisioning` resources, releases credit holds, and terminates orphans.

These are Cloud-owned commercial boundaries. They intentionally do **not** duplicate AllternitOS policy/authority services.

---

## 3. Managed Inference status + supported providers/model catalog behavior

**Status:** Partial.

**What works:**
- Model catalog (`fabric/model_catalog.rs`) seeds and persists OpenAI, Together, and Fireworks models.
- Model Gateway (`fabric/model_gateway.rs`) estimates token cost and charges the unified credits ledger.
- `/v1/models` and `/v1/responses` routes exist.
- `/v1/responses` is a deterministic MVP stub: it estimates input tokens from message length, uses `max_tokens` as output tokens, calls `ModelGateway::charge_usage`, and returns a canned response.
- `auto` model selection is supported at the product surface.

**Supported providers:** openai, together, fireworks.

**Supported model profiles (catalog):** GPT-4o mini, GPT-4o, GPT-4.1 mini, GPT-4.1, o3-mini, Llama 3.3 70B / 3.1 8B, DeepSeek R1 Distill Llama 70B, Qwen2.5 72B, Llama 3.1 8B/70B Instruct (Fireworks), Fireworks DeepSeek R1, Fireworks Qwen2.5 72B.

**Missing:** real provider proxy/streaming, OS workload + model intent conversion, OS Resource Scheduler integration, private/local Fabric model workers, response cache, request/response logging, rate-limit enforcement.

---

## 4. Managed Harness status (Gizzi/OpenCode)

**Status:** Partial. VM provisioning for harness runtimes exists via Agent Cloud, but harness runtimes are not yet canonical OS Harness/Workers.

**Supported harnesses:** Gizzi (existing VM/runtime path). OpenCode is **not** a model; it is planned as a harness adapter and is not yet wired.

**Missing:** OS Harness/Worker contract wrapping, OpenCode adapter, canonical capability lease integration, artifact + receipt emission, unified workload budget.

---

## 5. Cloud Computer Use status and VM substrates used

**Status:** Partial. Substrate provisioning exists; capability-native product surface does not.

**Supported substrates:** Incus, Tart, bare VM.

**GPU isolation:** passthrough supported; MIG/vGPU/SR-IOV/time-slice not yet implemented.

**Missing:** canonical Workload/Step mapping, node capability advertisement, `desktop.observe`/`desktop.act`, `browser.*`, `shell.exec`, `file.*`, `app.*` capability surfaces, lease-gated invocation, VM + AI + storage unified metering.

---

## 6. Credits/metering flows and tests

### Flows

1. **Resource/compute metering**
   - Raw usage events inserted into `fabric_usage_events`.
   - `UsageIngestor::process_event` resolves pricing from placement or resource class, creates a `fabric_cost_events` row, and charges the credits ledger.
   - Insufficient credits record the cost event and mark usage processed; reconciliation handles the unpaid charge.

2. **Model inference metering**
   - `/v1/responses` estimates input/output tokens.
   - `ModelGateway::charge_usage` debits the org ledger and records `fabric_model_usage_events`.

3. **Provisioning metering**
   - `Scheduler::schedule` places a credit hold before provisioning.
   - On success, the hold is charged; on failure, the hold is released.

### Tests covering these flows

- `fabric::usage::tests::process_event_creates_cost_event_and_charge`
- `fabric::usage::tests::process_event_uses_resource_class_fallback`
- `fabric::usage::tests::process_event_insufficient_credits_records_cost_but_marks_processed`
- `fabric::model_gateway::tests::charge_usage_records_event`
- `fabric::model_gateway::tests::insufficient_credits_fails`
- `fabric_model_routes::tests::responses_charges_ledger`
- `fabric_model_routes::tests::responses_returns_payment_required_when_insufficient_credits`
- `fabric::scheduler::tests::scheduler_provisions_selected_offer_and_charges_hold`
- `fabric::scheduler::tests::scheduler_releases_hold_on_provision_failure`
- `fabric::scheduler::tests::scheduler_blocks_when_insufficient_credits`

---

## 7. Cloud-to-AllternitOS contract mapping table

| AllternitOS canonical object | Cloud source type(s) | Cloud module | Mapping file | Canonical ID prefix | Notes / deviations |
|---|---|---|---|---|---|
| `NodeCapabilityRecord` | `FabricNodeRecord` + `NodeCapacity` | `node_registry.rs` | `os_mapping.rs::NodeCapabilityRecord` | `node_` | Cloud lacks hardware/software profile, workers, capabilities, topology. |
| `ResourceClass` | `ResourceClass` (Cloud SKU) | `sku.rs` | `os_mapping.rs::ResourceClass` | `res_` | Cloud has vCPU/memory/GPU/pricing; OS eligibility rules missing. |
| `Offer` | `allternit_computer_cloud::fabric::Offer` | `allternit-computer-cloud` | `os_mapping.rs::Offer` | `off_` | Synthetic ID from provider/region/instance. Missing `node_id` and OS resource binding. |
| `Placement` | `FabricPlacementSummary` (+ `FabricResource`) | `resources.rs` | `os_mapping.rs::Placement` | `plc_` | Provider-centric. Missing `node_id`, `offer_id`, `workload_id`. |
| `Assignment` | `FabricNodeAssignment` | `node_registry.rs` | `os_mapping.rs::Assignment` | `asg_` | Missing `workload_id`, `step_id`, `lease_id`. |
| `UsageEvent` | `UsageEvent` / `FabricUsageEvent` | `usage.rs`, `resources.rs` | `os_mapping.rs::UsageEvent` | `uev_` | Time/resource usage. Model token usage tracked separately. Missing `receipt_id`. |
| `ModelRequest` | `ResponsesRequest` + `FabricModelRecord` | `fabric_model_routes.rs`, `model_catalog.rs` | `os_mapping.rs::ModelRequest` | (request id) | Deterministic stub. Missing OS workload/step/lease/execution plan. |

---

## 8. Scheduler boundary: OS Resource Scheduler vs Cloud Supply Optimizer

```text
Workload needs compute
        ↓
AllternitOS Resource Scheduler  ← canonical eligibility, placement, node/resource semantics
        ↓
Cloud Supply Optimizer          ← this crate's scheduler/cost/price_cache
        ↓
RunPod / Vast / private / owned capacity
        ↓
node joins Fabric / advertises resources
        ↓
OS schedules work
        ↓
usage receipt
        ↓
Cloud credits/billing
```

**OS Resource Scheduler owns:** canonical resource eligibility, placement semantics, node/resource topology, lease/worker binding.

**Cloud Supply Optimizer owns:** external supplier selection based on price, availability, reliability, retry/fallback, margin, user budget, SLA, region, and SKU constraints.

The existing `Scheduler` struct was **not renamed** but its module docs and the `fabric/mod.rs` docs now explicitly call it the Cloud Supply Optimizer.

---

## 9. Tests run/results

Commands run:

```bash
cargo check -p allternit-api
cargo test -p allternit-api --lib
```

Results:

- `cargo check -p allternit-api`: **clean** (0 errors, pre-existing warnings only).
- `cargo test -p allternit-api --lib`: **601 passed; 0 failed; 0 ignored**.
  - Base commit had 588 lib tests; the 13 new tests come from the new modules (`os_mapping`, `hardening`, `product_lanes`).

---

## 10. Known blockers and TODOs

### Blockers for full convergence

1. **AllternitOS canonical contracts are not yet imported into this repo.** `os_mapping.rs` defines Cloud-side views; the OS integrator must replace/adopt them with the real canonical types.
2. **Model Gateway is a deterministic stub.** Real provider proxy/streaming and OS workload conversion are blocked on OS canonical model intent / execution planner.
3. **Managed Harness is not wrapped as OS Harness/Worker.** Needs canonical harness contract and OpenCode adapter.
4. **Cloud Computer Use lacks capability-native surfaces.** Needs OS Workload/Step mapping, node capability advertisement, and capability lease gating.
5. **Hardening stubs are not wired to background jobs.** Spend caps, provider health circuits, rate limits, and orphan cleanup need scheduling and persistence.

### TODOs left intentionally

- Wire `OrganizationLimit` to org/project settings and enforce at charge/hold time.
- Implement provider health tracker and retry/fallback policy.
- Implement rate-limit counters for `ModelGatewayRateLimit`.
- Implement `OrphanCleanupJob` as a scheduled worker.
- Add private/local Fabric model worker support to Model Gateway.
- Add streaming, cache, and logging to `/v1/responses`.
- Add MIG/vGPU/SR-IOV/time-sliced GPU policy to Cloud Computer Use.

---

## 11. Exact modules the AllternitOS integrator should reuse/wrap

| Capability | Cloud module/path | Reuse instruction |
|---|---|---|
| Credits ledger | `fabric::credits` | Wrap or move into OS billing authority; ledger logic is proven and tested. |
| Model catalog | `fabric::model_catalog` | Reuse catalog schema; canonical OS should own authoritative model registry. |
| Token-cost billing | `fabric::model_gateway` | Wrap as the commercial billing layer over OS inference execution. |
| Supplier offer discovery/scoring | `fabric::scheduler`, `fabric::cost`, `fabric::price_cache` | Keep as Cloud Supply Optimizer; OS Resource Scheduler calls it for external capacity. |
| Provider adapters | `allternit-computer-cloud` providers (Runpod, Vast, fake, fabric_node) | Reuse adapters; OS scheduler may invoke them through Cloud Supply Optimizer. |
| VM substrates | `allternit-computer-cloud` Incus/Tart/bare-VM provisioning | Reuse as execution boundaries for Cloud Computer Use and Managed Harness. |
| Usage ingestion | `fabric::usage` | Reuse cost-event creation; OS should own canonical receipt/ledger event shape. |
| Node enrollment/heartbeat | `fabric::node_registry` | Reuse DB layer; OS node directory should become authority. |
| OS contract mapping | `fabric::os_mapping` | Review and replace with canonical OS types where they exist. |
| Product lane status | `fabric::product_lanes` | Use as the convergence checklist for inference/harness/computer-use. |

---

## 12. Confirmation: no competing generic Fabric authority was created

**Confirmed.**

- No new `Workload`, `Worker`, `Capability`, `Lease`, `Node`, `Resource`, `Topology`, `Placement`, `Receipt`, `Artifact`, `FabricTransport`, or canonical scheduler semantics were introduced.
- The new modules are purely:
  - Cloud-to-OS **mapping** views (`os_mapping`).
  - Cloud-owned **hardening** boundaries (`hardening`).
  - Product lane **status reporting** (`product_lanes`).
- The existing `Scheduler` was reframed as the **Cloud Supply Optimizer** and its struct names were left unchanged.
- All generic node/resource/placement/identity/usage semantics are mapped to the os contracts, which the platform repo now owns (`spec/Contracts/os/v1`).

---

## 13. Integration instructions for the AllternitOS agent

1. Read `cmd/allternit-api/src/fabric/os_mapping.rs` and compare each struct with the canonical OS object model.
2. Replace Cloud-defined canonical views with real OS types where they exist; keep Cloud DB/storage types unchanged.
3. Import or wrap the credits ledger (`fabric::credits`), model catalog/gateway (`fabric::model_catalog`, `fabric::model_gateway`), and Cloud Supply Optimizer (`fabric::scheduler` + `cost` + `price_cache`) into the OS commercial layer.
4. Use `fabric::product_lanes::current()` status to prioritize remaining convergence gaps.
5. Promote `fabric::hardening` stubs into OS policy/health/reconciliation services, keeping Cloud as the commercial enforcement point.
6. Verify with `cargo test -p allternit-api --lib` and the Cloud-to-OS acceptance loops (C4–C6 in the convergence brief).
