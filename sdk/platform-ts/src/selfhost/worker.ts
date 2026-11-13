/**
 * A custom-tool worker for a self-hosted Agents Server.
 *
 * It follows `docs/SELFHOST_WORKER_PROTOCOL.md`: claim a work item with an
 * environment key, keep the lease alive, take the session's tool calls with
 * the per-session secret, run your handler for each custom tool and post the
 * result. It makes outbound HTTPS calls only.
 *
 *   await runCustomToolWorker({
 *     environmentId: "env_…",
 *     environmentKey: process.env.ALLTERNIT_ENVIRONMENT_KEY, // aek_…
 *     tools: {
 *       lookup_order: async (input) => ({ status: "shipped", id: input.order_id }),
 *     },
 *   });
 *
 * Built-in sandbox tools (bash, file edits) are run by `gizzi worker`, not by
 * this helper. Give agents whose tools this worker runs their own environment,
 * so a gizzi worker and this helper don't take each other's work. A tool call
 * with no handler here gets an error result.
 */

import { APIError, AuthenticationError, ConflictError, PermissionError } from "../errors.ts";
import { SelfHostClient } from "./client.ts";
import type { EnvironmentPolicy, ToolCall, WorkClaim } from "./types.ts";

/** Tool results above this many bytes are refused by the server (413 `result_too_large`). */
export const MAX_TOOL_RESULT_BYTES = 102_400;
/** The protocol says: renew every 20 seconds (the lease lasts 60). */
export const RENEW_EVERY_MS = 20_000;

export interface ToolCallContext {
  workId: string;
  sessionId: string;
  toolCallId: string;
  name: string;
  /** The environment rules from the claim. A handler that makes network calls should respect them. */
  policy: EnvironmentPolicy;
  /** Aborted when the lease is lost or the worker stops. The result is then discarded. */
  signal: AbortSignal;
}

/**
 * Runs one custom tool. Return a string, or any JSON value (it is sent as JSON
 * text). Throw to send an error result (`is_error: true`) with the message.
 */
export type ToolHandler = (input: Record<string, unknown>, context: ToolCallContext) => unknown | Promise<unknown>;

export interface CustomToolWorkerOptions {
  /** The environment whose work this worker claims. */
  environmentId: string;
  /** Tool name to handler. */
  tools: Record<string, ToolHandler>;
  /** The environment key, `aek_…`. Defaults to `process.env.ALLTERNIT_ENVIRONMENT_KEY`. Ignored when `client` is given. */
  environmentKey?: string;
  /** Server URL. Defaults to `process.env.ALLTERNIT_SERVER_URL` or http://127.0.0.1:8400. Ignored when `client` is given. */
  baseUrl?: string;
  /** A client built with the environment key, if you made one yourself. */
  client?: SelfHostClient;
  /** A stable name for this worker process, shown in worker status. Defaults to `custom-tools-<random>`. */
  workerId?: string;
  /** Long-poll wait for claims and tool calls, 0 to 30 000 ms. Default 30 000. */
  blockMs?: number;
  /** Claim one item, run it, complete it and return. Return after one empty claim too. */
  drain?: boolean;
  /** Also claim items whose lease ran out at least this long ago (work from a worker that died). */
  reclaimOlderThanMs?: number;
  /** Abort to stop. A held item is handed back with `/release`. */
  signal?: AbortSignal;
  /** Progress lines. Never receives the session secret. Default: no output. */
  log?: (line: string) => void;
}

interface ToolResult {
  output: string;
  is_error: boolean;
}

function env(name: string): string | undefined {
  const p = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process;
  return p?.env?.[name];
}

function randomId(): string {
  const c = (globalThis as { crypto?: { randomUUID?: () => string } }).crypto;
  return c?.randomUUID ? c.randomUUID().slice(0, 8) : Math.random().toString(16).slice(2, 10);
}

function isLeaseStale(err: unknown): boolean {
  return err instanceof ConflictError && err.code === "lease_stale";
}

/** The session secret ended or the key was revoked: the item is no longer ours. */
function isLeaseGone(err: unknown): boolean {
  return isLeaseStale(err) || err instanceof AuthenticationError || err instanceof PermissionError;
}

