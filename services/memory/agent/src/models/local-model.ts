/**
 * Local Model Manager - Ollama Integration
 *
 * Handles all interactions with local Ollama models.
 *
 * Provider switch: when `MEMORY_LLM_BASE_URL` is set (e.g. an MLX
 * OpenAI-compatible server at http://localhost:8080/v1), generation tasks are
 * routed to `{base}/chat/completions` with the model from `MEMORY_LLM_MODEL`
 * (default 'qwen3-4b-instruct'). Embeddings always stay on Ollama
 * (see store/vector-store.ts).
 */

import { Ollama } from 'ollama';
import type { ChatResponse } from 'ollama';
import type { MemoryImportance } from '../types/memory.types.js';

/**
 * Optional OpenAI-compatible generation provider config.
 * Values default to the MEMORY_LLM_BASE_URL / MEMORY_LLM_MODEL env vars.
 */
export interface LLMProviderConfig {
  baseUrl?: string;
  model?: string;
}

interface ChatMessage {
  role: 'system' | 'user';
  content: string;
}

/**
 * Optional OpenAI-compatible generation provider config.
 * Values default to the MEMORY_LLM_BASE_URL / MEMORY_LLM_MODEL env vars.
 */
export interface LLMProviderConfig {
  baseUrl?: string;
  model?: string;
}

interface ChatMessage {
  role: 'system' | 'user';
  content: string;
}

/**
 * Model configuration
 */
export interface ModelConfig {
  name: string;
  temperature: number;
  topP: number;
  numPredict: number;
}

/**
 * Default model configurations optimized for different tasks
 * Using Qwen 3.5 distilled models for efficiency
 */
export const MODEL_PRESETS: Record<string, ModelConfig> = {
  // Fast summarization and extraction
  ingest: {
    name: process.env.MEMORY_INGEST_MODEL || 'qwen3.5:2b',
    temperature: 0.3,
    topP: 0.9,
    numPredict: 500,
  },
  // Single-call enrichment for normal ingest (configurable for experimentation)
  fastIngest: {
    name: process.env.MEMORY_FAST_INGEST_MODEL || process.env.MEMORY_INGEST_MODEL || 'qwen3.5:2b',
    temperature: 0.2,
    topP: 0.9,
    // 600 tokens leaves headroom for long summaries + many entities/topics on
    // large docs without materially slowing the MLX/Ollama path.
    numPredict: 600,
  },
  // Reasoning and pattern finding
  consolidate: {
    name: process.env.MEMORY_CONSOLIDATE_MODEL || 'qwen3.5:4b',
    temperature: 0.5,
    topP: 0.9,
    numPredict: 1000,
  },
  // Query synthesis
  query: {
    name: process.env.MEMORY_QUERY_MODEL || 'qwen3.5:2b',
    temperature: 0.4,
    topP: 0.9,
    numPredict: 800,
  },
  // Entity extraction (structured output)
  extract: {
    name: process.env.MEMORY_EXTRACT_MODEL || 'qwen3.5:2b',
    temperature: 0.2,
    topP: 0.9,
    numPredict: 400,
  },
  // Embeddings (still using specialized model)
  embed: {
    name: 'mxbai-embed-large',
    temperature: 0,
    topP: 1.0,
    numPredict: 0,
  },
};

type BreakerState = 'closed' | 'open' | 'half_open';

/**
 * Local Model Manager class
 */
export class LocalModelManager {
  private ollama: Ollama;
  private llmBaseUrl?: string;
  private llmModel: string;
  // MLX/local OpenAI-compatible servers (e.g. mlx_lm.server) are often
  // single-threaded and deadlock or serialize poorly under concurrent load.
  // Queue generation requests so only one hits the server at a time.
  private llmQueue: Promise<unknown> = Promise.resolve();

  // Circuit breaker for the optional MLX/OpenAI-compatible generation provider.
  // If MLX hangs or errors repeatedly, generation falls back to Ollama for a
  // cooldown window instead of queueing every remaining caller into a timeout.
  private breakerState: BreakerState = 'closed';
  private consecutiveFailures = 0;
  private lastOpenedAt = 0;
  private breakerThreshold: number;
  private breakerCooldownMs: number;
  // In half-open state, allow only one in-flight probe request to MLX at a time
  // so concurrent callers don't all rush through before the first probe resolves.
  private breakerProbeInFlight = false;

