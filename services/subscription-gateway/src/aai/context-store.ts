// Persisted AAI context state (state.db `aai_contexts`, migration 0009), one row per (adapterId, contextId).
// The host writes it as contexts open, turn, are read and close; after a restart it is what tells a resumable
// context (restore it) from a lost one (CONTEXT_NOT_FOUND with details.reason CONTEXT_LOST) and a closed one.
import type { ContextSnapshot } from "@allternit/agent-gateway";
import type { BotExecutionBinding } from "@allternit/subscription-fabric-contracts";
import type { Db } from "../store/db.js";

export interface AaiContextRecord {
  adapterId: string;
  contextId: string;
  binding: BotExecutionBinding;
  agentId: string | null;
  conversationKeys: Record<string, string> | null;
  /** Provider snapshot state (opaque here). */
  state: unknown;
  lastCursor: string | null;
  closed: boolean;
  createdAt: string;
  updatedAt: string;
}

interface Row {
  adapter_id: string; context_id: string; binding: string; agent_id: string | null; conversation_keys: string | null;
  state: string | null; last_cursor: string | null; closed: number; created_at: string; updated_at: string;
}

const parse = (s: string | null): unknown => { if (s == null) return null; try { return JSON.parse(s); } catch { return null; } };

/** Closed contexts are kept this long (so a late call still learns "closed", not "lost"), then pruned. */
const CLOSED_RETENTION_MS = 30 * 86_400_000;

export class AaiContextStore {
  constructor(private db: Db, private now: () => Date = () => new Date()) {}

  get(adapterId: string, contextId: string): AaiContextRecord | undefined {
    const r = this.db.prepare("SELECT * FROM aai_contexts WHERE adapter_id = ? AND context_id = ?").get(adapterId, contextId) as Row | undefined;
    if (!r) return undefined;
    return {
      adapterId: r.adapter_id, contextId: r.context_id, binding: parse(r.binding) as BotExecutionBinding, agentId: r.agent_id,
      conversationKeys: parse(r.conversation_keys) as Record<string, string> | null, state: parse(r.state), lastCursor: r.last_cursor,
      closed: r.closed === 1, createdAt: r.created_at, updatedAt: r.updated_at,
    };
  }

  /** A context was opened (or adopted): (re)start its row. A reused id starts over as a new, open context. */
  opened(adapterId: string, contextId: string, binding: BotExecutionBinding, agentId: string | null, snap?: ContextSnapshot): void {
    const at = this.now().toISOString();
    this.db.prepare(
      `INSERT INTO aai_contexts (adapter_id, context_id, binding, agent_id, conversation_keys, state, last_cursor, closed, created_at, updated_at)
       VALUES (?, ?, ?, ?, ?, ?, NULL, 0, ?, ?)
       ON CONFLICT (adapter_id, context_id) DO UPDATE SET binding = excluded.binding, agent_id = excluded.agent_id,
         conversation_keys = COALESCE(excluded.conversation_keys, conversation_keys), state = COALESCE(excluded.state, state),
         closed = 0, updated_at = excluded.updated_at`,
    ).run(adapterId, contextId, JSON.stringify(binding), snap?.agentId ?? agentId, snap?.conversationKeys ? JSON.stringify(snap.conversationKeys) : null,
      snap?.state !== undefined ? JSON.stringify(snap.state) : null, at, at);
    this.db.prepare("DELETE FROM aai_contexts WHERE closed = 1 AND updated_at < ?").run(new Date(this.now().getTime() - CLOSED_RETENTION_MS).toISOString());
  }

  /** The provider's latest snapshot after a turn. */
  saveSnapshot(adapterId: string, contextId: string, snap: ContextSnapshot): void {
    this.db.prepare(
      `UPDATE aai_contexts SET state = ?, conversation_keys = COALESCE(?, conversation_keys), agent_id = COALESCE(?, agent_id), updated_at = ?
       WHERE adapter_id = ? AND context_id = ?`,
    ).run(snap.state !== undefined ? JSON.stringify(snap.state) : null, snap.conversationKeys ? JSON.stringify(snap.conversationKeys) : null,
      snap.agentId ?? null, this.now().toISOString(), adapterId, contextId);
  }

  cursor(adapterId: string, contextId: string, cursor: string): void {
    this.db.prepare("UPDATE aai_contexts SET last_cursor = ?, updated_at = ? WHERE adapter_id = ? AND context_id = ? AND last_cursor IS NOT ?")
      .run(cursor, this.now().toISOString(), adapterId, contextId, cursor);
  }

  closed(adapterId: string, contextId: string): void {
    this.db.prepare("UPDATE aai_contexts SET closed = 1, updated_at = ? WHERE adapter_id = ? AND context_id = ?")
      .run(this.now().toISOString(), adapterId, contextId);
  }
}
