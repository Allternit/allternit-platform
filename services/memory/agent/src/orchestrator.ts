/**
 * Memory Orchestrator - Root Agent
 * 
 * Coordinates Ingest, Consolidate, and Query agents
 * Provides unified API for the memory system
 */

import * as path from 'path';
import * as fs from 'fs/promises';
import { LocalModelManager } from './models/local-model.js';
import { MemoryStore } from './store/sqlite-store.js';
import { IngestAgent } from './ingest-agent.js';
import { ConsolidateAgent } from './consolidate-agent.js';
import { QueryAgent } from './query-agent.js';
import type {
  MemoryAgentConfig,
  IngestResult,
  QueryResult,
  HealthStatus,
} from './types/memory.types.js';
import {
  DEFAULT_CONFIG,
} from './types/memory.types.js';
import type { ConsolidationResult } from './consolidate-agent.js';

/**
 * Memory Orchestrator class
 * 
 * Root agent that coordinates all sub-agents
 */
export class MemoryOrchestrator {
  private config: MemoryAgentConfig;
  private modelManager!: LocalModelManager;
  private store!: MemoryStore;
  private ingestAgent!: IngestAgent;
  private consolidateAgent!: ConsolidateAgent;
  private queryAgent!: QueryAgent;
  private isInitialized: boolean = false;
  private isRunning: boolean = false;

  constructor(config?: Partial<MemoryAgentConfig>) {
    this.config = {
      ...DEFAULT_CONFIG,
      ...config,
    };
  }

  /**
   * Expose the model manager for diagnostics/metrics endpoints.
   */
  getModelManager(): LocalModelManager {
    return this.modelManager;
  }

  /**
   * Initialize all components
   */
  async initialize(): Promise<void> {
    if (this.isInitialized) {
      console.log('MemoryOrchestrator: Already initialized');
      return;
    }

    console.log('MemoryOrchestrator: Initializing...');

    try {
      // Initialize model manager
      this.modelManager = new LocalModelManager(
        this.config.ollamaHost,
        this.config.ollamaPort
      );

      // Check Ollama connection
      const ollamaRunning = await this.modelManager.isRunning();
      if (!ollamaRunning) {
        console.warn('MemoryOrchestrator: Ollama is not running. Some features will be limited.');
        console.warn('MemoryOrchestrator: Start Ollama with: ollama serve');
      } else {
        console.log('MemoryOrchestrator: Connected to Ollama');
        
        // Ensure required models are available
        await this.ensureModels();
      }

      // Initialize store
      const dbPath = path.resolve(this.config.databasePath);
      const dbDir = path.dirname(dbPath);
      
      // Ensure database directory exists
      try {
        await fs.access(dbDir);
      } catch {
        await fs.mkdir(dbDir, { recursive: true });
        console.log(`MemoryOrchestrator: Created database directory: ${dbDir}`);
      }

      this.store = new MemoryStore(dbPath);
      console.log(`MemoryOrchestrator: Database initialized at ${dbPath}`);

      // Initialize agents
      this.ingestAgent = new IngestAgent(
        this.modelManager,
        this.store,
        {
          watchDirectory: path.resolve(this.config.watchDirectory),
        }
      );

      this.consolidateAgent = new ConsolidateAgent(
        this.modelManager,
        this.store,
        {
          intervalMinutes: this.config.consolidationIntervalMinutes,
          minMemoriesForConsolidation: 3,
          maxMemoriesPerBatch: 20,
        }
      );

      this.queryAgent = new QueryAgent(
        this.modelManager,
        this.store,
        {
          maxResults: 10,
          minConfidence: 0.5,
          includeInsights: true,
        }
      );

      this.isInitialized = true;
      console.log('MemoryOrchestrator: Initialization complete');
    } catch (error) {
      console.error('MemoryOrchestrator: Initialization failed:', error);
      throw error;
    }
  }

  /**
   * Ensure required models are available
   */
  private async ensureModels(): Promise<void> {
    const models = [
      this.config.ingestModel,
      this.config.consolidateModel,
      this.config.queryModel,
    ];

    for (const model of models) {
      await this.modelManager.ensureModel(model);
    }
  }

  /**
   * Start the memory agent system
   */
  async start(): Promise<void> {
    if (!this.isInitialized) {
      await this.initialize();
    }

    if (this.isRunning) {
      console.log('MemoryOrchestrator: Already running');
      return;
    }

    console.log('MemoryOrchestrator: Starting all agents...');

    this.isRunning = true;

    // Start ingest agent (file watcher)
    await this.ingestAgent.startWatching();

    // Process any existing files in inbox
    await this.ingestAgent.processExistingFiles();

    // Start consolidation agent (periodic)
    this.consolidateAgent.start();

    console.log('MemoryOrchestrator: All agents started');
    console.log('MemoryOrchestrator: System is now running 24/7');
  }

  /**
   * Stop the memory agent system
   */
  async stop(): Promise<void> {
    if (!this.isRunning) {
      return;
    }

    console.log('MemoryOrchestrator: Stopping all agents...');

    this.isRunning = false;

    // Stop agents
    await this.ingestAgent.stopWatching();
    this.consolidateAgent.stop();

    // Close store
    this.store.close();

    console.log('MemoryOrchestrator: All agents stopped');
  }

  // ==================== PUBLIC API ====================

  /**
   * Ingest content directly
   */
  async ingest(
    content: string,
    source?: string,
    metadata?: Record<string, unknown>
  ): Promise<IngestResult> {
    if (!this.isInitialized) {
      await this.initialize();
    }

    return this.ingestAgent.ingestContent({
      content,
      source: source || 'direct-input',
      metadata,
    });
  }

