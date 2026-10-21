/**
 * Memory Agent HTTP API Server
 * 
 * Provides REST API for memory operations
 * Port: 3201
 */

import express from 'express';
import cors from 'cors';
import { MemoryOrchestrator } from './orchestrator.js';
import type { IngestRequest, QueryRequest } from './types/memory.types.js';
import type { ModelConfig } from './models/local-model.js';
import { VectorStore } from './store/vector-store.js';
import { observability } from './utils/observability.js';

const app: express.Application = express();
const PORT = process.env.MEMORY_HTTP_PORT || 3201;

// Middleware
app.use(cors());
app.use(express.json({ limit: '10mb' }));

// Initialize orchestrator
const orchestrator = new MemoryOrchestrator();
let isInitialized = false;
const vectorStore = new VectorStore();

/**
 * Initialize memory agent
 */
async function initializeMemory(): Promise<void> {
  if (!isInitialized) {
    console.log('HTTP API: Initializing memory agent...');
    await orchestrator.initialize();
    isInitialized = true;
    console.log('HTTP API: Memory agent initialized');
  }
}

// ==================== Health & Status ====================

app.get('/health', async (req, res) => {
  try {
    await initializeMemory();
    const health = await orchestrator.getHealthStatus();
    
    const checks = {
      ollama: health.ollamaConnected,
      database: health.databaseConnected,
      fileWatcher: health.watcherActive,
      httpServer: true,
    };
    
    const healthWithUptime = observability.getHealthCheck(checks);
    
    res.json({
      ...healthWithUptime,
      memory: health,
    });
  } catch (error) {
    res.status(500).json({
      status: 'unhealthy',
      error: error instanceof Error ? error.message : 'Unknown error',
      timestamp: new Date().toISOString(),
    });
  }
});

app.get('/stats', async (req, res) => {
  try {
    await initializeMemory();
    const stats = orchestrator.getStats();
    
    // Update observability metrics
    observability.updateStorageMetrics(
      stats.memories.total,
      stats.insights,
      stats.connections,
      0 // Would calculate actual DB size
    );
    
    res.json(stats);
  } catch (error) {
    res.status(500).json({
      error: error instanceof Error ? error.message : 'Unknown error',
    });
  }
});

/**
 * Metrics endpoint (Prometheus format)
 */
app.get('/metrics', (req, res) => {
  res.set('Content-Type', 'text/plain');
  res.send(observability.toPrometheusFormat());
});

/**
 * Per-backend generation metrics (MLX vs Ollama)
 */
app.get('/metrics/backends', async (req, res) => {
  try {
    await initializeMemory();
    const metrics = orchestrator.getModelManager().getMetrics();
    res.json(metrics);
  } catch (error) {
    res.status(500).json({
      error: error instanceof Error ? error.message : 'Failed to get backend metrics',
    });
  }
});

/**
 * Shadow comparison: run the same prompt through MLX and Ollama.
 * POST /shadow-compare
 * Body: { prompt, system_prompt?, temperature?, top_p?, max_tokens? }
 *
 * Inputs are capped and model_config is restricted to a small allowlist so the
 * endpoint cannot monopolize the single-threaded MLX queue with an oversized
 * prompt or an unbounded generation request.
 */
app.post('/shadow-compare', async (req, res) => {
  try {
    await initializeMemory();
    const { prompt, system_prompt, temperature, top_p, max_tokens } = req.body;

    if (!prompt || typeof prompt !== 'string') {
      return res.status(400).json({ error: 'prompt is required' });
    }
    if (prompt.length > 20000) {
      return res.status(400).json({ error: 'prompt exceeds 20000 characters' });
    }
    if (system_prompt && (typeof system_prompt !== 'string' || system_prompt.length > 5000)) {
      return res.status(400).json({ error: 'system_prompt invalid or exceeds 5000 characters' });
    }

    const modelConfig: Partial<ModelConfig> = {};
    if (typeof temperature === 'number') {
      modelConfig.temperature = Math.max(0, Math.min(2, temperature));
    }
    if (typeof top_p === 'number') {
      modelConfig.topP = Math.max(0, Math.min(1, top_p));
    }
    if (typeof max_tokens === 'number') {
      modelConfig.numPredict = Math.max(1, Math.min(4096, Math.floor(max_tokens)));
    }

    const result = await orchestrator.getModelManager().shadowCompare(
      prompt,
      system_prompt,
      Object.keys(modelConfig).length > 0 ? modelConfig : undefined
    );

    res.json(result);
  } catch (error) {
    res.status(500).json({
      error: error instanceof Error ? error.message : 'Shadow comparison failed',
    });
  }
});

