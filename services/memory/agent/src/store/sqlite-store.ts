/**
 * SQLite Store - Persistent Memory Storage
 * 
 * Handles all database operations for the memory agent system
 */

import Database from 'better-sqlite3';
import { v4 as uuidv4 } from 'uuid';
import type {
  Memory,
  MemoryConnection,
  Insight,
  MemoryStatus,
  MemoryImportance,
  FileType,
} from '../types/memory.types.js';
import { KernelAdapter } from './kernel-adapter.js';

/**
 * Database schema version
 */
const SCHEMA_VERSION = 1;

/**
 * SQLite Store class for memory persistence
 */
export class MemoryStore {
  private db: Database;
  /** Mirror to the canonical allternit-api memory kernel (WP-M1d); undefined = off. */
  private kernel: KernelAdapter | undefined;

  constructor(databasePath: string, kernel: KernelAdapter | undefined = KernelAdapter.fromEnv()) {
    this.kernel = kernel;
    this.db = new Database(databasePath);
    this.db.pragma('journal_mode = WAL');
    this.db.pragma('foreign_keys = ON');
    this.initializeSchema();
  }

  /**
   * Initialize database schema
   */
  private initializeSchema(): void {
    this.db.exec(`
      -- Schema version tracking
      CREATE TABLE IF NOT EXISTS schema_version (
        version INTEGER PRIMARY KEY,
        applied_at TEXT NOT NULL DEFAULT (datetime('now'))
      );

      -- Memories table
      CREATE TABLE IF NOT EXISTS memories (
        id TEXT PRIMARY KEY,
        content TEXT NOT NULL,
        summary TEXT NOT NULL,
        entities TEXT NOT NULL DEFAULT '[]',
        topics TEXT NOT NULL DEFAULT '[]',
        importance TEXT NOT NULL DEFAULT 'medium',
        status TEXT NOT NULL DEFAULT 'raw',
        source TEXT NOT NULL,
        source_type TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT (datetime('now')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now')),
        consolidated_at TEXT,
        metadata TEXT NOT NULL DEFAULT '{}'
      );

      -- Memory connections table
      CREATE TABLE IF NOT EXISTS memory_connections (
        id TEXT PRIMARY KEY,
        memory_ids TEXT NOT NULL DEFAULT '[]',
        relationship TEXT NOT NULL,
        strength REAL NOT NULL DEFAULT 0.5,
        created_at TEXT NOT NULL DEFAULT (datetime('now'))
      );

      -- Insights table
      CREATE TABLE IF NOT EXISTS insights (
        id TEXT PRIMARY KEY,
        title TEXT NOT NULL,
        content TEXT NOT NULL,
        memory_ids TEXT NOT NULL DEFAULT '[]',
        topics TEXT NOT NULL DEFAULT '[]',
        confidence REAL NOT NULL DEFAULT 0.5,
        created_at TEXT NOT NULL DEFAULT (datetime('now')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now'))
      );

      -- Indexes for performance
      CREATE INDEX IF NOT EXISTS idx_memories_status ON memories(status);
      CREATE INDEX IF NOT EXISTS idx_memories_importance ON memories(importance);
      CREATE INDEX IF NOT EXISTS idx_memories_created_at ON memories(created_at);
      CREATE INDEX IF NOT EXISTS idx_memories_topics ON memories(topics);
      CREATE INDEX IF NOT EXISTS idx_insights_topics ON insights(topics);

      -- Insert schema version if not exists
      INSERT OR IGNORE INTO schema_version (version) VALUES (${SCHEMA_VERSION});
    `);
  }

  /**
   * Get database instance (for advanced operations)
   */
  getDatabase(): Database {
    return this.db;
  }

  // ==================== MEMORY OPERATIONS ====================

  /**
   * Create a new memory
   */
  createMemory(memory: Omit<Memory, 'id' | 'createdAt' | 'updatedAt'>): Memory {
    const id = uuidv4();
    const now = new Date().toISOString();
    
    const stmt = this.db.prepare(`
      INSERT INTO memories (id, content, summary, entities, topics, importance, status, source, source_type, created_at, updated_at, consolidated_at, metadata)
      VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    `);

    stmt.run(
      id,
      memory.content,
      memory.summary,
      JSON.stringify(memory.entities),
      JSON.stringify(memory.topics),
      memory.importance,
      memory.status,
      memory.source,
      memory.sourceType,
      now,
      now,
      memory.consolidatedAt || null,
      JSON.stringify(memory.metadata)
    );

    const created = {
      ...memory,
      id,
      createdAt: now,
      updatedAt: now,
    };
    void this.kernel?.upsert([created]);
    return created;
  }

