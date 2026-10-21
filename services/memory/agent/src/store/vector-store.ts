/**
 * Vector Store - Embedding-based Similarity Search
 * 
 * Uses Ollama embeddings for semantic search capabilities
 */

import { Ollama } from 'ollama';
import type { Memory } from '../types/memory.types.js';

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
  private ollama: Ollama;
  private index: Map<string, VectorIndex> = new Map();
  private embeddingModel: string;

  constructor(embeddingModel: string = 'mxbai-embed-large') {
    this.ollama = new Ollama({ host: 'http://localhost:11434' });
    this.embeddingModel = embeddingModel;
  }

  /**
   * Generate embedding for text
   */
  async embed(text: string): Promise<number[]> {
    try {
      const response = await (this.ollama as any).embed({
        model: this.embeddingModel,
        input: text,
      });
      return response.embeddings?.[0] || [];
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
    const queryEmbedding = await this.embed(query);
    
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
