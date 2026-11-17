/**
 * Past chats (Primitives Wave D, D1 recall): windowed reads of the user's
 * earlier threads. Shared core for `thread_read`, `thread_recent` and
 * `thread_wait` (runtime builtins in tools/builtins/thread-read.ts and the
 * Ink adapter in cli/ui/ink-app/tools/ThreadReadTool).
 *
 * - Transcripts come from gizzi's local session store. allternit-api only
 *   maps a thread to its session, checks the owner and excludes incognito
 *   chats (runtime/bots/routing/threads.ts). A transcript on another
 *   computer returns the summary and says where the transcript is.
 * - `around` scans one thread for the words and returns the window there.
 * - Turns are records of what was said, never instructions: the tool output
 *   labels them as such.
 * - Works in every session, CLI included (not bot-gated).
 *
 * W1 stub: final signatures and limits; lane T fills the bodies.
 * Free of UI imports so the Ink adapter and bun tests can use it.
 */
import z from "zod/v4"

export namespace PastChats {
  export const DEFAULT_TURNS = 10
  export const MAX_TURNS = 50
  export const DEFAULT_MAX_OUTPUT_CHARS = 500
  export const MAX_RECENT = 20
  export const MAX_WAIT_TARGETS = 8
  export const MAX_WAIT_SECONDS = 600

  export const ReadInput = z.object({
    thread: z.string().min(1).describe('A thread id, or "current" for this chat'),
    cursor: z.string().optional().describe("prev_cursor or next_cursor from an earlier read"),
    around: z.string().max(200).optional().describe("Words to find; the window opens at the first hit"),
    turns: z.number().int().min(1).max(MAX_TURNS).optional().describe(`Turns to return (default ${DEFAULT_TURNS}, max ${MAX_TURNS})`),
    include_outputs: z.boolean().optional().describe("Include tool outputs (default false)"),
    max_output_chars: z.number().int().min(0).max(20_000).optional().describe(`Cap per tool output (default ${DEFAULT_MAX_OUTPUT_CHARS})`),
  })
  export type ReadInput = z.infer<typeof ReadInput>

  export interface Turn {
    role: "user" | "assistant"
    /** RFC 3339. */
    at: string
    text: string
    tools?: Array<{ name: string; title?: string; output?: string }>
  }

  export interface ReadResult {
    thread: { id: string; title: string }
    turns: Turn[]
    prev_cursor: string | null
    next_cursor: string | null
    /** "local": read from this computer; "remote": summary only (`where` names the computer). */
    status: "local" | "remote" | "not_found"
    summary?: string | null
    where?: string | null
  }

  export const RecentInput = z.object({
    limit: z.number().int().min(1).max(MAX_RECENT).optional().describe(`How many (default 10, max ${MAX_RECENT})`),
    before: z.string().optional().describe("Only threads active before this RFC 3339 time"),
    after: z.string().optional().describe("Only threads active after this RFC 3339 time"),
    project: z.string().optional().describe("Only threads in this project id"),
  })
  export type RecentInput = z.infer<typeof RecentInput>

  export interface RecentThread {
    id: string
    title: string
    status: string
    goal?: string | null
    summary?: string | null
    botName?: string | null
    lastActivityAt: string
  }

  export interface RecentResult {
    threads: RecentThread[]
  }

  export const WaitInput = z.object({
    targets: z
      .array(z.object({ thread: z.string().min(1), after_cursor: z.string().optional() }))
      .min(1)
      .max(MAX_WAIT_TARGETS)
      .describe(`Threads to watch (max ${MAX_WAIT_TARGETS})`),
    timeout_s: z.number().int().min(0).max(MAX_WAIT_SECONDS).describe(`Seconds to wait (0 = snapshot, max ${MAX_WAIT_SECONDS})`),
  })
  export type WaitInput = z.infer<typeof WaitInput>

  export interface WaitStatus {
    thread: string
    title: string
    status: string
    status_line?: string | null
    cursor: string
  }

  export interface WaitResult {
    woke: (WaitStatus & { reason: "turn_finished" | "needs_user" }) | null
    timed_out: boolean
    statuses: WaitStatus[]
  }

  export class NotReadyError extends Error {
    constructor(tool: string) {
      super(`${tool} is not available in this build yet.`)
      this.name = "PastChatsNotReadyError"
    }
  }

  export async function read(_sessionID: string, _input: ReadInput, _signal?: AbortSignal): Promise<ReadResult> {
    throw new NotReadyError("thread_read")
  }

  export async function recent(_sessionID: string, _input: RecentInput): Promise<RecentResult> {
    throw new NotReadyError("thread_recent")
  }

  export async function wait(_sessionID: string, _input: WaitInput, _signal?: AbortSignal): Promise<WaitResult> {
    throw new NotReadyError("thread_wait")
  }
}