/**
 * Detailed observability metrics
 */
app.get('/observability', (req, res) => {
  const metrics = observability.getMetrics();
  
  res.json({
    uptime_ms: metrics.uptimeMs,
    ingestion: {
      total: metrics.ingestionCount,
      errors: metrics.ingestionErrors,
      latency: observability.getLatencyPercentiles(metrics.ingestionLatencyMs),
    },
    queries: {
      total: metrics.queryCount,
      errors: metrics.queryErrors,
      latency: observability.getLatencyPercentiles(metrics.queryLatencyMs),
      cache_hit_rate: metrics.queryCount > 0
        ? metrics.queryCacheHits / metrics.queryCount
        : 0,
    },
    consolidation: {
      total: metrics.consolidationCount,
      errors: metrics.consolidationErrors,
      memories_processed: metrics.memoriesProcessed,
      connections_found: metrics.connectionsFound,
      insights_generated: metrics.insightsGenerated,
    },
    llm: {
      calls: metrics.llmCallsCount,
      errors: metrics.llmErrors,
      tokens_used: metrics.tokensUsed,
      latency: observability.getLatencyPercentiles(metrics.llmLatencyMs),
    },
    storage: {
      memories: metrics.memoryCount,
      insights: metrics.insightCount,
      connections: metrics.connectionCount,
      size_bytes: metrics.storageSizeBytes,
    },
  });
});

// ==================== Memory Operations ====================

/**
 * Query memory with natural language
 * POST /api/query
 */
app.post('/api/query', async (req, res) => {
  try {
    await initializeMemory();
    
    const { question, max_results = 10, tenant_id, session_id } = req.body as QueryRequest & {
      max_results?: number;
      tenant_id?: string;
      session_id?: string;
    };

    if (!question) {
      return res.status(400).json({ error: 'Question is required' });
    }

    const result = await orchestrator.query(question);
    
    res.json({
      query: result.query,
      answer: result.answer,
      memories: result.memories.slice(0, max_results),
      insights: result.insights,
      sources: result.sources,
      confidence: result.confidence,
      execution_time_ms: result.executionTimeMs,
      tenant_id,
      session_id,
    });
  } catch (error) {
    console.error('Query error:', error);
    res.status(500).json({
      error: error instanceof Error ? error.message : 'Query failed',
    });
  }
});

/**
 * Ingest content into memory
 * POST /api/ingest
 */
app.post('/api/ingest', async (req, res) => {
  try {
    await initializeMemory();
    
    const { content, source, metadata, tenant_id, session_id } = req.body as IngestRequest & {
      tenant_id?: string;
      session_id?: string;
    };

    if (!content) {
      return res.status(400).json({ error: 'Content is required' });
    }

    const enrichedSource = source 
      ? `${source}${tenant_id ? `:${tenant_id}` : ''}${session_id ? `:${session_id}` : ''}`
      : tenant_id || session_id || 'direct-input';

    const result = await orchestrator.ingest(content, enrichedSource, metadata);

    if (result.success) {
      // Populate the in-memory vector index so semantic search can find the memory
      if (result.memory) {
        await vectorStore.addMemory(result.memory);
      }

      res.status(201).json({
        success: true,
        memory_id: result.memoryId,
        source: enrichedSource,
        timestamp: new Date().toISOString(),
      });
    } else {
      res.status(400).json({
        success: false,
        error: result.error,
      });
    }
  } catch (error) {
    console.error('Ingest error:', error);
    res.status(500).json({
      error: error instanceof Error ? error.message : 'Ingest failed',
    });
  }
});

/**
 * Search memories (raw results, no synthesis)
 * GET /api/search?q=...
 */