  /**
   * Query the memory system
   */
  async query(question: string): Promise<QueryResult> {
    if (!this.isInitialized) {
      await this.initialize();
    }

    return this.queryAgent.query(question);
  }

  /**
   * Trigger manual consolidation
   */
  async consolidate(): Promise<ConsolidationResult> {
    if (!this.isInitialized) {
      await this.initialize();
    }

    return this.consolidateAgent.consolidateNow();
  }

  /**
   * Search memories
   */
  search(query: string) {
    if (!this.isInitialized) {
      this.initialize();
    }

    return this.queryAgent.search(query);
  }

  /**
   * Get recent memories
   */
  getRecent(limit: number = 10) {
    if (!this.isInitialized) {
      this.initialize();
    }

    return this.queryAgent.getRecent(limit);
  }

  /**
   * Get all insights
   */
  getInsights() {
    if (!this.isInitialized) {
      this.initialize();
    }

    return this.queryAgent.getInsights();
  }

  /**
   * Get system health status
   */
  async getHealthStatus(): Promise<HealthStatus> {
    const ollamaConnected = this.modelManager 
      ? await this.modelManager.isRunning()
      : false;

    const storeStats = this.store?.getStats() || {
      memoryCount: 0,
      insightCount: 0,
      connectionCount: 0,
      statusBreakdown: { raw: 0, processed: 0, consolidated: 0, archived: 0 },
    };

    const ingestStats = this.ingestAgent?.getStats() || {
      processedFilesCount: 0,
      activeWatchers: false,
    };

    // consolidateStats available via getStats() if needed

    // Determine overall status
    let status: HealthStatus['status'] = 'healthy';
    
    if (!ollamaConnected) {
      status = 'degraded';
      console.warn('MemoryOrchestrator: Ollama not connected - operating in degraded mode');
    }

    if (!ingestStats.activeWatchers) {
      status = 'degraded';
    }

    return {
      status,
      ollamaConnected,
      databaseConnected: true,
      watcherActive: ingestStats.activeWatchers,
      lastConsolidation: undefined, // Could track this
      memoryCount: storeStats.memoryCount,
      insightCount: storeStats.insightCount,
      connectionCount: storeStats.connectionCount,
    };
  }

  /**
   * Get system statistics
   */
  getStats(): {
    memories: {
      total: number;
      raw: number;
      processed: number;
      consolidated: number;
      archived: number;
    };
    insights: number;
    connections: number;
    processedFiles: number;
    ollamaConnected: boolean;
  } {
    const storeStats = this.store?.getStats() || {
      memoryCount: 0,
      insightCount: 0,
      connectionCount: 0,
      statusBreakdown: { raw: 0, processed: 0, consolidated: 0, archived: 0 },
    };

    const ingestStats = this.ingestAgent?.getStats() || {
      processedFilesCount: 0,
      activeWatchers: false,
    };

    return {
      memories: {
        total: storeStats.memoryCount,
        raw: storeStats.statusBreakdown.raw,
        processed: storeStats.statusBreakdown.processed,
        consolidated: storeStats.statusBreakdown.consolidated,
        archived: storeStats.statusBreakdown.archived,
      },
      insights: storeStats.insightCount,
      connections: storeStats.connectionCount,
      processedFiles: ingestStats.processedFilesCount,
      ollamaConnected: this.modelManager ? true : false,
    };
  }

  /**
   * Delete a memory by ID
   */
  deleteMemory(memoryId: string): boolean {
    if (!this.isInitialized) {
      this.initialize();
    }

    return this.store.deleteMemory(memoryId);
  }

  /**
   * Clear all memories (full reset)
   */
  clearAllMemories(): number {
    if (!this.isInitialized) {
      this.initialize();
    }

    return this.store.deleteAllMemories();
  }

  /**
   * Get orchestrator state
   */
  getState(): {
    isInitialized: boolean;
    isRunning: boolean;
    config: MemoryAgentConfig;
  } {
    return {
      isInitialized: this.isInitialized,
      isRunning: this.isRunning,
      config: this.config,
    };
  }
}

/**
 * Create and start a memory orchestrator instance
 */
export async function createMemoryOrchestrator(
  config?: Partial<MemoryAgentConfig>
): Promise<MemoryOrchestrator> {
  const orchestrator = new MemoryOrchestrator(config);
  await orchestrator.start();
  return orchestrator;
}

// ==================== CLI ENTRY POINT ====================

// Run as standalone if executed directly
if (import.meta.url === `file://${process.argv[1]}`) {
  (async () => {
    console.log('╔══════════════════════════════════════════════════════════╗');
    console.log('║     Allternit Always-On Memory Agent                    ║');
    console.log('║     Local LLM Powered (Ollama)                           ║');
    console.log('╚══════════════════════════════════════════════════════════╝');
    console.log('');

    const orchestrator = new MemoryOrchestrator();

    // Handle shutdown signals
    const shutdown = async () => {
      console.log('\nMemoryOrchestrator: Shutting down...');
      await orchestrator.stop();
      process.exit(0);
    };

    process.on('SIGINT', shutdown);
    process.on('SIGTERM', shutdown);

    try {
      await orchestrator.start();

      console.log('');
      console.log('System Status:');
      console.log('  - Watch Directory: ./inbox');
      console.log('  - Database: ./memory.db');
      console.log('  - Consolidation: Every 30 minutes');
      console.log('');
      console.log('Usage:');
      console.log('  - Drop files in ./inbox to ingest');
      console.log('  - Query: await orchestrator.query("your question")');
      console.log('  - Stats: orchestrator.getStats()');
      console.log('');
      console.log('Press Ctrl+C to stop');
    } catch (error) {
      console.error('Failed to start Memory Orchestrator:', error);
      process.exit(1);
    }
  })();
}