  // Per-backend generation metrics (latency, calls, failures).
  private metrics: {
    mlx: { calls: number; failures: number; totalLatencyMs: number };
    ollama: { calls: number; failures: number; totalLatencyMs: number };
    enrichment: { localFallbacks: number };
  } = {
    mlx: { calls: 0, failures: 0, totalLatencyMs: 0 },
    ollama: { calls: 0, failures: 0, totalLatencyMs: 0 },
    enrichment: { localFallbacks: 0 },
  };

  constructor(host: string = 'localhost', port: number = 11434, llm?: LLMProviderConfig) {
    this.ollama = new Ollama({ host: `http://${host}:${port}` });
    const baseUrl = llm?.baseUrl ?? process.env.MEMORY_LLM_BASE_URL;
    this.llmBaseUrl = baseUrl ? baseUrl.replace(/\/+$/, '') : undefined;
    this.llmModel = llm?.model ?? process.env.MEMORY_LLM_MODEL ?? 'qwen3-4b-instruct';
    this.breakerThreshold = Math.max(1, parseInt(process.env.MEMORY_LLM_BREAKER_THRESHOLD || '3', 10));
    this.breakerCooldownMs = Math.max(0, parseInt(process.env.MEMORY_LLM_BREAKER_COOLDOWN_MS || '60000', 10));
  }

