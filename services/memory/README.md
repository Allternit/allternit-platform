# Allternit Memory & Data Fabric

**Location**: `services/memory/`  
**Domain**: Layer 4 — Services (Memory & Persistence)

---

## Overview

This directory contains the unified memory and data fabric for the Allternit
platform. The canonical Rust implementation is **`allternit-memory-fabric`**
(`services/memory/data/memory-fabric`), which provides:

- A common [`MemoryProvider`] trait for all memory backends.
- A [`MemoryPlane`] / [`MemoryRouter`] for routing operations across multiple
  backends with fallback.
- An append-only, hash-chained [`HistoryLedger`] for auditability.
- A built-in in-memory provider for testing and simple deployments.

The TypeScript **Memory Agent** (`services/memory/agent/`) remains the primary
long-term memory implementation (SQLite + vector search + local LLM
consolidation). Rust services consume it through the HTTP adapter in the
top-level `services/memory` crate, which implements the fabric's
[`MemoryProvider`] trait.

---

## Layout

```
services/memory/
├── Cargo.toml                  # Top-level Rust service (HTTP adapter + simple provider)
├── src/
│   ├── lib.rs                  # Simple in-memory MemoryProvider
│   ├── main.rs                 # Service binary
│   └── memory_agent_adapter.rs # HTTP client to TypeScript agent
├── agent/                      # TypeScript Always-On Memory Agent
├── data/
│   ├── memory-fabric/          # Unified Rust memory fabric (allternit-memory-fabric)
│   ├── history-ledger/         # Original ledger crate (allternit-history)
│   │                           # Logic now lives in memory-fabric; kept for compat.
│   ├── allternit-memory-provider/ # Original trait crate; superseded by memory-fabric
│   ├── memory-kernel/          # Three-layer memory model (experimental)
│   └── ~~ars-contexta/~~       # Node.js native NLP module — archived to archive/ts-orphans/ars-contexta
├── observation/                # Event observation service (uses memory-fabric ledger)
├── state/
│   ├── history/                # WASM-event history ledger (experimental)
│   └── memory/                 # Advanced tiered memory service (experimental)
├── docs/
│   └── ARCHITECTURE.md         # Detailed architecture (this tree)
└── spec/
    └── MEMORY_FABRIC_SPEC.md   # Fabric specification (this tree)
```

---

## Quick Start

### Build the fabric

```bash
cargo check -p allternit-memory-fabric
cargo test -p allternit-memory-fabric
```

### Use the fabric in a Rust crate

```rust
use allternit_memory_fabric::{MemoryFabric, MemoryProvider, MemoryQuery};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fabric = MemoryFabric::new_in_memory().await?;
    fabric.store("greeting", serde_json::json!("hello")).await?;
    let value = fabric.retrieve("greeting").await?;
    println!("{:?}", value);
    Ok(())
}
```

### Start the TypeScript Memory Agent

```bash
cd services/memory/agent
pnpm install
pnpm run start:http
```

---

## Relationship Between Rust Fabric and TypeScript Agent

```
┌─────────────────────────────────────────────────────────────┐
│                    Rust Services (Layer 4)                   │
│  ┌─────────────────┐      ┌──────────────────────────────┐ │
│  │ MemoryProvider  │◄────►│ allternit-memory-fabric      │ │
│  │ implementations │      │ (traits, ledger, in-memory)  │ │
│  └─────────────────┘      └──────────────────────────────┘ │
│           │                              │                  │
│  ┌────────┴────────────────┐  ┌─────────┴────────┐         │
│  │ InMemoryProvider        │  │ HTTP Adapter     │         │
│  │ (simple / dev / tests)  │  │ (TypeScript agent)│        │
│  └─────────────────────────┘  └─────────┬────────┘         │
└─────────────────────────────────────────┼──────────────────┘
                                          │ HTTP
┌─────────────────────────────────────────┼──────────────────┐
│              TypeScript Memory Agent    │                  │
│  (SQLite + vector index + Ollama/Qwen) ◄┘                  │
└────────────────────────────────────────────────────────────┘
```

- **Rust fabric** is the source of truth for typed interfaces, audit ledgers,
  and provider routing.
- **TypeScript agent** keeps its own ingest/enrichment and LLM-driven
  consolidation, but semantic recall lives in allternit-api's memory plane
  (next section).

---

## One Memory Plane

allternit-api's memory plane is the single source of truth for semantic
recall. It is one SQLite index per user (`cmd/allternit-api/src/memory_index.rs`:
FTS5 keyword + EmbeddingGemma 2 vectors from `ALLTERNIT_EMBED_URL`, fused with
reciprocal rank fusion) over facts, entities, observations, notes, procedures
and document chunks. The memory agent and open-notebook keep their own logic
and UX and their local stores, but write into the plane and recall from it, so
an Agency run (`agency_api/memory_context.rs`, which recalls every space of the
run owner) sees what they know.

