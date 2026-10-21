/**
 * Brain Service
 *
 * The gizzi brain — triple-store memory with episodic, semantic, and procedural layers.
 *
 * Architecture:
 *   ┌─────────────────────────────────────────┐
 *   │  BRAIN.md (conscious, human-readable)   │
 *   └─────────────────────────────────────────┘
 *                   │
 *                   ▼
 *   ┌─────────────────────────────────────────┐
 *   │  SQLite (Drizzle)                       │
 *   │  - memory_chunk: raw memories           │
 *   │  - memory_embedding: vector search      │
 *   │  - memory_entity: knowledge graph       │
 *   │  - memory_relation: graph edges         │
 *   └─────────────────────────────────────────┘
 *
 * Retrieval Strategy (no embeddings required by default):
 *   1. Recency: memories from last N hours
 *   2. Importance: high-importance memories rank higher
 *   3. Keyword: full-text match on content
 *   4. Access frequency: frequently accessed memories persist
 *
 * With embeddings configured:
 *   5. Semantic: cosine similarity on vector embeddings
 */

import { eq, and, desc, sql, gte, like, or } from "drizzle-orm"
import { Database } from "@/runtime/session/storage/db"
import {
  MemoryChunkTable,
  MemoryEmbeddingTable,
  MemoryEntityTable,
  MemoryRelationTable,
} from "./memory.sql"
import { Log } from "@/shared/util/log"
import { randomUUID } from "crypto"
import { homedir } from "os"
import { join } from "path"
import { readFile, writeFile, mkdir } from "fs/promises"
import { access } from "fs/promises"

const log = Log.create({ service: "brain" })

// ── Types ───────────────────────────────────────────────────────────────────

export interface MemoryQuery {
  tenant_id: string
  query?: string
  chunk_type?: "episodic" | "semantic" | "procedural"
  limit?: number
  since?: number // timestamp
  min_importance?: number
}

export interface MemoryChunk {
  id: string
  tenant_id: string
  chunk_type: "episodic" | "semantic" | "procedural"
  source?: string
  content: string
  metadata?: Record<string, unknown>
  importance: number
  access_count: number
  last_accessed_at?: number
  time_created: number
}

// TODO(types): mirrors the entity_type $type<> union on
// MemoryEntityTable (memory.sql.ts) — Entity previously widened it to string.
type EntityType = "person" | "concept" | "file" | "tool" | "task" | "project"

export interface Entity {
  id: string
  tenant_id: string
  entity_type: EntityType
  name: string
  description?: string
  mention_count: number
}

// ── Storage ─────────────────────────────────────────────────────────────────

export namespace BrainService {
  // ── Memory Chunks ─────────────────────────────────────────────────────────

  export function createChunk(
    tenantId: string,
    chunk: Omit<MemoryChunk, "id" | "tenant_id" | "time_created" | "time_updated" | "access_count" | "last_accessed_at">,
  ): MemoryChunk {
    const id = `mem-${randomUUID()}`
    const now = Date.now()

    Database.use((db) => {
      db.insert(MemoryChunkTable).values({
        id,
        tenant_id: tenantId,
        chunk_type: chunk.chunk_type,
        source: chunk.source,
        content: chunk.content,
        metadata_json: chunk.metadata ?? {},
        importance: chunk.importance ?? 5,
        access_count: 0,
        time_created: now,
        time_updated: now,
      }).run()
    })

    return getChunk(id)!
  }

  export function getChunk(id: string): MemoryChunk | undefined {
    return Database.use((db) => {
      const rows = db.select().from(MemoryChunkTable).where(eq(MemoryChunkTable.id, id)).all()
      return rows[0] as MemoryChunk | undefined
    })
  }