  /**
   * Get memory by ID
   */
  getMemory(id: string): Memory | undefined {
    const stmt = this.db.prepare('SELECT * FROM memories WHERE id = ?');
    const row = stmt.get(id) as any;
    
    if (!row) return undefined;
    
    return this.rowToMemory(row);
  }

  /**
   * Get memories by status
   */
  getMemoriesByStatus(status: MemoryStatus): Memory[] {
    const stmt = this.db.prepare('SELECT * FROM memories WHERE status = ? ORDER BY created_at DESC');
    const rows = stmt.all(status) as any[];
    
    return rows.map(row => this.rowToMemory(row));
  }

  /**
   * Get all memories
   */
  getAllMemories(): Memory[] {
    const stmt = this.db.prepare('SELECT * FROM memories ORDER BY created_at DESC');
    const rows = stmt.all() as any[];
    
    return rows.map(row => this.rowToMemory(row));
  }

  /**
   * Get memories by topic
   */
  getMemoriesByTopic(topic: string): Memory[] {
    const stmt = this.db.prepare(`
      SELECT * FROM memories 
      WHERE topics LIKE ? 
      ORDER BY created_at DESC
    `);
    const rows = stmt.all(`%${topic}%`) as any[];
    
    return rows.map(row => this.rowToMemory(row));
  }

  /**
   * Get memories by entity
   */
  getMemoriesByEntity(entity: string): Memory[] {
    const stmt = this.db.prepare(`
      SELECT * FROM memories 
      WHERE entities LIKE ? 
      ORDER BY created_at DESC
    `);
    const rows = stmt.all(`%${entity}%`) as any[];
    
    return rows.map(row => this.rowToMemory(row));
  }

  /**
   * Update memory
   */
  updateMemory(id: string, updates: Partial<Memory>): Memory | undefined {
    const existing = this.getMemory(id);
    if (!existing) return undefined;

    const fields: string[] = [];
    const values: any[] = [];

    if (updates.content !== undefined) {
      fields.push('content = ?');
      values.push(updates.content);
    }
    if (updates.summary !== undefined) {
      fields.push('summary = ?');
      values.push(updates.summary);
    }
    if (updates.entities !== undefined) {
      fields.push('entities = ?');
      values.push(JSON.stringify(updates.entities));
    }
    if (updates.topics !== undefined) {
      fields.push('topics = ?');
      values.push(JSON.stringify(updates.topics));
    }
    if (updates.importance !== undefined) {
      fields.push('importance = ?');
      values.push(updates.importance);
    }
    if (updates.status !== undefined) {
      fields.push('status = ?');
      values.push(updates.status);
    }
    if (updates.consolidatedAt !== undefined) {
      fields.push('consolidated_at = ?');
      values.push(updates.consolidatedAt);
    }
    if (updates.metadata !== undefined) {
      fields.push('metadata = ?');
      values.push(JSON.stringify(updates.metadata));
    }

    fields.push('updated_at = ?');
    values.push(new Date().toISOString());

    values.push(id);

    const stmt = this.db.prepare(`
      UPDATE memories 
      SET ${fields.join(', ')} 
      WHERE id = ?
    `);

    stmt.run(...values);

    const updated = this.getMemory(id);
    if (updated) void this.kernel?.upsert([updated]);
    return updated;
  }

  /**
   * Delete memory
   */
  deleteMemory(id: string): boolean {
    const stmt = this.db.prepare('DELETE FROM memories WHERE id = ?');
    const result = stmt.run(id);
    if (result.changes > 0) void this.kernel?.remove([id]);
    return result.changes > 0;
  }

  /**
   * Delete all memories
   */
  deleteAllMemories(): number {
    const stmt = this.db.prepare('DELETE FROM memories');
    const result = stmt.run();
    return result.changes;
  }

  /**
   * Get memory count
   */
  getMemoryCount(): number {
    const stmt = this.db.prepare('SELECT COUNT(*) as count FROM memories');
    const result = stmt.get() as { count: number };
    return result.count;
  }

