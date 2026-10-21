# GIZZI Brain Architecture

## The Best Memory Brain for Local-First AI

After analyzing 10+ memory solutions (Mem0, Zep, Chroma, Pinecone, sqlite-vec, CrewAI, AutoGen, Graphiti, and plain Git), the best architecture for gizzi-code is a **triple-store layered system** that combines human readability with machine searchability.

---

## Why This Architecture Wins

| Requirement | Other Solutions | GIZZI Brain |
|-------------|----------------|-------------|
| **Zero dependencies** | Mem0 (cloud), Zep (Postgres), Chroma (separate process) | SQLite only — already used |
| **Human readable** | Vector DBs are opaque | BRAIN.md — git-friendly markdown |
| **Semantic search** | Requires OpenAI API ($) | Pluggable: keyword → OpenAI → Ollama |
| **Knowledge graph** | Zep has it, others don't | Built-in entity + relation tables |
| **Temporal awareness** | Most ignore time | First/last seen, recency ranking |
| **Privacy** | Cloud sends data to 3rd party | 100% local by default |
| **Speed** | Network round-trips | <10ms query on local SQLite |

---

## The Triple Store

```
┌─────────────────────────────────────────────────────────────┐
│                     CONSCIOUS LAYER                          │
│                  BRAIN.md (human-readable)                   │
│     Executive summary, active context, key entities          │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                    COGNITIVE LAYER                           │
│              SQLite — Drizzle ORM schema                     │
│  ┌──────────────┐  ┌──────────────┐  ┌─────────────────┐   │
│  │memory_chunk  │  │memory_entity │  │memory_relation  │   │
│  │──────────────│  │──────────────│  │─────────────────│   │
│  │episodic      │  │person        │  │subject_id       │   │
│  │semantic      │  │concept       │  │predicate        │   │
│  │procedural    │  │file          │  │object_id        │   │
│  └──────────────┘  │tool          │  │confidence       │   │
│                    │task          │  └─────────────────┘   │
│                    └──────────────┘                          │
│  ┌──────────────┐                                            │
│  │memory_embedding│  ← optional, pluggable                  │
│  │──────────────│                                            │
│  │vector_json   │  ← stores embedding arrays                │
│  │provider      │  ← openai | ollama | local | keyword      │
│  └──────────────┘                                            │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                   EMBEDDING LAYER                            │
│              Pluggable — zero deps by default                │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐   │
│  │ keyword  │  │  OpenAI  │  │  Ollama  │  │  Local   │   │
│  │ (free)   │→ │  (best)  │→ │  (local) │→ │  (tiny)  │   │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘   │
│     Default      API key       Self-hosted     ONNX/WASM   │
└─────────────────────────────────────────────────────────────┘
```

---

## The Three Memory Types

### 1. Episodic — "What Happened"
Events with timestamps. Raw conversation history, tool calls, file edits.

```typescript
BrainService.recordEvent(tenantId, {
  event_type: "file_edited",
  description: "Fixed race condition in auth.rs",
  importance: 8,
})
```

**Retrieval:** Recency-weighted. Recent events float to the top.

### 2. Semantic — "What Matters"
Extracted facts, insights, decisions. Distilled from episodic noise.

```typescript
BrainService.remember(tenantId, {
  fact: "Redis runs on port 6379 in production",
  category: "infrastructure",
  importance: 9,
})
```

**Retrieval:** Importance + semantic similarity. Facts that match the current query context.

### 3. Procedural — "How To Do It"
Skills, playbooks, successful patterns.

```typescript
BrainService.recordSkill(tenantId, {
  name: "fix-race-condition",
  description: "Pattern for fixing race conditions in Rust",
  steps: ["Identify shared state", "Add mutex or atomics", "Write test"],
})
```

**Retrieval:** Skill name matching + step similarity.

---

## The Entity Graph

Beyond flat memories, the brain tracks **entities and relationships**:

```
[auth.rs] ──contains──► [RaceCondition]
   │                        │
   └──mentioned_in──► [PR #472]
                           │
                           └──fixed_by──► [User: macbook]
```

This enables "graph walking" — starting from a file, find related PRs, then find who fixed them, then find their other contributions.

---

## Retrieval Strategy (No Embeddings Required)

By default, the brain uses **zero-dependency retrieval**:

1. **Recency filter** — only memories from last N hours (configurable)
2. **Importance rank** — high-importance memories score higher
3. **Keyword match** — SQLite `LIKE` on content and source
4. **Access frequency** — frequently recalled memories persist longer
5. **Entity expansion** — if query mentions "auth.rs", also retrieve entities related to that file

**With embeddings configured**, add:
6. **Semantic similarity** — cosine similarity on vector embeddings

---

## BRAIN.md Sync

The brain automatically maintains a human-readable `BRAIN.md`:

```markdown
# GIZZI Brain

**Tenant:** session-abc-123  
**Updated:** 2026-04-14T10:45:00Z  
**Memories:** 47  
**Entities:** 12

## Active Context
- **2026-04-14 10:42** — Fixed race condition in auth.rs
- **2026-04-14 10:38** — Reviewed PR #472

## Key Facts
- Redis runs on port 6379 in production
- The project uses Drizzle ORM with SQLite

## Known Skills
- **fix-race-condition** — Pattern for fixing race conditions in Rust

## Key Entities
- **auth.rs** (file) — Authentication module
- **PR #472** (task) — Race condition fix
```

This file is:
- **Git-friendly** — commit it to track context evolution
- **Human-readable** — developers can review what the AI knows
- **AI-parseable** — the AI reads this at session start for quick context

---

## Embedding Providers (Pluggable)

| Provider | Setup | Quality | Cost | Privacy |
|----------|-------|---------|------|---------|
| **Keyword** (default) | Nothing | Low | Free | 100% local |
| **OpenAI** | `OPENAI_API_KEY` | Excellent | $0.02/1M tokens | Cloud |
| **Ollama** | `ollama run nomic-embed-text` | Good | Free | Local |
| **Local ONNX** | Download 20MB model | Good | Free | Local |

**Recommendation:** Start with keyword (works immediately). Add Ollama when you want semantic search without cloud costs. Use OpenAI only if you need the absolute best quality.

---

## Why Not Mem0 / Zep / Chroma?

| Solution | Why We Didn't Use It |
|----------|---------------------|
| **Mem0** | Cloud-only, vendor lock-in, sends all data to their servers |
| **Zep** | Requires PostgreSQL + complex setup, overkill for local CLI |
| **Chroma** | Separate process to manage, adds deployment complexity |
| **Pinecone** | Cloud-only, expensive at scale, not local-first |
| **sqlite-vec** | Would be ideal, but extension loading in Bun is unreliable |
| **CrewAI Memory** | Tied to CrewAI framework, not general-purpose |
| **AutoGen Memory** | Microsoft research code, too heavy for a CLI tool |

Our SQLite-native approach gives 90% of the value with 10% of the complexity.

---

## CLI Usage

```bash
# Check brain status
gizzi brain status

# Remember a fact
gizzi brain remember -t "Redis runs on port 6379" --importance 9

# Recall memories
gizzi brain recall -t "Redis" --limit 5

# List entities
gizzi brain entities

# Sync BRAIN.md
gizzi brain sync

# Forget something
gizzi brain forget --id mem-xxx
```

---

## Future Enhancements

1. **Auto-extraction** — AI automatically extracts facts from conversations
2. **Memory decay** — Low-importance, unaccessed memories fade over time
3. **Conflict resolution** — When new facts contradict old ones, flag for review
4. **Cross-session sync** — Share memories between sessions via git-synced BRAIN.md
5. **ONNX embeddings** — Load a 20MB embedding model for fully local semantic search