  export function queryChunks(options: MemoryQuery): MemoryChunk[] {
    return Database.use((db) => {
      let query = db.select().from(MemoryChunkTable).$dynamic()
      const conditions = [eq(MemoryChunkTable.tenant_id, options.tenant_id)]

      if (options.chunk_type) {
        conditions.push(eq(MemoryChunkTable.chunk_type, options.chunk_type))
      }
      if (options.since) {
        conditions.push(gte(MemoryChunkTable.time_created, options.since))
      }
      if (options.min_importance) {
        conditions.push(gte(MemoryChunkTable.importance, options.min_importance))
      }
      if (options.query) {
        const q = `%${options.query}%`
        conditions.push(or(like(MemoryChunkTable.content, q), like(MemoryChunkTable.source, q)))
      }

      query = query.where(and(...conditions)) as typeof query
      query = query
        .orderBy(desc(MemoryChunkTable.importance), desc(MemoryChunkTable.time_created))
        .limit(options.limit ?? 20) as typeof query

      return query.all() as MemoryChunk[]
    })
  }

  export function recordAccess(id: string): void {
    Database.use((db) => {
      db.update(MemoryChunkTable)
        .set({
          access_count: sql`${MemoryChunkTable.access_count} + 1`,
          last_accessed_at: Date.now(),
        })
        .where(eq(MemoryChunkTable.id, id))
        .run()
    })
  }

  export function deleteChunk(id: string): void {
    Database.use((db) => {
      db.delete(MemoryChunkTable).where(eq(MemoryChunkTable.id, id)).run()
    })
  }

  // ── Episodic Memory (Events) ──────────────────────────────────────────────

  export function recordEvent(
    tenantId: string,
    event: {
      event_type: string
      description: string
      importance?: number
      metadata?: Record<string, unknown>
    },
  ): MemoryChunk {
    return createChunk(tenantId, {
      chunk_type: "episodic",
      source: event.event_type,
      content: event.description,
      metadata: event.metadata,
      importance: event.importance ?? 5,
    })
  }

  // ── Semantic Memory (Facts) ───────────────────────────────────────────────

  export function remember(
    tenantId: string,
    fact: {
      fact: string
      category?: string
      importance?: number
      metadata?: Record<string, unknown>
    },
  ): MemoryChunk {
    return createChunk(tenantId, {
      chunk_type: "semantic",
      source: fact.category ?? "fact",
      content: fact.fact,
      metadata: fact.metadata,
      importance: fact.importance ?? 7,
    })
  }

  // ── Procedural Memory (Skills) ────────────────────────────────────────────

  export function recordSkill(
    tenantId: string,
    skill: {
      name: string
      description: string
      steps: string[]
      importance?: number
    },
  ): MemoryChunk {
    return createChunk(tenantId, {
      chunk_type: "procedural",
      source: `skill:${skill.name}`,
      content: `${skill.description}\n\nSteps:\n${skill.steps.map((s, i) => `${i + 1}. ${s}`).join("\n")}`,
      importance: skill.importance ?? 8,
    })
  }

  // ── Entity Graph ──────────────────────────────────────────────────────────

  export function ensureEntity(
    tenantId: string,
    entity: {
      entity_type: Entity["entity_type"]
      name: string
      description?: string
      canonical?: Record<string, unknown>
    },
  ): Entity {
    const existing = Database.use((db) => {
      return db
        .select()
        .from(MemoryEntityTable)
        .where(
          and(eq(MemoryEntityTable.tenant_id, tenantId), eq(MemoryEntityTable.name, entity.name)),
        )
        .get() as Entity | undefined
    })

    if (existing) {
      Database.use((db) => {
        db.update(MemoryEntityTable)
          .set({
            last_seen_at: Date.now(),
            mention_count: sql`${MemoryEntityTable.mention_count} + 1`,
          })
          .where(eq(MemoryEntityTable.id, existing.id))
          .run()
      })
      return existing
    }

    const id = `ent-${randomUUID()}`
    const now = Date.now()

    Database.use((db) => {
      db.insert(MemoryEntityTable).values({
        id,
        tenant_id: tenantId,
        entity_type: entity.entity_type,
        name: entity.name,
        description: entity.description,
        canonical_json: entity.canonical ?? {},
        first_seen_at: now,
        last_seen_at: now,
        mention_count: 1,
        time_created: now,
        time_updated: now,
      }).run()
    })

    return getEntity(id)!
  }