app.get('/api/search', async (req, res) => {
  try {
    await initializeMemory();
    
    const { q, limit = 20, semantic = false } = req.query;
    
    if (!q) {
      return res.status(400).json({ error: 'Query parameter "q" is required' });
    }

    let memories;
    
    // Use vector search if semantic=true
    if (semantic === 'true') {
      memories = await vectorStore.similaritySearch(String(q), Number(limit));
    } else {
      memories = orchestrator.search(String(q));
    }
    
    res.json({
      query: q,
      memories: memories.slice(0, Number(limit)),
      count: memories.length,
      search_type: semantic === 'true' ? 'semantic' : 'text',
    });
  } catch (error) {
    res.status(500).json({
      error: error instanceof Error ? error.message : 'Search failed',
    });
  }
});

/**
 * Vector similarity search
 * POST /api/vector/search
 */
app.post('/api/vector/search', async (req, res) => {
  try {
    await initializeMemory();
    
    const { query, limit = 10, min_similarity = 0.5 } = req.body;
    
    if (!query) {
      return res.status(400).json({ error: 'Query is required' });
    }

    const memories = await vectorStore.similaritySearch(query, Number(limit));
    
    // Filter by minimum similarity (would need to calculate scores)
    const filteredMemories = memories; // Simplified for now
    
    res.json({
      query,
      memories: filteredMemories,
      count: filteredMemories.length,
      search_type: 'vector',
    });
  } catch (error) {
    res.status(500).json({
      error: error instanceof Error ? error.message : 'Vector search failed',
    });
  }
});

/**
 * Get vector index stats
 * GET /api/vector/stats
 */
app.get('/api/vector/stats', async (req, res) => {
  try {
    await initializeMemory();
    
    const stats = vectorStore.getStats();
    
    res.json({
      indexed_memories: stats.count,
      embedding_model: stats.model,
    });
  } catch (error) {
    res.status(500).json({
      error: error instanceof Error ? error.message : 'Failed to get vector stats',
    });
  }
});

/**
 * Get recent memories
 * GET /api/recent?limit=10
 */
app.get('/api/recent', async (req, res) => {
  try {
    await initializeMemory();
    
    const { limit = 10 } = req.query;
    const memories = orchestrator.getRecent(Number(limit));
    
    res.json({
      memories,
      count: memories.length,
    });
  } catch (error) {
    res.status(500).json({
      error: error instanceof Error ? error.message : 'Failed to get recent memories',
    });
  }
});

/**
 * Get all insights
 * GET /api/insights
 */
app.get('/api/insights', async (req, res) => {
  try {
    await initializeMemory();
    const insights = orchestrator.getInsights();
    
    res.json({
      insights,
      count: insights.length,
    });
  } catch (error) {
    res.status(500).json({
      error: error instanceof Error ? error.message : 'Failed to get insights',
    });
  }
});

/**
 * Trigger manual consolidation
 * POST /api/consolidate
 */
app.post('/api/consolidate', async (req, res) => {
  try {
    await initializeMemory();
    const result = await orchestrator.consolidate();
    
    res.json({
      success: true,
      ...result,
    });
  } catch (error) {
    res.status(500).json({
      error: error instanceof Error ? error.message : 'Consolidation failed',
    });
  }
});

/**
 * Delete a memory
 * DELETE /api/memory/:id
 */
app.delete('/api/memory/:id', async (req, res) => {
  try {
    await initializeMemory();
    const { id } = req.params;
    
    const deleted = orchestrator.deleteMemory(id);
    
    if (deleted) {
      res.json({ success: true, memory_id: id });
    } else {
      res.status(404).json({ success: false, error: 'Memory not found' });
    }
  } catch (error) {
    res.status(500).json({
      error: error instanceof Error ? error.message : 'Delete failed',
    });
  }
});

/**
 * Clear all memories (dangerous!)
 * POST /api/clear
 */
app.post('/api/clear', async (req, res) => {
  try {
    await initializeMemory();
    
    // Require confirmation header for safety
    const confirmHeader = req.headers['x-confirm-dangerous'];
    if (confirmHeader !== 'true') {
      return res.status(400).json({
        error: 'Add header X-Confirm-Dangerous: true to confirm',
      });
    }
    
    const count = orchestrator.clearAllMemories();
    
    res.json({
      success: true,
      memories_deleted: count,
    });
  } catch (error) {
    res.status(500).json({
      error: error instanceof Error ? error.message : 'Clear failed',
    });
  }
});

