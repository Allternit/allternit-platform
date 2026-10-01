/**
 * One-time import of this service's memories into the allternit-api memory
 * kernel (WP-M1d). Idempotent: unchanged memories are no-ops on re-run.
 * Usage: ALLTERNIT_API_TOKEN=... pnpm kernel:import [path/to/memory.db]
 */
import { MemoryStore } from '../src/store/sqlite-store.js';
import { KernelAdapter } from '../src/store/kernel-adapter.js';

const adapter = KernelAdapter.fromEnv();
if (!adapter) {
  console.error('Set ALLTERNIT_API_TOKEN (and ALLTERNIT_API_URL if not the local API).');
  process.exit(1);
}
const store = new MemoryStore(process.argv[2] ?? process.env.MEMORY_DB_PATH ?? './memory.db');
const all = store.getAllMemories();
const ok = await adapter.upsert(all);
console.log(`${ok ? 'imported' : 'partially imported'} ${all.length} memories into the memory kernel`);
store.close();
process.exit(ok ? 0 : 1);