  export function getEntity(id: string): Entity | undefined {
    return Database.use((db) => {
      return db.select().from(MemoryEntityTable).where(eq(MemoryEntityTable.id, id)).get() as
        | Entity
        | undefined
    })
  }

  export function queryEntities(tenantId: string, type?: string): Entity[] {
    return Database.use((db) => {
      let query = db
        .select()
        .from(MemoryEntityTable)
        .where(eq(MemoryEntityTable.tenant_id, tenantId))
        .$dynamic()
      if (type) {
        query = query.where(eq(MemoryEntityTable.entity_type, type as EntityType)) as typeof query
      }
      return query.orderBy(desc(MemoryEntityTable.mention_count)).all() as Entity[]
    })
  }

  export function relateEntities(
    tenantId: string,
    subjectId: string,
    predicate: string,
    objectId: string,
    opts?: { confidence?: number; evidenceChunkId?: string },
  ): void {
    const id = `rel-${randomUUID()}`
    const now = Date.now()

    Database.use((db) => {
      db.insert(MemoryRelationTable)
        .values({
          id,
          tenant_id: tenantId,
          subject_id: subjectId,
          predicate,
          object_id: objectId,
          confidence: opts?.confidence ?? 7,
          evidence_chunk_id: opts?.evidenceChunkId,
          time_created: now,
          time_updated: now,
        })
        .onConflictDoNothing()
        .run()
    })
  }

  export function getRelated(tenantId: string, entityId: string): Array<{ predicate: string; entity: Entity }> {
    return Database.use((db) => {
      const relations = db
        .select()
        .from(MemoryRelationTable)
        .where(
          and(eq(MemoryRelationTable.tenant_id, tenantId), eq(MemoryRelationTable.subject_id, entityId)),
        )
        .all()

      const results: Array<{ predicate: string; entity: Entity }> = []
      for (const rel of relations) {
        const entity = db
          .select()
          .from(MemoryEntityTable)
          .where(eq(MemoryEntityTable.id, rel.object_id))
          .get() as Entity | undefined
        if (entity) {
          results.push({ predicate: rel.predicate, entity })
        }
      }
      return results
    })
  }

  // ── BRAIN.md Sync ─────────────────────────────────────────────────────────

  export async function syncBrainMd(tenantId: string, workspacePath?: string): Promise<string> {
    const ws = workspacePath ?? join(homedir(), ".allternit", "workspace")
    const brainPath = join(ws, ".gizzi", "L1-COGNITIVE", "BRAIN.md")

    // Ensure directory exists
    await mkdir(join(ws, ".gizzi", "L1-COGNITIVE"), { recursive: true })

    // Query recent + important memories
    const recent = queryChunks({ tenant_id: tenantId, limit: 50, min_importance: 5 })
    const entities = queryEntities(tenantId)

    // Build BRAIN.md content
    const episodic = recent.filter((m) => m.chunk_type === "episodic")
    const semantic = recent.filter((m) => m.chunk_type === "semantic")
    const procedural = recent.filter((m) => m.chunk_type === "procedural")

    const lines = [
      "# GIZZI Brain",
      "",
      `**Tenant:** ${tenantId}  `,
      `**Updated:** ${new Date().toISOString()}  `,
      `**Memories:** ${recent.length}  `,
      `**Entities:** ${entities.length}`,
      "",
      "## Active Context",
      "",
      ...(episodic.slice(0, 10).map((m) => `- **${new Date(m.time_created).toLocaleString()}** — ${m.content}`)),
      "",
      "## Key Facts",
      "",
      ...(semantic.slice(0, 10).map((m) => `- ${m.content}`)),
      "",
      "## Known Skills",
      "",
      ...(procedural.slice(0, 5).map((m) => `- **${m.source}** — ${m.content.split("\n")[0]}`)),
      "",
      "## Key Entities",
      "",
      ...(entities.slice(0, 15).map((e) => `- **${e.name}** (${e.entity_type}) — ${e.description ?? "no description"}`)),
      "",
    ]

    const content = lines.join("\n")
    await writeFile(brainPath, content, "utf-8")
    log.info("synced BRAIN.md", { path: brainPath, memories: recent.length, entities: entities.length })

    return content
  }