  /**
   * Serialize an async operation. Returns a promise that resolves/rejects
   * with the operation's result, and internally chains behind any prior call.
   */
  private async serialized<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.llmQueue.then(operation);
    // Track completion (success or failure) so the next queued call waits
    // until this one finishes, but don't let a rejection break the chain.
    this.llmQueue = result.catch(() => undefined);
    return result;
  }

  /**
   * Call the configured OpenAI-compatible endpoint (MLX path).
   * Throws with endpoint + status on any failure — never falls back to
   * Ollama mid-config (R3: wrong-model answers are worse than failed ones).
   */
  private async openAIChat(messages: ChatMessage[], modelConfig: ModelConfig): Promise<string> {
    const endpoint = `${this.llmBaseUrl}/chat/completions`;

    let response: Response;
    try {
      response = await fetch(endpoint, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          model: this.llmModel,
          messages,
          temperature: modelConfig.temperature,
          top_p: modelConfig.topP,
          max_tokens: modelConfig.numPredict,
          stream: false,
        }),
        // Generations can be slow; 2m is generous without letting a hung
        // server wedge the serialized queue forever.
        signal: AbortSignal.timeout(120_000),
      });
    } catch (error) {
      throw new Error(
        `OpenAI-compatible provider unreachable at ${endpoint}: ${error instanceof Error ? error.message : 'network error'}`
      );
    }

    if (!response.ok) {
      throw new Error(`OpenAI-compatible provider error at ${endpoint}: HTTP ${response.status}`);
    }

    const data = (await response.json()) as {
      choices?: Array<{ message?: { content?: unknown } }>;
    };
    const content = data.choices?.[0]?.message?.content;
    if (typeof content !== 'string') {
      throw new Error(`OpenAI-compatible provider at ${endpoint} returned an unexpected response shape`);
    }
    return content;
  }

  /**
   * Circuit-breaker state machine. Returns true when the MLX path is currently
   * allowed to be tried (closed, or half-open with no probe in flight).
   */
  private mlxAllowed(): boolean {
    if (!this.llmBaseUrl) return false;
    if (this.breakerState === 'closed') return true;
    if (this.breakerState === 'open') {
      if (Date.now() - this.lastOpenedAt >= this.breakerCooldownMs) {
        this.breakerState = 'half_open';
      } else {
        return false;
      }
    }
    // half_open: only one caller may probe MLX at a time.
    if (this.breakerProbeInFlight) return false;
    this.breakerProbeInFlight = true;
    return true;
  }

  /**
   * Record a successful MLX call: close the breaker and reset failures.
   */
  private recordMlxSuccess(): void {
    this.breakerState = 'closed';
    this.consecutiveFailures = 0;
  }

  /**
   * Record a failed MLX call: open the breaker if the failure threshold is hit.
   */
  private recordMlxFailure(): void {
    this.consecutiveFailures += 1;
    if (this.consecutiveFailures >= this.breakerThreshold) {
      this.breakerState = 'open';
      this.lastOpenedAt = Date.now();
      console.warn(
        `LocalModelManager: MLX provider failed ${this.consecutiveFailures} consecutive times; ` +
          `falling back to Ollama for ${this.breakerCooldownMs}ms`
      );
    }
  }

  /**
   * Release the half-open probe lock so the next caller can probe MLX.
   */
  private releaseBreakerProbe(): void {
    this.breakerProbeInFlight = false;
  }

  /**
   * Record which backend served a generation call for auditability.
   */
  private logBackend(backend: 'mlx' | 'ollama'): void {
    console.log(`LocalModelManager: generation backend=${backend}`);
  }

  /**
   * Record a latency/failure sample for the given backend.
   */
  private recordLatency(backend: 'mlx' | 'ollama', latencyMs: number, failed: boolean): void {
    const bucket = this.metrics[backend];
    bucket.calls += 1;
    bucket.totalLatencyMs += latencyMs;
    if (failed) bucket.failures += 1;
  }

  /**
   * Return a snapshot of per-backend generation metrics.
   */
  getMetrics(): {
    mlx: { calls: number; failures: number; avgLatencyMs: number };
    ollama: { calls: number; failures: number; avgLatencyMs: number };
    enrichment: { localFallbacks: number };
  } {
    const avg = (bucket: { calls: number; totalLatencyMs: number }) =>
      bucket.calls > 0 ? bucket.totalLatencyMs / bucket.calls : 0;
    return {
      mlx: { ...this.metrics.mlx, avgLatencyMs: avg(this.metrics.mlx) },
      ollama: { ...this.metrics.ollama, avgLatencyMs: avg(this.metrics.ollama) },
      enrichment: { ...this.metrics.enrichment },
    };
  }

  /**
   * Synchronous Ollama chat helper used by both the default path and the MLX
   * circuit-breaker fallback.
   */
  private async ollamaChat(messages: ChatMessage[], modelConfig: ModelConfig): Promise<string> {
    const response: ChatResponse = await this.ollama.chat({
      model: modelConfig.name,
      messages,
      options: {
        temperature: modelConfig.temperature,
        top_p: modelConfig.topP,
        num_predict: modelConfig.numPredict,
      },
    });
    return response.message.content;
  }

  /**
   * Check if Ollama is running and accessible
   */
  async isRunning(): Promise<boolean> {
    try {
      await this.ollama.list();
      return true;
    } catch (error) {
      return false;
    }
  }

  /**
   * Get list of available models
   */
  async listModels(): Promise<string[]> {
    try {
      const response = await this.ollama.list();
      return response.models.map((m: { name: string }) => m.name);
    } catch (error) {
      console.error('Error listing models:', error);
      return [];
    }
  }

  /**
   * Pull a model if not already installed.
   * When an OpenAI-compatible generation provider (e.g. MLX) is configured, the
   * provider's model is server-managed and we skip the pull. However, if the
   * requested model name looks like an Ollama tag (no '/' namespace), ensure it
   * is available as a circuit-breaker fallback.
   */
  async ensureModel(modelName: string): Promise<boolean> {
    const isOllamaTag = !modelName.includes('/');
    if (this.llmBaseUrl && !isOllamaTag) {
      return true;
    }
    try {
      const models = await this.listModels();
      if (models.some(m => m.startsWith(modelName))) {
        return true;
      }

      console.log(`Pulling model: ${modelName}...`);
      await this.ollama.pull({ model: modelName });
      console.log(`Model ${modelName} pulled successfully`);
      return true;
    } catch (error) {
      console.error(`Error pulling model ${modelName}:`, error);
      return false;
    }
  }

  /**
   * Generate a response and report which backend served it.
   * When MEMORY_LLM_BASE_URL is set, generation tries the MLX/OpenAI-compatible
   * provider first. If that provider fails repeatedly, a circuit breaker trips
   * and generation falls back to Ollama for a cooldown window.
   */
  private async generateWithBackend(
    prompt: string,
    systemPrompt?: string,
    config?: Partial<ModelConfig>
  ): Promise<{ content: string; backend: 'mlx' | 'ollama' }> {
    const modelConfig = config?.name
      ? { ...MODEL_PRESETS.ingest, ...config }
      : MODEL_PRESETS.ingest;

    const messages: ChatMessage[] = [
      ...(systemPrompt ? [{ role: 'system' as const, content: systemPrompt }] : []),
      { role: 'user' as const, content: prompt },
    ];

    const startMs = Date.now();
    const probingMlx = this.mlxAllowed();
    if (probingMlx) {
      try {
        const result = await this.serialized(() => this.openAIChat(messages, modelConfig));
        this.recordMlxSuccess();
        this.logBackend('mlx');
        this.recordLatency('mlx', Date.now() - startMs, false);
        return { content: result, backend: 'mlx' };
      } catch (error) {
        this.recordMlxFailure();
        this.recordLatency('mlx', Date.now() - startMs, true);
        console.warn('LocalModelManager: MLX generation failed, trying Ollama fallback:', error);
      } finally {
        this.releaseBreakerProbe();
      }
    }

    this.logBackend('ollama');
    let startOllamaMs = Date.now();
    try {
      const result = await this.ollamaChat(messages, modelConfig);
      this.recordLatency('ollama', Date.now() - startOllamaMs, false);
      return { content: result, backend: 'ollama' };
    } catch (error) {
      this.recordLatency('ollama', Date.now() - startOllamaMs, true);
      console.error('Error generating response:', error);
      throw new Error(`Model generation failed: ${error instanceof Error ? error.message : 'Unknown error'}`);
    }
  }

  /**
   * Generate a response from the model.
   */
  async generate(
    prompt: string,
    systemPrompt?: string,
    config?: Partial<ModelConfig>
  ): Promise<string> {
    const { content } = await this.generateWithBackend(prompt, systemPrompt, config);
    return content;
  }

  /**
   * Generate with streaming (for long responses).
   * Mirrors the circuit-breaker behavior of generate(): tries MLX first, falls
   * back to Ollama if the breaker is open. The MLX path is non-streaming at the
   * protocol level and yielded as a single chunk.
   */
  async *generateStream(
    prompt: string,
    systemPrompt?: string,
    config?: Partial<ModelConfig>
  ): AsyncGenerator<string> {
    const modelConfig = config?.name
      ? { ...MODEL_PRESETS.ingest, ...config }
      : MODEL_PRESETS.ingest;

    const messages: ChatMessage[] = [
      ...(systemPrompt ? [{ role: 'system' as const, content: systemPrompt }] : []),
      { role: 'user' as const, content: prompt },
    ];

    const probingMlx = this.mlxAllowed();
    if (probingMlx) {
      try {
        const result = await this.serialized(() => this.openAIChat(messages, modelConfig));
        this.recordMlxSuccess();
        this.logBackend('mlx');
        yield result;
        return;
      } catch (error) {
        this.recordMlxFailure();
        console.warn('LocalModelManager: MLX streaming failed, trying Ollama fallback:', error);
      } finally {
        this.releaseBreakerProbe();
      }
    }

    this.logBackend('ollama');
    try {
      const stream = await this.ollama.chat({
        model: modelConfig.name,
        messages,
        options: {
          temperature: modelConfig.temperature,
          top_p: modelConfig.topP,
          num_predict: modelConfig.numPredict,
        },
        stream: true,
      });

      for await (const part of stream as unknown as AsyncIterable<{ message?: { content?: string } }>) {
        if (part.message?.content) {
          yield part.message.content;
        }
      }
    } catch (error) {
      console.error('Error streaming response:', error);
      throw new Error(`Model streaming failed: ${error instanceof Error ? error.message : 'Unknown error'}`);
    }
  }

  /**
   * Run the same prompt against both MLX and Ollama generation backends and
   * return both responses for comparison. This bypasses the circuit breaker so
   * it can be used for shadow-mode quality monitoring even when MLX is failing.
   */
  async shadowCompare(
    prompt: string,
    systemPrompt?: string,
    config?: Partial<ModelConfig>
  ): Promise<{
    mlx?: { content: string; latencyMs: number };
    ollama?: { content: string; latencyMs: number };
  }> {
    const modelConfig = config?.name
      ? { ...MODEL_PRESETS.ingest, ...config }
      : MODEL_PRESETS.ingest;

    const messages: ChatMessage[] = [
      ...(systemPrompt ? [{ role: 'system' as const, content: systemPrompt }] : []),
      { role: 'user' as const, content: prompt },
    ];

    const result: {
      mlx?: { content: string; latencyMs: number };
      ollama?: { content: string; latencyMs: number };
    } = {};

    if (this.llmBaseUrl) {
      const startMs = Date.now();
      try {
        const content = await this.serialized(() => this.openAIChat(messages, modelConfig));
        result.mlx = { content, latencyMs: Date.now() - startMs };
      } catch (error) {
        console.warn('LocalModelManager: shadow MLX comparison failed:', error);
      }
    }

    const startMs = Date.now();
    try {
      const content = await this.ollamaChat(messages, modelConfig);
      result.ollama = { content, latencyMs: Date.now() - startMs };
    } catch (error) {
      console.warn('LocalModelManager: shadow Ollama comparison failed:', error);
    }

    return result;
  }

  /**
   * Extract structured data using the model
   * Returns JSON-parseable object
   */
  async extractStructured<T>(
    prompt: string,
    schema: string,
    config?: Partial<ModelConfig>
  ): Promise<T | null> {
    const systemPrompt = `You are a data extraction assistant. 
Extract information from the input and format it as valid JSON according to this schema:
${schema}

Respond ONLY with valid JSON. No explanations, no markdown, no extra text.`;

    try {
      const response = await this.generate(prompt, systemPrompt, {
        ...config,
        temperature: 0.1, // Low temperature for consistent structured output
      });

      // Clean up response (remove markdown code blocks if present)
      const cleanedResponse = response
        .replace(/```json\s*/g, '')
        .replace(/```\s*/g, '')
        .trim();

      return JSON.parse(cleanedResponse) as T;
    } catch (error) {
      console.error('Error extracting structured data:', error);
      return null;
    }
  }

  /**
   * Summarize text
   */
  async summarize(text: string, maxLength: number = 200): Promise<string> {
    const prompt = `Summarize the following text in ${maxLength} words or less. 
Focus on the key information and main points:

${text}`;

    return await this.generate(prompt, undefined, MODEL_PRESETS.ingest);
  }

  /**
   * Extract entities from text
   */
  async extractEntities(text: string): Promise<{ entities: string[]; topics: string[] }> {
    const schema = `{
  "entities": ["list of named entities (people, places, organizations, products, etc.)"],
  "topics": ["list of 3-5 main topics or themes"]
}`;

    const prompt = `Analyze the following text and extract named entities and main topics:

${text}

Return your analysis as JSON.`;

    const result = await this.extractStructured<{ entities: string[]; topics: string[] }>(
      prompt,
      schema,
      MODEL_PRESETS.extract
    );

    return result || { entities: [], topics: [] };
  }

  /**
   * Assess importance of content
   */
  async assessImportance(text: string): Promise<'low' | 'medium' | 'high' | 'critical'> {
    const prompt = `Assess the importance of this content on a scale of:
- low: Routine information, everyday details
- medium: Notable information, somewhat significant
- high: Important information, significant insights
- critical: Crucial information, major decisions, key learnings

Content:
${text}

Respond with ONLY one word: low, medium, high, or critical`;

    try {
      const response = await this.generate(prompt, undefined, {
        ...MODEL_PRESETS.extract,
        numPredict: 50,
      });

      const importance = response.trim().toLowerCase() as 'low' | 'medium' | 'high' | 'critical';
      
      // Validate response
      if (['low', 'medium', 'high', 'critical'].includes(importance)) {
        return importance;
      }
      
      return 'medium'; // Default
    } catch (error) {
      console.error('Error assessing importance:', error);
      return 'medium';
    }
  }

  /**
   * Validate and sanitize the structured enrichment result.
   * Returns null if the result is missing required fields or has wrong types.
   */
  private validateEnrichment(
    result: Record<string, unknown>,
    maxLength: number
  ): { summary: string; entities: string[]; topics: string[]; importance: MemoryImportance } | null {
    if (!result || typeof result !== 'object') return null;

    const rawSummary = result.summary;
    if (typeof rawSummary !== 'string' || rawSummary.trim().length === 0) return null;

    const rawEntities = result.entities;
    const rawTopics = result.topics;
    if (!Array.isArray(rawEntities) || !Array.isArray(rawTopics)) return null;

    const entities = rawEntities.filter((e): e is string => typeof e === 'string');
    const topics = rawTopics.filter((t): t is string => typeof t === 'string');

    const rawImportance = String(result.importance || 'medium').trim().toLowerCase();
    const importance: MemoryImportance = ['low', 'medium', 'high', 'critical'].includes(rawImportance)
      ? (rawImportance as MemoryImportance)
      : 'medium';

    // Bound summary length so downstream storage/query prompts stay predictable.
    const summary = rawSummary.trim().slice(0, Math.max(80, maxLength * 8));

    return { summary, entities, topics, importance };
  }

  /**
   * Fast local fallback when the LLM fails to return valid structured JSON.
   * Avoids the old three-call fallback so the speedup isn't lost on failure.
   */
  private localEnrichmentFallback(
    text: string,
    maxLength: number
  ): { summary: string; entities: string[]; topics: string[]; importance: MemoryImportance } {
    const sentences = text
      .replace(/\s+/g, ' ')
      .split(/(?<=[.!?])\s+/)
      .filter(s => s.trim().length > 0);

    const targetChars = Math.max(120, maxLength * 8);
    let summary = '';
    for (const sentence of sentences) {
      if (summary.length + sentence.length > targetChars && summary.length > 0) break;
      summary += (summary ? ' ' : '') + sentence.trim();
    }

    // Crude keyword extraction: capitalized phrases and alphanumeric tokens > 5 chars.
    const entityMatches = text.match(/\b[A-Z][a-zA-Z]+(?:\s+[A-Z][a-zA-Z]+)*\b/g) || [];
    const entities = Array.from(new Set(entityMatches.filter(e => e.length > 3).slice(0, 10)));

    const topicMatches = text.toLowerCase().match(/\b[a-z]{6,}\b/g) || [];
    const topicFreq = new Map<string, number>();
    for (const t of topicMatches) {
      if (['because', 'through', 'between', 'however', 'therefore', 'following'].includes(t)) continue;
      topicFreq.set(t, (topicFreq.get(t) || 0) + 1);
    }
    const topics = Array.from(topicFreq.entries())
      .sort((a, b) => b[1] - a[1])
      .slice(0, 5)
      .map(([t]) => t);

    // Heuristic importance: longer, decision/learning-heavy docs rank higher.
    const lower = text.toLowerCase();
    const hasCritical = /critical|crucial|block|outage|breach|rollback/i.test(lower);
    const hasDecision = /decision|adopted|rejected|approved|roadmap|milestone/i.test(lower);
    const importance: MemoryImportance = hasCritical
      ? 'critical'
      : hasDecision || text.length > 5000
        ? 'high'
        : text.length > 1000
          ? 'medium'
          : 'low';

    return { summary: summary.trim() || text.slice(0, targetChars), entities, topics, importance };
  }

  /**
   * Single-call enrichment: summary + entities + topics + importance + backend.
   * Faster than the separate calls because the long document prompt is
   * processed once instead of three times. If the LLM returns malformed or
   * incomplete structured output, a fast local fallback keeps ingestion moving
   * without reverting to three extra LLM calls.
   */
  async enrichContent(
    text: string,
    maxLength: number = 150
  ): Promise<{
    summary: string;
    entities: string[];
    topics: string[];
    importance: MemoryImportance;
    backend: 'mlx' | 'ollama' | 'local';
  }> {
    const schema = `{
  "summary": "string, ${maxLength} words or less",
  "entities": ["named entities (people, places, organizations, products, etc.)"],
  "topics": ["3-5 main topics or themes"],
  "importance": "one of: low, medium, high, critical"
}`;

    const systemPrompt = `You are a data extraction assistant.
Extract information from the input and format it as valid JSON according to this schema:
${schema}

Respond ONLY with valid JSON. No explanations, no markdown, no extra text.`;

    const prompt = `Analyze the following text and return a single JSON object with this schema:
${schema}

Text:
${text}

Respond ONLY with valid JSON. No explanations, no markdown, no extra text.`;

    try {
      const { content, backend } = await this.generateWithBackend(prompt, systemPrompt, {
        ...MODEL_PRESETS.fastIngest,
        temperature: 0.1, // Low temperature for consistent structured output
      });

      const cleaned = content.replace(/```json\s*/g, '').replace(/```\s*/g, '').trim();
      const raw = JSON.parse(cleaned) as Record<string, unknown>;
      const validated = this.validateEnrichment(raw, maxLength);
      if (validated) {
        return { ...validated, backend };
      }
    } catch (error) {
      console.error('LocalModelManager: enrichment structured call failed:', error);
    }

    console.log('LocalModelManager: using fast local enrichment fallback');
    this.metrics.enrichment.localFallbacks += 1;
    return { ...this.localEnrichmentFallback(text, maxLength), backend: 'local' };
  }

  /**
   * Find connections between memories
   */
  async findConnections(memories: Array<{ id: string; summary: string; topics: string[] }>): Promise<
    Array<{
      memoryId1: string;
      memoryId2: string;
      relationship: string;
      strength: number;
    }>
  > {
    const schema = `{
  "connections": [
    {
      "memoryId1": "id of first memory",
      "memoryId2": "id of second memory", 
      "relationship": "description of how they relate",
      "strength": 0.0-1.0 confidence score
    }
  ]
}`;

    const memoriesText = memories
      .map(m => `- ${m.id}: ${m.summary} [Topics: ${m.topics.join(', ')}]`)
      .join('\n');

    const prompt = `Find meaningful connections between these memories:

${memoriesText}

Look for:
- Shared topics or themes
- Causal relationships
- Temporal connections
- Complementary information
- Contradictions or tensions

Return your analysis as JSON.`;

    const result = await this.extractStructured<{
      connections: Array<{
        memoryId1: string;
        memoryId2: string;
        relationship: string;
        strength: number;
      }>;
    }>(prompt, schema, MODEL_PRESETS.consolidate);

    return result?.connections || [];
  }

  /**
   * Generate insights from multiple memories
   */
  async generateInsights(
    memories: Array<{ id: string; summary: string; content: string }>
  ): Promise<Array<{ title: string; content: string; confidence: number }>> {
    const schema = `{
  "insights": [
    {
      "title": "Short descriptive title",
      "content": "The insight or pattern discovered",
      "confidence": 0.0-1.0 confidence score
    }
  ]
}`;

    const memoriesText = memories
      .map(m => `---\nMemory ${m.id}:\n${m.content}`)
      .join('\n');

    const prompt = `Analyze these memories and identify cross-cutting insights, patterns, and themes:

${memoriesText}

Look for:
- Recurring patterns
- Emerging themes
- Unexpected connections
- Higher-level abstractions
- Actionable insights

Return your analysis as JSON.`;

    const result = await this.extractStructured<{
      insights: Array<{
        title: string;
        content: string;
        confidence: number;
      }>;
    }>(prompt, schema, MODEL_PRESETS.consolidate);

    return result?.insights || [];
  }

  /**
   * Synthesize answer from memories
   */
  async synthesizeAnswer(
    query: string,
    memories: Array<{ summary: string; source: string }>
  ): Promise<{ answer: string; confidence: number }> {
    const schema = `{
  "answer": "Comprehensive answer synthesizing information from the memories",
  "confidence": 0.0-1.0 confidence score
}`;

    const memoriesText = memories
      .map((m, i) => `[${i + 1}] ${m.summary} (Source: ${m.source})`)
      .join('\n');

    const prompt = `Based on these memories, answer the following question:

Question: ${query}

Relevant Memories:
${memoriesText}

Provide a comprehensive answer that synthesizes information from the memories.
Cite sources where appropriate.
If the memories don't contain enough information to answer confidently, say so.

Return your response as JSON.`;

    const result = await this.extractStructured<{
      answer: string;
      confidence: number;
    }>(prompt, schema, MODEL_PRESETS.query);

    return result || { 
      answer: "I couldn't find enough information in memory to answer this question.", 
      confidence: 0 
    };
  }

  /**
   * Get Ollama client instance
   */
  getClient(): Ollama {
    return this.ollama;
  }
}
