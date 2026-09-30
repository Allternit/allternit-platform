/**
 * WP9 Context Compiler (C5, CL-048 / CL-121 / CL-140).
 *
 *   (AgentState, node) → packed context with provenance on every chunk.
 *
 * Pipeline: state → projection → retrieval → dedup → rank → sensitivity gate →
 * budget pack → serialization. Output conforms to the frozen ABI 1.0.0 contracts
 * ContextChunkV1 / ContextVisibilityDecisionV1 / ContextProjectionV1.
 *
 * Rules honoured (02 §13):
 *  - HIDE is not delete: every chunk gets a decision, hidden ones included.
 *  - Compression (SHORT) only happens once the node's next operation is known.
 *  - Projections are fingerprinted, cacheable and disposable.
 *  - Untrusted content is fenced, never concatenated into control instructions.
 *  - Pure + deterministic: same inputs (incl. `now`) → byte-identical output.
 *
 * SharedRetrievalSnapshot is named-but-undefined in the ABI (CL-121 PROVISIONAL),
 * so its shape here is internal (`SharedRetrievalSnapshotV0`) and is referenced
 * from the projection only by id, which the ABI already allows.
 */
import type {
  AgentStateV1,
  ContextChunkV1,
  ContextProjectionV1,
  ContextVisibilityDecisionV1,
  Provenance,
  SensitivityClass,
  TrustClass,
} from "../../../../../../spec/Contracts/kernel/v1/generated/ts/kernel-abi"
import { TOKENIZER_FAMILY, canonicalize, estimateTokens, hashValue, sha256Tagged } from "./jcs"

export const CONTEXT_SCHEMA_VERSION = "1.0.0"
type Hash = `sha256:${string}`
export type ChunkKind = ContextChunkV1["kind"]
export type SourceType = Provenance["source_type"]
export type Visibility = ContextVisibilityDecisionV1["visibility"]

export interface ContextCandidate {
  /** Stable source key (e.g. "fs:src/a.ts#foo", "instr:style"). */
  key: string
  kind: ChunkKind
  text: string
  /** Pre-computed compressed form, used for SHORT visibility. */
  short?: string
  source: SourceType
  source_id: string
  trust_class?: TrustClass
  sensitivity?: SensitivityClass
  /** Base relevance 0..1. */
  priority?: number
  pinned?: boolean
  /** Keys of candidates this one supersedes. */
  supersedes?: string[]
  observed_at?: string
  path?: string
  extraction_method?: string
  dependency_distance?: number | null
}

export interface CompilerNode {
  node_id: string
  /** Capability the projection targets, e.g. cap.code.edit. */
  capability: string
  /** Next operation, once known; enables compression. */
  next_op?: string | null
  /** Logical model family (never a vendor id). */
  target_model_family?: string | null
}

export interface Codemap {
  files: { path: string; symbols?: string[]; imports?: string[] }[]
}

// ---------------------------------------------------------------------------
// Repo index, keyed per Q6: repository + worktree + base commit + workspace
// generation + content fingerprint (never commit SHA alone).
// ---------------------------------------------------------------------------

export interface RepoIndexKey {
  repository: string
  worktree: string
  base_commit: string
  workspace_generation: number
  content_fingerprint: Hash
}

export interface RepoIndexEntry {
  path: string
  symbol: string | null
  text: string
  content_hash: Hash
}

export interface RepoIndex {
  key: RepoIndexKey
  key_hash: Hash
  entries: RepoIndexEntry[]
}

function sortEntries(entries: RepoIndexEntry[]) {
  return [...entries].sort((a, b) =>
    a.path === b.path ? (a.symbol ?? "").localeCompare(b.symbol ?? "") : a.path < b.path ? -1 : 1,
  )
}

function fingerprintEntries(entries: RepoIndexEntry[]): Hash {
  return hashValue(sortEntries(entries).map((e) => [e.path, e.symbol, e.content_hash]))
}