```
 Memory Agent (:3201)                      open-notebook (:5055)
 SQLite memories                           SurrealDB + in-memory Qdrant
   │ write: MemoryStore -> KernelAdapter      │ write: Retriever.add_chunks
   │   POST /api/v1/memory/adapters/upsert    │   POST /api/v1/memory/adapters/chunks/upsert
   │   POST /api/v1/memory/adapters/delete    │   POST /api/v1/memory/adapters/chunks/delete
   │ recall: ApiMemoryBackend                 │ recall: Retriever.search (plane first)
   │   POST /api/v1/memory/adapters/search    │   POST /api/v1/memory/adapters/chunks/search
   │   POST /api/v1/memory/adapters/chunks/search
   ▼                                          ▼
 ┌───────────────────────── allternit-api memory plane ─────────────────────────┐
 │ memory_facts (one fact per memory, memory_adapter_links: source+external id) │
 │ memory_documents -> memory_index_chunks (one document per notebook chunk)    │
 │ memory_index_fts + memory_embeddings, scoped by user id                      │
 └──────────────────────────────────┬────────────────────────────────────────────┘
                                    │ recall (all spaces, owner only)
                         Agency context step, /memory/v2/recall, local brain
```

| Store | Writes as | Idempotency key | Local copy |
|-------|-----------|-----------------|------------|
| Memory agent memories | canonical facts (`source: memory-agent`) | (user, source, memory id); unchanged text is a no-op, changed text supersedes the old fact | SQLite + in-memory `VectorStore` (offline fallback for recall) |
| Notebook chunks | memory documents, chunked + embedded at once (`source: open-notebook`, collection = notebook id) | (user, source, notebook id, `<source id>:<chunk index>`) | SurrealDB + Qdrant (fallback when the API is off, down, or has nothing for the notebook) |

**Auth and tenancy.** Both services authenticate with a bearer credential of
one user, `ALLTERNIT_API_TOKEN` (an `at-…` access token or another token the
API's auth middleware accepts), at `ALLTERNIT_API_URL` (default
`http://127.0.0.1:8013`). The API takes the scope from the verified token,
never from the request body, and derives row ids from (user, source, …), so a
service can only read and write its token owner's memory and cannot address
another user's rows. Do not use the internal service token here: it maps
every call to one shared `internal-service` principal. The memory agent is a
single-principal daemon: everything it learns lands in the token owner's
scope. open-notebook has no per-request identity, so it only mirrors notebooks
whose `owner_id` is the token's user (`GET /api/v1/me`) or that have no owner
(`OPEN_NOTEBOOK_MIRROR_UNOWNED=0` turns the latter off); notebooks owned by
someone else, even when shared with that user, stay local.

**Switches.** Unset `ALLTERNIT_API_TOKEN` (or `MEMORY_AGENT_KERNEL=0`,
`OPEN_NOTEBOOK_MEMORY_PLANE=0`) and each service runs on its local store only,
as before. The Rust `memory_agent_adapter` reaches the plane through the agent:
its `/api/search` calls are the agent's text search; `semantic=true` goes
through `ApiMemoryBackend`.

**Migrating existing data (one-shot backfill, idempotent by id).** Run once per
user after setting the token; re-runs send the same ids and the API reports
them unchanged.

```bash
# Memory agent: every SQLite memory -> canonical facts
cd services/memory/agent
ALLTERNIT_API_TOKEN=at-... pnpm kernel:import [path/to/memory.db]

# open-notebook: re-chunk every owned notebook's sources -> memory documents
cd services/open-notebook
ALLTERNIT_API_TOKEN=at-... uv run python scripts/backfill_memory_plane.py
```

The notebook backfill re-chunks the stored source text with the service's own
chunker, so the ids match what live ingestion writes. Notebook chunks written
before this change carry no chunk index in SurrealDB; after the backfill the
plane holds them, and search reads the plane first.

---

## Documentation

| Document | Location | Purpose |
|----------|----------|---------|
| Architecture | `docs/ARCHITECTURE.md` | Design and component relationships |
| Specification | `spec/MEMORY_FABRIC_SPEC.md` | Trait contracts and data formats |
| Agent README | `agent/README.md` | TypeScript agent setup |
| Integration Guide | `agent/INTEGRATION_GUIDE.md` | Developer integration |

---

## Consolidation Notes

- `allternit-memory-fabric` now centralizes the trait interface previously in
  `data/allternit-memory-provider` and the ledger logic previously in
  `data/history-ledger`.
- `services/memory/Cargo.toml` no longer references missing SDK crates.
- `services/memory/observation` now depends on `allternit-memory-fabric`.
- Experimental crates (`state/memory`, `state/history`, `data/memory-kernel`)
  remain in the tree but are not included in the workspace until their missing
  SDK/policy/router dependencies are available.

---

**Layer**: 4 (Services)  
**Domain**: Memory & Persistence  
**Status**: Unified fabric active ✅
