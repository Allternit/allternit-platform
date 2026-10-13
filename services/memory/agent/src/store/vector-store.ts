/**
 * Vector Store - Embedding-based Similarity Search
 *
 * Embeds through the shared Allternit embedding server (EmbeddingGemma 2,
 * tools/system-one-local/laya/serve-embed.sh) at ALLTERNIT_EMBED_URL, the same
 * OpenAI-compatible endpoint allternit-api's memory index uses, so memory
 * vectors live in one space across services.
 */

import type { Memory } from '../types/memory.types.js';

export const DEFAULT_EMBED_URL = 'http://127.0.0.1:7719';
export const DEFAULT_EMBED_MODEL = 'google/embeddinggemma-2';

export type EmbedInputType = 'query' | 'document';

/**
 * In-memory vector index (for small datasets)
 * For production, use Qdrant, Pinecone, or pgvector
 */
interface VectorIndex {
  id: string;
  embedding: number[];
  memory: Memory;
}

export class VectorStore {
  private embedUrl: string;
  private index: Map<string, VectorIndex> = new Map();
  private embeddingModel: string;

  constructor(
    embeddingModel: string = process.env.ALLTERNIT_EMBED_MODEL || DEFAULT_EMBED_MODEL,
    embedUrl: string = process.env.ALLTERNIT_EMBED_URL || DEFAULT_EMBED_URL,
  ) {
    this.embedUrl = embedUrl.replace(/\/+$/, '');
    this.embeddingModel = embeddingModel;
  }

  /**
   * Generate embedding for text. Queries and documents get different task
   * prefixes server-side; returns [] when the endpoint is off or down.
   */
  async embed(text: string, inputType: EmbedInputType = 'document'): Promise<number[]> {
    if (this.embedUrl === 'off') return [];
    try {
      const res = await fetch(`${this.embedUrl}/v1/embeddings`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ model: this.embeddingModel, input: [text], input_type: inputType }),
        signal: AbortSignal.timeout(30_000),
      });
      if (!res.ok) throw new Error(`embed endpoint returned ${res.status}`);
      const body = (await res.json()) as { data?: Array<{ embedding?: number[] }> };
      return body.data?.[0]?.embedding ?? [];
    } catch (error) {
      console.error('Error generating embedding:', error);
      return [];
    }
  }

  /**
   * Add memory to vector index
   */
  async addMemory(memory: Memory): Promise<void> {
    const embedding = await this.embed(memory.content + ' ' + memory.summary);
    
    if (embedding.length > 0) {
      this.index.set(memory.id, {
        id: memory.id,
        embedding,
        memory,
      });
    }
  }

  /**
   * Remove memory from index
   */
  removeMemory(memoryId: string): void {
    this.index.delete(memoryId);
  }

  /**
   * Similarity search using cosine similarity
   */
  async similaritySearch(query: string, limit: number = 10): Promise<Memory[]> {
    const queryEmbedding = await this.embed(query, 'query');
    
    if (queryEmbedding.length === 0) {
      return [];
    }

    // Calculate similarity scores
    const scores: Array<{ memory: Memory; score: number }> = [];

    for (const entry of this.index.values()) {
      const score = this.cosineSimilarity(queryEmbedding, entry.embedding);
      scores.push({ memory: entry.memory, score });
    }

    // Sort by similarity and return top results
    scores.sort((a, b) => b.score - a.score);

    return scores.slice(0, limit).map(s => s.memory);
  }

  /**
   * Cosine similarity between two vectors
   */
  private cosineSimilarity(a: number[], b: number[]): number {
    if (a.length !== b.length || a.length === 0) {
      return 0;
    }

    let dotProduct = 0;
    let magnitudeA = 0;
    let magnitudeB = 0;

    for (let i = 0; i < a.length; i++) {
      dotProduct += a[i] * b[i];
      magnitudeA += a[i] * a[i];
      magnitudeB += b[i] * b[i];
    }

    magnitudeA = Math.sqrt(magnitudeA);
    magnitudeB = Math.sqrt(magnitudeB);

    if (magnitudeA === 0 || magnitudeB === 0) {
      return 0;
    }

    return dotProduct / (magnitudeA * magnitudeB);
  }

  /**
   * Get index statistics
   */
  getStats(): { count: number; model: string } {
    return {
      count: this.index.size,
      model: this.embeddingModel,
    };
  }

  /**
   * Clear the index
   */
  clear(): void {
    this.index.clear();
  }
}