export function buildRepoIndex(
  key: Omit<RepoIndexKey, "content_fingerprint">,
  files: { path: string; symbol?: string | null; text: string }[],
): RepoIndex {
  const entries = sortEntries(
    files.map((f) => ({ path: f.path, symbol: f.symbol ?? null, text: f.text, content_hash: sha256Tagged(f.text) })),
  )
  const full: RepoIndexKey = { ...key, content_fingerprint: fingerprintEntries(entries) }
  return { key: full, key_hash: hashValue(full), entries }
}

/** Paths that (transitively) import any of `paths`, per the codemap. */
export function dependents(codemap: Codemap | undefined, paths: string[]): Set<string> {
  const out = new Set(paths)
  if (!codemap) return out
  let grew = true
  while (grew) {
    grew = false
    for (const f of codemap.files) {
      if (out.has(f.path)) continue
      if ((f.imports ?? []).some((i) => out.has(i))) {
        out.add(f.path)
        grew = true
      }
    }
  }
  return out
}

/**
 * Incremental invalidation (Q6): drop entries for changed paths and their
 * dependents, add replacements, bump the workspace generation and re-key.
 */
export function invalidateRepoIndex(
  index: RepoIndex,
  changedPaths: string[],
  opts: { codemap?: Codemap; replacements?: { path: string; symbol?: string | null; text: string }[] } = {},
): { index: RepoIndex; invalidated: string[] } {
  const affected = dependents(opts.codemap, changedPaths)
  // Affected entries are dropped; callers re-index them via `replacements`.
  const kept = index.entries.filter((e) => !affected.has(e.path))
  const next = buildRepoIndex(
    {
      repository: index.key.repository,
      worktree: index.key.worktree,
      base_commit: index.key.base_commit,
      workspace_generation: index.key.workspace_generation + 1,
    },
    [...kept, ...(opts.replacements ?? [])],
  )
  return { index: next, invalidated: [...affected].sort() }
}

// ---------------------------------------------------------------------------
// SharedRetrievalSnapshot (CL-121): one immutable, read-only retrieval view that
// the foreground node and background read-only graphs (bg.review, bg.eval, …) share.
// ---------------------------------------------------------------------------

export interface SnapshotEntry {
  entry_id: string
  path: string
  symbol: string | null
  content_hash: Hash
  dependency_distance: number | null
}

export interface SharedRetrievalSnapshotV0 {
  readonly snapshot_id: string
  readonly fingerprint: Hash
  readonly repo_index_key_hash: Hash
  readonly state_version: number
  readonly read_only: true
  readonly entries: readonly SnapshotEntry[]
}

function deepFreeze<T>(o: T): T {
  if (o && typeof o === "object" && !Object.isFrozen(o)) {
    Object.freeze(o)
    for (const v of Object.values(o as any)) deepFreeze(v)
  }
  return o
}

/** BFS distance from target paths over the codemap's import graph (both directions). */
export function dependencyDistances(codemap: Codemap | undefined, targets: string[]): Map<string, number> {
  const dist = new Map<string, number>()
  const adj = new Map<string, Set<string>>()
  const link = (a: string, b: string) => {
    if (!adj.has(a)) adj.set(a, new Set())
    adj.get(a)!.add(b)
  }
  for (const f of codemap?.files ?? []) for (const i of f.imports ?? []) (link(f.path, i), link(i, f.path))
  let frontier = [...new Set(targets)].sort()
  for (const t of frontier) dist.set(t, 0)
  let d = 0
  while (frontier.length) {
    d++
    const next: string[] = []
    for (const p of frontier)
      for (const n of [...(adj.get(p) ?? [])].sort())
        if (!dist.has(n)) {
          dist.set(n, d)
          next.push(n)
        }
    frontier = next
  }
  return dist
}

export function targetPaths(state: AgentStateV1): string[] {
  return (state.selected_targets ?? [])
    .filter((r) => r.startsWith("fs:"))
    .map((r) => r.slice(3))
    .sort()
}

