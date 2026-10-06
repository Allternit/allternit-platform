/**
 * MCP client transport over a child process Desktop already spawned (so it
 * keeps the `spawnSidecar` lifeline and restart policy). Framing comes from the
 * official SDK (`ReadBuffer` / `serializeMessage`), so the same Client speaks
 * MCP 2026-07-28 (`server/discover`) and the legacy `initialize` handshake.
 *
 * It exposes `pid` and `stderr` so the SDK classifies it as a stdio transport:
 * a probe the server ignores (timeout) or one that makes it exit (closed) means
 * "legacy server", never an outage.
 */
import type { Readable, Writable } from 'node:stream';
import {
  Client,
  ReadBuffer,
  serializeMessage,
  type JSONRPCMessage,
  type Transport,
} from '@modelcontextprotocol/client';

export interface McpProc {
  stdin: Writable;
  stdout: Readable;
  stderr?: Readable | null;
  pid?: number;
  kill(): void;
  onExit(cb: () => void): void;
}

export class ProcStdioTransport implements Transport {
  onclose?: () => void;
  onerror?: (error: Error) => void;
  onmessage?: (message: JSONRPCMessage) => void;
  private readonly buffer = new ReadBuffer();
  private closed = false;

  constructor(private readonly proc: McpProc) {}

  get pid(): number | null {
    return this.proc.pid ?? null;
  }

  get stderr(): Readable | null {
    return this.proc.stderr ?? null;
  }

  async start(): Promise<void> {
    this.proc.stdout.on('data', (chunk: Buffer | string) => {
      this.buffer.append(typeof chunk === 'string' ? Buffer.from(chunk, 'utf8') : chunk);
      for (;;) {
        let message: JSONRPCMessage | null;
        try {
          message = this.buffer.readMessage();
        } catch (error) {
          // Non-JSON noise on stdout: report and skip the line.
          this.onerror?.(error as Error);
          continue;
        }
        if (!message) break;
        this.onmessage?.(message);
      }
    });
    this.proc.stdout.on('error', (error: Error) => this.onerror?.(error));
    this.proc.onExit(() => this.markClosed());
  }

  async send(message: JSONRPCMessage): Promise<void> {
    if (this.closed) throw new Error('MCP server process is not running');
    await new Promise<void>((resolve, reject) =>
      this.proc.stdin.write(serializeMessage(message), (error) => (error ? reject(error) : resolve())),
    );
  }

  async close(): Promise<void> {
    if (this.closed) return;
    try {
      this.proc.stdin.end();
    } catch {
      /* already closed */
    }
    this.markClosed();
  }

  private markClosed(): void {
    if (this.closed) return;
    this.closed = true;
    this.buffer.clear();
    this.onclose?.();
  }
}

/** Probe budget for `server/discover` before treating a silent stdio server as legacy. */
export const MCP_PROBE_TIMEOUT_MS = 5_000;

/**
 * A Client that negotiates the era: 2026-07-28 when the server answers
 * `server/discover`, otherwise the legacy `initialize` handshake.
 */
export function createNegotiatingClient(name: string, version: string, mode: 'auto' | 'legacy' = 'auto'): Client {
  return new Client(
    { name, version },
    { versionNegotiation: { mode, probe: { timeoutMs: MCP_PROBE_TIMEOUT_MS } } } as ConstructorParameters<typeof Client>[1],
  );
}
