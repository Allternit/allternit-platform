/**
 * Brain Runtime — Triple-store memory for gizzi
 *
 * Export the brain service, schema, and embedding providers.
 */

export {
  BrainService,
  KeywordEmbedding,
  createOpenAIEmbedder,
  type MemoryQuery,
  type MemoryChunk,
  type Entity,
  type EmbeddingProvider,
} from "./brain.service"

export {
  MemoryChunkTable,
  MemoryEmbeddingTable,
  MemoryEntityTable,
  MemoryRelationTable,
} from "./memory.sql"
