/**
 * What `/pet` stores in the global config. The pet's identity is not stored
 * here: it is the Allternit bot the Desktop pet wears (see petBots.ts). This
 * only records that the terminal pet is turned on. `name`/`personality` are
 * left over from the old hatched creatures and ignored.
 */
export type StoredCompanion = { hatchedAt: number; name?: string; personality?: string }