export function createSharedRetrievalSnapshot(input: {
  index: RepoIndex
  codemap?: Codemap
  targets: string[]
  state_version: number
}): SharedRetrievalSnapshotV0 {
  const dist = dependencyDistances(input.codemap, input.targets)
  const entries: SnapshotEntry[] = input.index.entries.map((e) => ({
    entry_id: `${e.path}${e.symbol ? "#" + e.symbol : ""}`,
    path: e.path,
    symbol: e.symbol,
    content_hash: e.content_hash,
    dependency_distance: dist.has(e.path) ? dist.get(e.path)! : null,
  }))
  const fingerprint = hashValue({ key: input.index.key_hash, state_version: input.state_version, entries })
  return deepFreeze({
    snapshot_id: `rsnap:${fingerprint.slice(7, 31)}`,
    fingerprint,
    repo_index_key_hash: input.index.key_hash,
    state_version: input.state_version,
    read_only: true as const,
    entries,
  })
}

/** In-process registry so background read-only graphs share one snapshot by id. */
export namespace SnapshotRegistry {
  const store = new Map<string, SharedRetrievalSnapshotV0>()
  export function put(s: SharedRetrievalSnapshotV0) {
    store.set(s.snapshot_id, s)
    return s.snapshot_id
  }
  export function get(id: string) {
    return store.get(id)
  }
  export function clear() {
    store.clear()
  }
}

// ---------------------------------------------------------------------------
// Compiler
// ---------------------------------------------------------------------------

export interface CompileContextInput {
  state: AgentStateV1
  node: CompilerNode
  budget_tokens: number
  /** RFC 3339 timestamp; pass explicitly for determinism. */
  now: string
  candidates?: ContextCandidate[]
  codemap?: Codemap
  repoIndex?: RepoIndex
  snapshot?: SharedRetrievalSnapshotV0
  /** Max dependency distance pulled in by retrieval (default 2). */
  max_retrieval_distance?: number
}

export interface CompiledContext {
  projection: ContextProjectionV1
  chunks: ContextChunkV1[]
  /** chunk_id → text actually assembled (FULL or SHORT form). */
  contents: Record<string, string>
  snapshot: SharedRetrievalSnapshotV0 | null
  /** Visible chunks serialized in assembly order; untrusted content fenced. */
  rendered: string
  overflow: boolean
}

interface Working {
  c: ContextCandidate
  chunk_id: string
  hash: Hash
  score: number
  visibility: Visibility
  reason: string
  evidence: string[]
  compression: string | null
  sensitivity: SensitivityClass
  trust: TrustClass
}

const SENS_ORDER: SensitivityClass[] = ["PUBLIC", "INTERNAL", "RESTRICTED", "SECRET"]
const maxSens = (a: SensitivityClass, b: SensitivityClass) =>
  SENS_ORDER.indexOf(a) >= SENS_ORDER.indexOf(b) ? a : b

function chunkIdFor(kind: string, key: string) {
  return `ctx:${hashValue({ kind, key }).slice(7, 31)}`
}

