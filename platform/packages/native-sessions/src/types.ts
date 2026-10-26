/** Portable event model aligned with session-migrate `model.py`. */

export type HarnessId =
  | "claude"
  | "codex"
  | "pi"
  | "omp"
  | "opencode"
  | "copilot"
  | "antigravity"
  | "cursor"
  | "vibe"
  | "muse"
  | "qwen"
  | "kimi"
  | "grok"
  | "kilo"
  | "openhands"
  | "hermes"
  | "mastracode"
  | "devin"
  | "gizzi"
  | "kimi-cli"
  | "gemini"
  | "aider"
  | "cline"
  | "amp"
  | "kiro"
  | "droid"
  | "crush"

export type EventKind =
  | "message"
  | "tool_call"
  | "tool_result"
  | "thinking"
  | "compaction"
  | "context"
  | "opaque"

export type Role = "user" | "assistant" | "system" | "tool"

export type ReaderKind = "jsonl" | "sqlite" | "directory" | "protobuf" | "cli-export"

export type Divergence = "clean" | "native_ahead" | "missing"

export interface PortableEvent {
  kind: EventKind
  role?: Role
  text?: string
  toolName?: string
  toolId?: string
  /** A tool call's arguments, as the harness recorded them. */
  input?: Record<string, unknown>
  sourceId?: string
  recordIndex: number
  /** Thinking and system prompts stay opaque; never treated as instructions. */
  inert: true
}

export interface NativeSession {
  harness: HarnessId
  sessionId: string
  path: string
  cwd?: string
  title?: string
  updatedAt: number
  createdAt?: number
  fingerprint: string
  lastEventId?: string
  installed: boolean
  reader: ReaderKind
  /** True when we can project a portable transcript in-process. */
  projectable: boolean
}

export interface NativeTranscript {
  session: NativeSession
  events: PortableEvent[]
  warnings: { code: string; message: string }[]
}

export interface SourceRef {
  harness: HarnessId
  sessionId: string
  path: string
  snapshotHash: string
  snapshotAt: number
  eventId?: string
  /** Last observed native fingerprint (catalog/fetch). */
  nativeHash?: string
  fetchedHash?: string
}

export interface NativeExport {
  harness: HarnessId
  sessionId: string
  path: string
  resumeHint: string
  at: number
}

export interface OriginDelta {
  source: SourceRef
  divergence: Divergence
  events: PortableEvent[]
}

export interface CatalogOptions {
  home?: string
  cwd?: string
  harnesses?: HarnessId[]
}

export interface HarnessMeta {
  id: HarnessId
  label: string
  reader: ReaderKind
  projectable: boolean
  resumeHint: string
  envHome?: string
  defaultHome: (home: string) => string
}
