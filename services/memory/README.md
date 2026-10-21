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
- **TypeScript agent** is the source of truth for long-term semantic memory,
  embeddings, and LLM-driven consolidation.

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