function toText(value: unknown): string {
  if (typeof value === "string") return value;
  if (value === undefined) return "";
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

/** Cut `text` to at most `maxBytes` UTF-8 bytes, with a note saying it was cut. */
export function truncateUtf8(text: string, maxBytes: number = MAX_TOOL_RESULT_BYTES): string {
  const enc = new TextEncoder();
  const bytes = enc.encode(text);
  if (bytes.length <= maxBytes) return text;
  const note = `\n[truncated: ${bytes.length} bytes, limit ${maxBytes}]`;
  const keep = Math.max(0, maxBytes - enc.encode(note).length);
  // Drop a partial multi-byte character at the cut.
  const head = new TextDecoder("utf-8", { fatal: false }).decode(bytes.subarray(0, keep)).replace(/�$/, "");
  return head + note;
}

/**
 * Claim work for one environment and run custom tools until `signal` aborts
 * (or after one item with `drain`). Work items run one at a time; start more
 * processes for more at once.
 */
export async function runCustomToolWorker(options: CustomToolWorkerOptions): Promise<void> {
  const log = options.log ?? (() => undefined);
  const blockMs = Math.min(Math.max(options.blockMs ?? 30_000, 0), 30_000);
  const workerId = options.workerId ?? `custom-tools-${randomId()}`;
  const outer = options.signal;
  const client: SelfHostClient = options.client ?? (() => {
    const apiKey = options.environmentKey ?? env("ALLTERNIT_ENVIRONMENT_KEY");
    if (!apiKey) throw new Error("No environment key. Pass { environmentKey } or set ALLTERNIT_ENVIRONMENT_KEY (aek_…).");
    return new SelfHostClient({ apiKey, baseUrl: options.baseUrl });
  })();
  // One result per tool_call_id: a call handed out again (at-least-once) is
  // answered from here instead of running twice.
  const done = new Map<string, ToolResult>();
  const remember = (id: string, r: ToolResult) => {
    done.set(id, r);
    if (done.size > 1000) done.delete(done.keys().next().value as string);
  };
  const requestTimeout = blockMs + 30_000;

  while (!outer?.aborted) {
    let claim: WorkClaim | undefined;
    try {
      claim = await client.work.claim(
        options.environmentId,
        { worker_id: workerId, block_ms: blockMs, ...(options.reclaimOlderThanMs !== undefined ? { reclaim_older_than_ms: options.reclaimOlderThanMs } : {}) },
        { signal: outer, timeout: requestTimeout },
      );
    } catch (err) {
      if (outer?.aborted) return;
      throw err;
    }
    if (!claim) {
      if (options.drain) return;
      continue;
    }
    await runItem(client, claim);
    if (options.drain) return;
  }

  async function runItem(c: SelfHostClient, claim: WorkClaim): Promise<void> {
    const workId = claim.work_id;
    const generation = claim.lease_generation;
    const item = new AbortController();
    const onOuterAbort = () => item.abort();
    outer?.addEventListener("abort", onOuterAbort, { once: true });
    let lost = false;
    let failure: string | undefined;
    log(`claimed ${workId} (session ${claim.session_id}, generation ${generation})`);

    const renew = setInterval(() => {
      c.work.renew(workId, { lease_generation: generation }, { timeout: 15_000 }).catch((err) => {
        if (isLeaseStale(err)) {
          log(`lease on ${workId} is stale; another worker owns it. Stopping.`);
          lost = true;
          item.abort();
        } else {
          log(`renew failed for ${workId}: ${(err as Error)?.message ?? err}`);
        }
      });
    }, RENEW_EVERY_MS);

    // Only the worker holds the secret. It is never logged or passed to a handler.
    const headers = { "X-Session-Secret": claim.session_secret };
    try {
      while (!item.signal.aborted) {
        let call: ToolCall | undefined;
        try {
          call = await c.toolCalls.next(workId, { block_ms: blockMs }, { headers, signal: item.signal, timeout: requestTimeout });
        } catch (err) {
          if (item.signal.aborted) break;
          if (isLeaseGone(err)) {
            lost = true;
            break;
          }
          throw err;
        }
        // No tool call in a whole wait window: this item is finished.
        if (!call) break;
        const result = await runCall(claim, call, item.signal);
        if (item.signal.aborted) break; // lease lost or stopping: discard
        try {
          await postResult(c, call.tool_call_id, result, headers);
          remember(call.tool_call_id, result);
        } catch (err) {
          if (isLeaseGone(err)) {
            lost = true;
            break;
          }
          throw err;
        }
      }
    } catch (err) {
      failure = (err as Error)?.message ?? String(err);
      log(`work ${workId} failed: ${failure}`);
    } finally {
      clearInterval(renew);
      outer?.removeEventListener("abort", onOuterAbort);
    }

    if (lost) return; // another worker owns it: post nothing more
    if (outer?.aborted) {
      await c.work.release(workId, { lease_generation: generation }, { timeout: 15_000 }).catch(() => undefined);
      log(`released ${workId}`);
      return;
    }
    try {
      await c.work.complete(
        workId,
        failure ? { lease_generation: generation, outcome: "failed", error_message: failure.slice(0, 2000) } : { lease_generation: generation, outcome: "success" },
        { timeout: 15_000 },
      );
      log(`completed ${workId}`);
    } catch (err) {
      if (!isLeaseStale(err)) throw err;
    }
  }

  async function runCall(claim: WorkClaim, call: ToolCall, signal: AbortSignal): Promise<ToolResult> {
    const cached = done.get(call.tool_call_id);
    if (cached) return cached;
    const handler = Object.prototype.hasOwnProperty.call(options.tools, call.name) ? options.tools[call.name] : undefined;
    if (!handler) {
      return { output: `No handler for tool "${call.name}" on this worker.`, is_error: true };
    }
    try {
      const value = await handler(call.input ?? {}, {
        workId: claim.work_id,
        sessionId: claim.session_id,
        toolCallId: call.tool_call_id,
        name: call.name,
        policy: claim.environment,
        signal,
      });
      return { output: toText(value), is_error: false };
    } catch (err) {
      return { output: (err as Error)?.message ?? String(err), is_error: true };
    }
  }

  async function postResult(c: SelfHostClient, id: string, result: ToolResult, headers: Record<string, string>): Promise<void> {
    const body = { output: truncateUtf8(result.output), is_error: result.is_error };
    try {
      await c.toolCalls.submitToolResult(id, body, { headers, timeout: 30_000 });
    } catch (err) {
      if (!(err instanceof APIError) || err.status !== 413) throw err;
      // The server counts bytes differently than expected: cut harder once.
      await c.toolCalls.submitToolResult(id, { output: truncateUtf8(result.output, MAX_TOOL_RESULT_BYTES / 2), is_error: result.is_error }, { headers, timeout: 30_000 });
    }
  }
}