  export async function readBrainMd(workspacePath?: string): Promise<string | null> {
    const ws = workspacePath ?? join(homedir(), ".allternit", "workspace")
    const brainPath = join(ws, ".gizzi", "L1-COGNITIVE", "BRAIN.md")
    try {
      return await readFile(brainPath, "utf-8")
    } catch {
      return null
    }
  }

  // ── Stats ─────────────────────────────────────────────────────────────────

  export function stats(tenantId: string): {
    total_memories: number
    episodic: number
    semantic: number
    procedural: number
    entities: number
    relations: number
  } {
    return Database.use((db) => {
      const total = db
        .select({ count: sql<number>`count(*)` })
        .from(MemoryChunkTable)
        .where(eq(MemoryChunkTable.tenant_id, tenantId))
        .get()!.count

      const episodic = db
        .select({ count: sql<number>`count(*)` })
        .from(MemoryChunkTable)
        .where(and(eq(MemoryChunkTable.tenant_id, tenantId), eq(MemoryChunkTable.chunk_type, "episodic")))
        .get()!.count

      const semantic = db
        .select({ count: sql<number>`count(*)` })
        .from(MemoryChunkTable)
        .where(and(eq(MemoryChunkTable.tenant_id, tenantId), eq(MemoryChunkTable.chunk_type, "semantic")))
        .get()!.count

      const procedural = db
        .select({ count: sql<number>`count(*)` })
        .from(MemoryChunkTable)
        .where(and(eq(MemoryChunkTable.tenant_id, tenantId), eq(MemoryChunkTable.chunk_type, "procedural")))
        .get()!.count

      const entities = db
        .select({ count: sql<number>`count(*)` })
        .from(MemoryEntityTable)
        .where(eq(MemoryEntityTable.tenant_id, tenantId))
        .get()!.count

      const relations = db
        .select({ count: sql<number>`count(*)` })
        .from(MemoryRelationTable)
        .where(eq(MemoryRelationTable.tenant_id, tenantId))
        .get()!.count

      return { total_memories: total, episodic, semantic, procedural, entities, relations }
    })
  }
}

// ── Embedding (Pluggable) ───────────────────────────────────────────────────

export interface EmbeddingProvider {
  name: string
  dimensions: number
  embed(text: string): Promise<number[]>
}

/** Simple keyword-based "embedding" fallback — requires zero dependencies */
export const KeywordEmbedding: EmbeddingProvider = {
  name: "keyword",
  dimensions: 1,
  async embed(text: string): Promise<number[]> {
    // Normalize and hash for deterministic pseudo-embedding
    const normalized = text.toLowerCase().replace(/[^a-z0-9\s]/g, "")
    let hash = 0
    for (let i = 0; i < normalized.length; i++) {
      hash = ((hash << 5) - hash + normalized.charCodeAt(i)) | 0
    }
    return [hash]
  },
}

/** OpenAI embedding provider */
export function createOpenAIEmbedder(apiKey: string, model = "text-embedding-3-small"): EmbeddingProvider {
  return {
    name: `openai:${model}`,
    dimensions: model.includes("3-large") ? 3072 : model.includes("3-small") ? 1536 : 1536,
    async embed(text: string): Promise<number[]> {
      const res = await fetch("https://api.openai.com/v1/embeddings", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${apiKey}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ input: text, model }),
      })
      if (!res.ok) throw new Error(`OpenAI embed error: ${res.status}`)
      const data = (await res.json()) as { data: { embedding: number[] }[] }
      return data.data[0].embedding
    },
  }
}
