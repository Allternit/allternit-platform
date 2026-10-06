/**
 * Capability classes (O1) and output caps by call type (O5).
 *
 * Mirror of the kernel router's table (`factory/engine/src/workflows/kernel/classes.rs`).
 * Both sides pin the same values in tests; change them together.
 * Classes are logical: no vendor or model name appears here.
 */
import type { ModelPoolEntryV1, Role } from "./pool"

export const GEN_SMALL = "gen.small"
export const GEN_STANDARD = "gen.standard"
export const GEN_DEEP = "gen.deep"
export type GenClass = typeof GEN_SMALL | typeof GEN_STANDARD | typeof GEN_DEEP
export const GEN_CLASSES: GenClass[] = [GEN_SMALL, GEN_STANDARD, GEN_DEEP]

/** Remote entries at or below this blended pool cost (USD / 1k tokens) are small. */
export const SMALL_COST_MAX = 0.0015

export function genClassOf(e: Pick<ModelPoolEntryV1, "cognitive_roles" | "residency" | "cost" | "extensions">): GenClass {
  const pinned = e.extensions?.["x-gen_class"]
  if (typeof pinned === "string" && (GEN_CLASSES as string[]).includes(pinned)) return pinned as GenClass
  if (e.cognitive_roles.includes("S3")) return GEN_DEEP
  if (e.residency !== "REMOTE" || e.cost <= SMALL_COST_MAX) return GEN_SMALL
  return GEN_STANDARD
}

const SMALL_CALLS = new Set(["title", "summary", "compaction", "extraction", "memory_extraction", "memory_curation", "curation", "lessons", "classify"])
const DEEP_CALLS = new Set(["plan", "deep", "solver", "escalation"])

export function callTypeClass(callType: string): GenClass {
  if (SMALL_CALLS.has(callType)) return GEN_SMALL
  if (DEEP_CALLS.has(callType)) return GEN_DEEP
  return GEN_STANDARD
}

export const OUTPUT_CAPS_VERSION = "o5.v1"
export function defaultMaxOutputTokens(role: Role, callType?: string): number | undefined {
  if (role === "S0") return undefined
  if (role === "S1") return 0
  switch (callType) {
    case "title":
      return 64
    case "summary":
    case "compaction":
    case "extraction":
    case "memory_extraction":
    case "memory_curation":
    case "curation":
    case "lessons":
      return 1024
    case "patch":
    case "edit":
      return 8192
    default:
      return undefined
  }
}

/** One class up (cascade escalation); deep stays deep. */
export function escalate(c: GenClass): GenClass {
  return c === GEN_SMALL ? GEN_STANDARD : GEN_DEEP
}