/** Stage 1: state → candidate chunks. */
export function stateCandidates(state: AgentStateV1): ContextCandidate[] {
  const out: ContextCandidate[] = []
  const sid = `state:${state.identity.run_id}@${state.identity.state_version}`
  out.push({
    key: "state:user_intent",
    kind: "USER_TURN",
    text: state.user_intent.objective,
    source: "USER",
    source_id: sid,
    trust_class: "INTERNAL",
    priority: 1,
    pinned: true,
    extraction_method: "state.user_intent",
  })
  for (const p of state.instruction_predicates ?? [])
    out.push({
      key: `state:instruction:${p.predicate_id}`,
      kind: "INSTRUCTION",
      text: `${p.instruction_ref} when ${p.condition}`,
      source: "POLICY",
      source_id: p.instruction_ref,
      trust_class: "INTERNAL",
      priority: 0.9,
      pinned: !!p.pinned,
      extraction_method: "state.instruction_predicates",
    })
  if (state.plan && Object.keys(state.plan).length)
    out.push({
      key: "state:plan",
      kind: "PLAN",
      text: canonicalize(state.plan),
      source: "SYSTEM",
      source_id: sid,
      trust_class: "INTERNAL",
      priority: 0.8,
      extraction_method: "state.plan",
    })
  ;(state.unresolved_failures ?? []).forEach((f, i) =>
    out.push({
      key: `state:failure:${i}`,
      kind: "DIAGNOSTIC",
      text: canonicalize(f),
      source: "TOOL",
      source_id: sid,
      trust_class: "INTERNAL",
      priority: 0.75,
      extraction_method: "state.unresolved_failures",
    }),
  )
  ;(state.diagnostics ?? []).forEach((d, i) =>
    out.push({
      key: `state:diagnostic:${i}`,
      kind: "DIAGNOSTIC",
      text: canonicalize(d),
      source: "TOOL",
      source_id: sid,
      trust_class: "INTERNAL",
      priority: 0.7,
      extraction_method: "state.diagnostics",
    }),
  )
  for (const h of state.hypotheses ?? [])
    if (h.status === "OPEN" || h.status === "SUPPORTED")
      out.push({
        key: `state:hypothesis:${h.hypothesis_id}`,
        kind: "OTHER",
        text: `[${h.status}] ${h.statement}`,
        source: "MODEL",
        source_id: h.hypothesis_id,
        trust_class: "INTERNAL",
        priority: 0.6,
        extraction_method: "state.hypotheses",
      })
  return out
}

/** Stage 3: retrieval from the shared snapshot. */
function retrievalCandidates(
  snapshot: SharedRetrievalSnapshotV0,
  index: RepoIndex | undefined,
  maxDistance: number,
): ContextCandidate[] {
  const byHash = new Map((index?.entries ?? []).map((e) => [e.content_hash, e.text]))
  const out: ContextCandidate[] = []
  for (const e of snapshot.entries) {
    if (e.dependency_distance === null || e.dependency_distance > maxDistance) continue
    const text = byHash.get(e.content_hash)
    if (text === undefined) continue
    out.push({
      key: `fs:${e.entry_id}`,
      kind: e.symbol ? "SYMBOL" : "FILE",
      text,
      source: "RETRIEVAL",
      source_id: `${snapshot.snapshot_id}/${e.entry_id}`,
      trust_class: "INTERNAL",
      priority: 0.65 / (1 + e.dependency_distance),
      path: e.path,
      dependency_distance: e.dependency_distance,
      extraction_method: "retrieval.snapshot",
    })
  }
  return out
}