// ==================== Bulk Operations ====================

/**
 * Bulk ingest multiple items
 * POST /api/ingest/bulk
 */
app.post('/api/ingest/bulk', async (req, res) => {
  try {
    await initializeMemory();
    
    const { items } = req.body as {
      items: Array<{ content: string; source?: string; metadata?: Record<string, unknown> }>;
    };
    
    if (!items || !Array.isArray(items)) {
      return res.status(400).json({ error: 'Items array is required' });
    }

    const results = [];
    let successCount = 0;
    let errorCount = 0;

    for (const item of items) {
      try {
        const result = await orchestrator.ingest(
          item.content,
          item.source || 'bulk-ingest',
          item.metadata
        );

        if (result.success && result.memory) {
          await vectorStore.addMemory(result.memory);
        }

        results.push({
          source: item.source || 'bulk-ingest',
          success: result.success,
          memory_id: result.memoryId,
          error: result.error,
        });

        if (result.success) successCount++;
        else errorCount++;
      } catch (error) {
        results.push({
          source: item.source || 'bulk-ingest',
          success: false,
          error: error instanceof Error ? error.message : 'Unknown error',
        });
        errorCount++;
      }
    }

    res.json({
      total: items.length,
      success: successCount,
      errors: errorCount,
      results,
    });
  } catch (error) {
    res.status(500).json({
      error: error instanceof Error ? error.message : 'Bulk ingest failed',
    });
  }
});

// ==================== Event Streaming ====================

/**
 * Stream memory events (SSE)
 * GET /api/events
 */
app.get('/api/events', async (req, res) => {
  res.setHeader('Content-Type', 'text/event-stream');
  res.setHeader('Cache-Control', 'no-cache');
  res.setHeader('Connection', 'keep-alive');
  
  // Send initial connection event
  res.write(`data: ${JSON.stringify({ type: 'connected', timestamp: new Date().toISOString() })}\n\n`);
  
  // TODO: Implement event subscription when event bus is added
  // For now, just keep connection alive
  const keepAlive = setInterval(() => {
    res.write(': keepalive\n\n');
  }, 30000);
  
  req.on('close', () => {
    clearInterval(keepAlive);
  });
});

// ==================== Error Handling ====================

app.use((err: Error, req: express.Request, res: express.Response, next: express.NextFunction) => {
  console.error('Unhandled error:', err);
  res.status(500).json({
    error: 'Internal server error',
    message: process.env.NODE_ENV === 'development' ? err.message : undefined,
  });
});

// ==================== Server Start ====================

async function startServer(): Promise<void> {
  try {
    // Pre-initialize memory agent
    await initializeMemory();
    
    app.listen(PORT, () => {
      console.log('╔══════════════════════════════════════════════════════════╗');
      console.log('║     Allternit Memory Agent HTTP API                     ║');
      console.log('╚══════════════════════════════════════════════════════════╝');
      console.log('');
      console.log(`Server running on http://localhost:${PORT}`);
      console.log('');
      console.log('Endpoints:');
      console.log(`  GET  /health              - Health check`);
      console.log(`  GET  /stats               - Memory statistics`);
      console.log(`  GET  /metrics/backends    - MLX/Ollama generation metrics`);
      console.log(`  POST /shadow-compare      - Run prompt through both backends`);
      console.log(`  POST /api/query           - Query memory`);
      console.log(`  POST /api/ingest          - Ingest content`);
      console.log(`  POST /api/ingest/bulk     - Bulk ingest`);
      console.log(`  GET  /api/search?q=...    - Search memories`);
      console.log(`  GET  /api/recent          - Recent memories`);
      console.log(`  GET  /api/insights        - Get insights`);
      console.log(`  POST /api/consolidate     - Trigger consolidation`);
      console.log(`  DELETE /api/memory/:id    - Delete memory`);
      console.log(`  POST /api/clear           - Clear all memories`);
      console.log(`  GET  /api/events          - Event stream (SSE)`);
      console.log('');
    });
  } catch (error) {
    console.error('Failed to start HTTP server:', error);
    process.exit(1);
  }
}

// Start server if run directly
if (import.meta.url === `file://${process.argv[1]}`) {
  startServer();
}

export { app, initializeMemory };