  /**
   * Get unconsolidated memories (raw or processed)
   */
  getUnconsolidatedMemories(): Memory[] {
    const stmt = this.db.prepare(`
      SELECT * FROM memories 
      WHERE status IN ('raw', 'processed') 
      ORDER BY created_at ASC
    `);
    const rows = stmt.all() as any[];
    
    return rows.map(row => this.rowToMemory(row));
  }

  // ==================== CONNECTION OPERATIONS ====================

  /**
   * Create a memory connection
   */
  createConnection(connection: Omit<MemoryConnection, 'id' | 'createdAt'>): MemoryConnection {
    const id = uuidv4();
    const now = new Date().toISOString();
    
    const stmt = this.db.prepare(`
      INSERT INTO memory_connections (id, memory_ids, relationship, strength, created_at)
      VALUES (?, ?, ?, ?, ?)
    `);

    stmt.run(
      id,
      JSON.stringify(connection.memoryIds),
      connection.relationship,
      connection.strength,
      now
    );

    return {
      ...connection,
      id,
      createdAt: now,
    };
  }

  /**
   * Get all connections
   */
  getAllConnections(): MemoryConnection[] {
    const stmt = this.db.prepare('SELECT * FROM memory_connections ORDER BY created_at DESC');
    const rows = stmt.all() as any[];
    
    return rows.map(row => ({
      id: row.id,
      memoryIds: JSON.parse(row.memory_ids),
      relationship: row.relationship,
      strength: row.strength,
      createdAt: row.created_at,
    }));
  }

  /**
   * Get connections for a memory
   */
  getConnectionsForMemory(memoryId: string): MemoryConnection[] {
    const stmt = this.db.prepare('SELECT * FROM memory_connections');
    const rows = stmt.all() as any[];
    
    return rows
      .map(row => ({
        id: row.id,
        memoryIds: JSON.parse(row.memory_ids),
        relationship: row.relationship,
        strength: row.strength,
        createdAt: row.created_at,
      }))
      .filter(conn => conn.memoryIds.includes(memoryId));
  }

  /**
   * Delete connection
   */
  deleteConnection(id: string): boolean {
    const stmt = this.db.prepare('DELETE FROM memory_connections WHERE id = ?');
    const result = stmt.run(id);
    return result.changes > 0;
  }

  /**
   * Get connection count
   */
  getConnectionCount(): number {
    const stmt = this.db.prepare('SELECT COUNT(*) as count FROM memory_connections');
    const result = stmt.get() as { count: number };
    return result.count;
  }

  // ==================== INSIGHT OPERATIONS ====================

  /**
   * Create an insight
   */
  createInsight(insight: Omit<Insight, 'id' | 'createdAt' | 'updatedAt'>): Insight {
    const id = uuidv4();
    const now = new Date().toISOString();
    
    const stmt = this.db.prepare(`
      INSERT INTO insights (id, title, content, memory_ids, topics, confidence, created_at, updated_at)
      VALUES (?, ?, ?, ?, ?, ?, ?, ?)
    `);

    stmt.run(
      id,
      insight.title,
      insight.content,
      JSON.stringify(insight.memoryIds),
      JSON.stringify(insight.topics),
      insight.confidence,
      now,
      now
    );

    return {
      ...insight,
      id,
      createdAt: now,
      updatedAt: now,
    };
  }

  /**
   * Get all insights
   */
  getAllInsights(): Insight[] {
    const stmt = this.db.prepare('SELECT * FROM insights ORDER BY created_at DESC');
    const rows = stmt.all() as any[];
    
    return rows.map(row => ({
      id: row.id,
      title: row.title,
      content: row.content,
      memoryIds: JSON.parse(row.memory_ids),
      topics: JSON.parse(row.topics),
      confidence: row.confidence,
      createdAt: row.created_at,
      updatedAt: row.updated_at,
    }));
  }

  /**
   * Get insight by ID
   */
  getInsight(id: string): Insight | undefined {
    const stmt = this.db.prepare('SELECT * FROM insights WHERE id = ?');
    const row = stmt.get(id) as any;
    
    if (!row) return undefined;
    
    return {
      id: row.id,
      title: row.title,
      content: row.content,
      memoryIds: JSON.parse(row.memory_ids),
      topics: JSON.parse(row.topics),
      confidence: row.confidence,
      createdAt: row.created_at,
      updatedAt: row.updated_at,
    };
  }