export function compileContext(input: CompileContextInput): CompiledContext {
  const { state, node, now } = input
  const stateVersion = state.identity.state_version
  const maxDist = input.max_retrieval_distance ?? 2

  // Retrieval snapshot (built once, shared by id).
  let snapshot = input.snapshot ?? null
  if (!snapshot && input.repoIndex)
    snapshot = createSharedRetrievalSnapshot({
      index: input.repoIndex,
      codemap: input.codemap,
      targets: targetPaths(state),
      state_version: stateVersion,
    })

  const candidates = [
    ...stateCandidates(state),
    ...(input.candidates ?? []),
    ...(snapshot ? retrievalCandidates(snapshot, input.repoIndex, maxDist) : []),
  ]

  // Stage 2: projection — sensitivity labels from state.
  const labels = new Map<string, SensitivityClass>()
  for (const l of state.sensitivity_labels ?? []) labels.set(l.resource, maxSens(labels.get(l.resource) ?? "PUBLIC", l.class))

  const seenKeys = new Set<string>()
  const work: Working[] = []
  for (const c of candidates) {
    if (seenKeys.has(c.key)) continue // identical source key → first wins (state > caller > retrieval)
    seenKeys.add(c.key)
    let sensitivity = c.sensitivity ?? "INTERNAL"
    if (c.path) sensitivity = maxSens(sensitivity, labels.get(`fs:${c.path}`) ?? "PUBLIC")
    sensitivity = maxSens(sensitivity, labels.get(c.key) ?? "PUBLIC")
    const trust = c.trust_class ?? "INTERNAL"
    work.push({
      c,
      chunk_id: chunkIdFor(c.kind, c.key),
      hash: sha256Tagged(c.text),
      score: (c.priority ?? 0.5) + (c.pinned ? 10 : 0),
      visibility: "FULL",
      reason: c.pinned ? "PINNED" : "SELECTED",
      evidence: [],
      compression: null,
      sensitivity,
      trust,
    })
  }

  // Stage 4: rank (deterministic tie-break on chunk_id).
  work.sort((a, b) => b.score - a.score || (a.chunk_id < b.chunk_id ? -1 : a.chunk_id > b.chunk_id ? 1 : 0))
  const byKey = new Map(work.map((w) => [w.c.key, w]))

  // Stage 5: supersession + dedup by content hash (keep highest-ranked).
  for (const w of work)
    for (const k of w.c.supersedes ?? []) {
      const old = byKey.get(k)
      if (old && old !== w && old.visibility !== "HIDE") {
        old.visibility = "HIDE"
        old.reason = "SUPERSEDED"
        old.evidence = [w.chunk_id]
      }
    }
  const winner = new Map<string, Working>()
  for (const w of work) {
    if (w.visibility === "HIDE") continue
    const first = winner.get(w.hash)
    if (!first) winner.set(w.hash, w)
    else {
      w.visibility = "HIDE"
      w.reason = "DUPLICATE"
      w.evidence = [first.chunk_id]
    }
  }

  // Stage 6: sensitivity gate — SECRET never enters a pack.
  for (const w of work)
    if (w.visibility !== "HIDE" && (w.sensitivity === "SECRET" || w.trust === "SECRET")) {
      w.visibility = "HIDE"
      w.reason = "SENSITIVITY_SECRET"
    }

  // Stage 7: budget pack. Compress (only once next op is known), then drop
  // lowest-priority non-pinned chunks first.
  const tokensOf = (w: Working) =>
    w.visibility === "HIDE" ? 0 : estimateTokens(w.visibility === "SHORT" ? w.c.short! : w.c.text)
  const total = () => work.reduce((s, w) => s + tokensOf(w), 0)
  const budget = Math.max(0, Math.floor(input.budget_tokens))
  const lowestFirst = () => [...work].reverse().filter((w) => w.visibility !== "HIDE" && !w.c.pinned)
  if (node.next_op && total() > budget)
    for (const w of lowestFirst()) {
      if (total() <= budget) break
      if (w.visibility === "FULL" && w.c.short && w.c.short.length < w.c.text.length) {
        w.visibility = "SHORT"
        w.reason = "COMPRESSED_FOR_BUDGET"
        w.compression = "provided_short"
      }
    }
  for (const w of lowestFirst()) {
    if (total() <= budget) break
    w.visibility = "HIDE"
    w.reason = "BUDGET_DROPPED"
  }
  const overflow = total() > budget

  // Stage 8: serialization.
  const visible = work.filter((w) => w.visibility !== "HIDE")
  const hidden = work.filter((w) => w.visibility === "HIDE").sort((a, b) => (a.chunk_id < b.chunk_id ? -1 : 1))
  const decisions: ContextVisibilityDecisionV1[] = []
  const contents: Record<string, string> = {}
  visible.forEach((w, i) => {
    const text = w.visibility === "SHORT" ? w.c.short! : w.c.text
    contents[w.chunk_id] = text
    decisions.push(decision(w, estimateTokens(text), i))
  })
  for (const w of hidden) decisions.push(decision(w, 0, null))

  const chunks: ContextChunkV1[] = [...visible, ...hidden].map((w) => {
    // Chunk identity names the serialized representation. Source provenance
    // retains the original hash so a SHORT summary keeps its source lineage.
    const text = contents[w.chunk_id] ?? w.c.text
    const contentHash = sha256Tagged(text)
    const prov: Provenance = {
      source_type: w.c.source,
      source_id: w.c.source_id,
      trust_class: w.trust,
      content_hash: w.hash,
      observed_at: w.c.observed_at ?? now,
    }
    return {
      schema_id: "allternit.kernel.ContextChunkV1",
      schema_version: CONTEXT_SCHEMA_VERSION,
      chunk_id: w.chunk_id,
      kind: w.c.kind,
      content_ref: `cas:${contentHash}`,
      content_hash: contentHash,
      source_ref: prov,
      trust_class: w.trust,
      sensitivity: w.sensitivity,
      created_at: w.c.observed_at ?? now,
      supersedes: (w.c.supersedes ?? []).map((k) => byKey.get(k)?.chunk_id).filter(Boolean) as string[],
      pinned_conditions: w.c.pinned ? [w.chunk_id] : [],
      freshness: null,
      dependency_distance: w.c.dependency_distance ?? null,
      token_estimates: { [TOKENIZER_FAMILY]: estimateTokens(text) },
      extensions: {
        "x-item": {
          authority: w.c.source,
          relevance: Math.min(1, w.c.priority ?? 0.5),
          token_cost: estimateTokens(text),
          trust_level: w.trust,
          duplicate_group: w.hash,
          state_version: stateVersion,
          extraction_method: w.c.extraction_method ?? "caller",
          source_key: w.c.key,
        },
        ...(w.trust === "UNTRUSTED" ? { "x-fenced": true } : {}),
      },
    }
  })

  const rendered = visible
    .map((w) => {
      const body = contents[w.chunk_id]
      const head = `[${w.chunk_id} ${w.c.kind} ${w.c.source}:${w.c.source_id}]`
      return w.trust === "UNTRUSTED" ? `${head}\n<untrusted-content>\n${body}\n</untrusted-content>` : `${head}\n${body}`
    })
    .join("\n\n")


  const compiled = decisions.reduce((s, d) => s + (d.estimated_tokens ?? 0), 0)
  const fingerprint = hashValue({
    capability: node.capability,
    model_family: node.target_model_family ?? null,
    node_id: node.node_id,
    selected: decisions.map((d) => [d.chunk_id, d.visibility, d.assembly_order]),
    hashes: chunks.map((c) => c.content_hash),
    rendered_hash: sha256Tagged(rendered),
    snapshot: snapshot?.fingerprint ?? null,
  })
  const projection: ContextProjectionV1 = {
    schema_id: "allternit.kernel.ContextProjectionV1",
    schema_version: CONTEXT_SCHEMA_VERSION,
    projection_id: `ctxproj:${fingerprint.slice(7, 31)}`,
    target_capability: node.capability,
    target_model_family: node.target_model_family ?? null,
    selected: decisions,
    compiled_tokens_estimate: compiled,
    budget_tokens: budget,
    instruction_fragments: visible.filter((w) => w.c.kind === "INSTRUCTION").map((w) => w.chunk_id),
    retrieval_snapshot_id: snapshot?.snapshot_id ?? null,
    context_fingerprint: fingerprint,
    cache_strategy: "REBUILD",
    extensions: { "x-node_id": node.node_id, "x-state_version": stateVersion, ...(overflow ? { "x-overflow": true } : {}) },
  }


  return { projection, chunks, contents, snapshot, rendered, overflow }
}

function decision(w: Working, tokens: number, order: number | null): ContextVisibilityDecisionV1 {
  return {
    schema_id: "allternit.kernel.ContextVisibilityDecisionV1",
    schema_version: CONTEXT_SCHEMA_VERSION,
    chunk_id: w.chunk_id,
    visibility: w.visibility,
    relevance: Math.max(0, Math.min(1, w.c.priority ?? 0.5)),
    reason_code: w.reason,
    compression_method: w.compression,
    evidence_pointers: w.evidence,
    estimated_tokens: tokens,
    assembly_order: order,
  }
}