  /**
   * Update insight
   */
  updateInsight(id: string, updates: Partial<Insight>): Insight | undefined {
    const existing = this.getInsight(id);
    if (!existing) return undefined;

    const fields: string[] = [];
    const values: any[] = [];

    if (updates.title !== undefined) {
      fields.push('title = ?');
      values.push(updates.title);
    }
    if (updates.content !== undefined) {
      fields.push('content = ?');
      values.push(updates.content);
    }
    if (updates.memoryIds !== undefined) {
      fields.push('memory_ids = ?');
      values.push(JSON.stringify(updates.memoryIds));
    }
    if (updates.topics !== undefined) {
      fields.push('topics = ?');
      values.push(JSON.stringify(updates.topics));
    }
    if (updates.confidence !== undefined) {
      fields.push('confidence = ?');
      values.push(updates.confidence);
    }

    fields.push('updated_at = ?');
    values.push(new Date().toISOString());

    values.push(id);

    const stmt = this.db.prepare(`
      UPDATE insights 
      SET ${fields.join(', ')} 
      WHERE id = ?
    `);

    stmt.run(...values);

    return this.getInsight(id);
  }

  /**
   * Delete insight
   */
  deleteInsight(id: string): boolean {
    const stmt = this.db.prepare('DELETE FROM insights WHERE id = ?');
    const result = stmt.run(id);
    return result.changes > 0;
  }

  /**
   * Get insight count
   */
  getInsightCount(): number {
    const stmt = this.db.prepare('SELECT COUNT(*) as count FROM insights');
    const result = stmt.get() as { count: number };
    return result.count;
  }

  // ==================== SEARCH OPERATIONS ====================

  /**
   * Search memories by content
   */
  searchMemories(query: string): Memory[] {
    const stmt = this.db.prepare(`
      SELECT * FROM memories 
      WHERE content LIKE ? OR summary LIKE ? OR topics LIKE ? OR entities LIKE ?
      ORDER BY created_at DESC
    `);
    const searchTerm = `%${query}%`;
    const rows = stmt.all(searchTerm, searchTerm, searchTerm, searchTerm) as any[];
    
    return rows.map(row => this.rowToMemory(row));
  }

  /**
   * Search insights by content
   */
  searchInsights(query: string): Insight[] {
    const stmt = this.db.prepare(`
      SELECT * FROM insights 
      WHERE title LIKE ? OR content LIKE ? OR topics LIKE ?
      ORDER BY created_at DESC
    `);
    const searchTerm = `%${query}%`;
    const rows = stmt.all(searchTerm, searchTerm, searchTerm) as any[];
    
    return rows.map(row => ({
      id: row.id,
      title: row.title,
      content: row.content,
      memoryIds: JSON.parse(row.memory_ids),
      topics: JSON.parse(row.topics),
      confidence: row.confidence,
      createdAt: row.created_at,
      updatedAt: row.updated_at,
    }));
  }

  // ==================== UTILITY METHODS ====================

  /**
   * Convert database row to Memory object
   */
  private rowToMemory(row: any): Memory {
    return {
      id: row.id,
      content: row.content,
      summary: row.summary,
      entities: JSON.parse(row.entities),
      topics: JSON.parse(row.topics),
      importance: row.importance as MemoryImportance,
      status: row.status as MemoryStatus,
      source: row.source,
      sourceType: row.source_type as FileType,
      createdAt: row.created_at,
      updatedAt: row.updated_at,
      consolidatedAt: row.consolidated_at || undefined,
      metadata: JSON.parse(row.metadata),
    };
  }

  /**
   * Close database connection
   */
  close(): void {
    this.db.close();
  }

  /**
   * Get database statistics
   */
  getStats(): {
    memoryCount: number;
    insightCount: number;
    connectionCount: number;
    statusBreakdown: Record<MemoryStatus, number>;
  } {
    const memoryCount = this.getMemoryCount();
    const insightCount = this.getInsightCount();
    const connectionCount = this.getConnectionCount();
    
    const statusBreakdown = {
      raw: this.getMemoriesByStatus('raw').length,
      processed: this.getMemoriesByStatus('processed').length,
      consolidated: this.getMemoriesByStatus('consolidated').length,
      archived: this.getMemoriesByStatus('archived').length,
    };
    
    return {
      memoryCount,
      insightCount,
      connectionCount,
      statusBreakdown,
    };
  }
}
