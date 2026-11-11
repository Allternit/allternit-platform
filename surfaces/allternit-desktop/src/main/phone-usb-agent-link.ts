/**
 * The Desktop end of USBA/1 and USBA/2: a TCP listener boards dial into over
 * Wi-Fi, or that the node pipes a relayed board into (contract
 * hardware/link/README.md section 4.4; the handshake stays end to end).
 *
 * It has to listen on the LAN (the board is on the phone, not this computer),
 * so nothing is trusted until the mutual handshake in
 * phone-usb-agent-protocol.ts passes for a board this computer set up. Unknown
 * boards, bad proofs, slow handshakes and oversized lines all just drop the
 * connection. One session per board; a new authenticated session replaces
 * the old one.
 *
 * Commands are sent one at a time. A command whose reply doesn't arrive is
 * reported `uncertain`, never ok: the board may or may not have acted, and
 * callers must look at the screen before trying again.
 *
 * USBA/2 sessions also check the board's side: every reply and state line
 * carries a tag under the session key. A missing or wrong tag ends the
 * session (`bad_tag`), so a relay in the path cannot forge an "ok" or an
 * armed state. Typed text goes sealed as `typex`. The display commands
 * (`face`, `prog`, `label`) are refused on USBA/1 without sending anything,
 * because USBA/1 firmware resets its idle timer on any command.
 */

import { EventEmitter } from 'node:events';
import * as net from 'node:net';
import {
  USBA_HEARTBEAT_MS,
  USBA_MAX_LINE,
  boardProof,
  commandLine,
  encodeCommand,
  equalHex,
  newNonceHex,
  parseAuth,
  parseHello,
  parseReply,
  parseState,
  sealType,
  serverProof,
  sessionKey,
  textKey,
  type BoardCommand,
  type BoardState,
  type Hello,
  type Reply,
  type UsbaVersion,
} from './phone-usb-agent-protocol.js';

export const HANDSHAKE_TIMEOUT_MS = 5_000;
export const COMMAND_TIMEOUT_MS = 4_000;
const MAX_UNAUTHENTICATED = 8;

export type SendResult =
  | { ok: true }
  | { ok: false; error: 'offline' | 'invalid' | 'disarmed' | 'usb_asleep' | 'rejected'; message: string }
  | { ok: false; error: 'uncertain'; message: string };

export interface BoardSessionInfo {
  boardId: string;
  version: UsbaVersion;
  firmware: string;
  resetReason: string;
  address: string;
  state: BoardState;
  connectedAt: number;
}

/** Key lookup: the hex key for a board this computer set up, else null. */
export type KeyLookup = (boardId: string) => string | null;

interface Session {
  info: BoardSessionInfo;
  socket: net.Socket;
  sk: string;
  /** USBA/2 text key for `typex`; null on USBA/1. */
  ek: string | null;
  seq: number;
  /** Highest state-line `n` applied on a USBA/2 session. */
  lastN: number;
  /** Serializes commands: the board answers in order and we never pipeline. */
  queue: Promise<unknown>;
  pending: { seq: number; resolve: (reply: Reply | null) => void } | null;
  heartbeat: ReturnType<typeof setInterval> | null;
}

/** Splits a byte stream into lines; reports overflow so the caller can drop the peer. */
export class LineReader {
  private buf = '';
  constructor(private readonly onLine: (line: string) => void, private readonly onOverflow: () => void) {}
  push(chunk: Buffer | string): void {
    this.buf += chunk.toString('latin1');
    let i: number;
    while ((i = this.buf.indexOf('\n')) >= 0) {
      const line = this.buf.slice(0, i).replace(/\r$/, '');
      this.buf = this.buf.slice(i + 1);
      if (line.length > USBA_MAX_LINE) return this.onOverflow();
      if (line.length) this.onLine(line);
    }
    if (this.buf.length > USBA_MAX_LINE) this.onOverflow();
  }
}

export class UsbAgentLink extends EventEmitter {
  private server: net.Server | null = null;
  private sessions = new Map<string, Session>();
  private unauthenticated = 0;

  constructor(
    private readonly keyFor: KeyLookup,
    private readonly opts: { handshakeTimeoutMs?: number; commandTimeoutMs?: number; heartbeatMs?: number; now?: () => number } = {},
  ) {
    super();
  }

  listen(port: number, host = '0.0.0.0'): Promise<number> {
    if (this.server) {
      const addr = this.server.address();
      return Promise.resolve(typeof addr === 'object' && addr ? addr.port : port);
    }
    const server = net.createServer((socket) => this.accept(socket));
    server.maxConnections = 32;
    this.server = server;
    return new Promise((resolve, reject) => {
      server.once('error', reject);
      server.listen(port, host, () => {
        server.off('error', reject);
        const addr = server.address();
        resolve(typeof addr === 'object' && addr ? addr.port : port);
      });
    });
  }

  close(): void {
    for (const s of this.sessions.values()) this.endSession(s, 'shutdown');
    this.server?.close();
    this.server = null;
  }

  sessionsInfo(): BoardSessionInfo[] {
    return [...this.sessions.values()].map((s) => ({ ...s.info, state: { ...s.info.state } }));
  }

  session(boardId: string): BoardSessionInfo | null {
    const s = this.sessions.get(boardId);
    return s ? { ...s.info, state: { ...s.info.state } } : null;
  }

  /** Drops a board's session (used when the board is forgotten). */
  disconnect(boardId: string): void {
    const s = this.sessions.get(boardId);
    if (s) this.endSession(s, 'forgotten');
  }

  // ── commands ──────────────────────────────────────────────────────────────

  send(boardId: string, cmd: BoardCommand): Promise<SendResult> {
    const s = this.sessions.get(boardId);
    if (!s) return Promise.resolve({ ok: false, error: 'offline', message: 'The USB Agent is not connected.' });
    if (s.info.version === 1 && (cmd.op === 'face' || cmd.op === 'prog' || cmd.op === 'label')) {
      return Promise.resolve({ ok: false, error: 'invalid', message: `This board runs USBA/1 firmware, which has no "${cmd.op}" command. Update its firmware.` });
    }
    const text = encodeCommand(cmd);
    if (text === null) return Promise.resolve({ ok: false, error: 'invalid', message: `Command out of bounds: ${cmd.op}.` });
    // USBA/2 never sends text in the clear: the relay reads USBA lines.
    const seal = s.ek !== null && cmd.op === 'type' ? { ek: s.ek, text: cmd.text } : null;
    const run = s.queue.then(() => this.sendNow(s, text, seal));
    s.queue = run.catch(() => undefined);
    return run;
  }

  private sendNow(s: Session, text: string, seal: { ek: string; text: string } | null): Promise<SendResult> {
    if (this.sessions.get(s.info.boardId) !== s) {
      return Promise.resolve({ ok: false, error: 'offline', message: 'The USB Agent disconnected.' });
    }
    s.seq += 1;
    const seq = s.seq;
    const command = seal ? sealType(seal.ek, seq, seal.text) : text;
    const timeoutMs = this.opts.commandTimeoutMs ?? COMMAND_TIMEOUT_MS;
    return new Promise<SendResult>((resolve) => {
      const timer = setTimeout(() => {
        s.pending = null;
        resolve({ ok: false, error: 'uncertain', message: `No answer to "${text.split(' ')[0]}" — it may or may not have happened. Look at the screen before retrying.` });
      }, timeoutMs);
      s.pending = {
        seq,
        resolve: (reply) => {
          clearTimeout(timer);
          s.pending = null;
          if (reply === null) {
            return resolve({ ok: false, error: 'uncertain', message: 'The USB Agent disconnected mid-command — it may or may not have happened.' });
          }
          if (reply.ok) return resolve({ ok: true });
          if (reply.reason === 'disarmed') {
            return resolve({ ok: false, error: 'disarmed', message: 'The USB Agent is disarmed. Press its button to let the bot use the phone.' });
          }
          if (reply.reason.startsWith('phone usb asleep')) {
            return resolve({ ok: false, error: 'usb_asleep', message: 'The phone is locked or asleep. Unlock it to continue.' });
          }
          resolve({ ok: false, error: 'rejected', message: `The USB Agent refused: ${reply.reason}.` });
        },
      };
      s.socket.write(`${commandLine(s.sk, seq, command)}\n`);
    });
  }

  // ── connections ───────────────────────────────────────────────────────────

  private accept(socket: net.Socket): void {
    if (this.unauthenticated >= MAX_UNAUTHENTICATED) {
      socket.destroy();
      return;
    }
    this.unauthenticated += 1;
    let counted = true;
    const uncount = () => {
      if (counted) this.unauthenticated -= 1;
      counted = false;
    };
    socket.setNoDelay(true);
    socket.setKeepAlive(true, 5_000);
    const address = `${socket.remoteAddress ?? '?'}:${socket.remotePort ?? '?'}`;
    const timer = setTimeout(() => socket.destroy(), this.opts.handshakeTimeoutMs ?? HANDSHAKE_TIMEOUT_MS);

    let hello: Hello | null = null;
    let key: string | null = null;
    let sn = '';
    let session: Session | null = null;

    const reader = new LineReader(
      (line) => {
        if (session) return this.onSessionLine(session, line);
        if (!hello) {
          hello = parseHello(line);
          key = hello ? this.keyFor(hello.boardId) : null;
          if (!hello || !key) {
            this.emit('rejected', { address, reason: hello ? 'unknown_board' : 'bad_hello', boardId: hello?.boardId });
            return socket.destroy();
          }
          sn = newNonceHex();
          socket.write(`challenge ${sn} ${serverProof(key, hello.boardId, hello.boardNonce, sn, hello.version)}\n`);
          return;
        }
        const proof = parseAuth(line);
        if (!proof || !key || !equalHex(proof, boardProof(key, hello.boardId, hello.boardNonce, sn, hello.version))) {
          this.emit('rejected', { address, reason: 'bad_proof', boardId: hello.boardId });
          return socket.destroy();
        }
        clearTimeout(timer);
        uncount();
        const ek = hello.version === 2 ? textKey(key, hello.boardNonce, sn) : null;
        session = this.startSession(socket, hello, sessionKey(key, hello.boardNonce, sn, hello.version), ek, address);
      },
      () => socket.destroy(),
    );

    socket.on('data', (chunk) => reader.push(chunk));
    socket.on('error', () => socket.destroy());
    socket.on('close', () => {
      clearTimeout(timer);
      uncount();
      if (session && this.sessions.get(session.info.boardId) === session) this.endSession(session, 'closed');
    });
  }

  private startSession(socket: net.Socket, hello: Hello, sk: string, ek: string | null, address: string): Session {
    const old = this.sessions.get(hello.boardId);
    if (old) this.endSession(old, 'replaced');
    const s: Session = {
      info: {
        boardId: hello.boardId,
        version: hello.version,
        firmware: hello.firmware,
        resetReason: hello.resetReason,
        address,
        state: { armed: false, usb: 'none' },
        connectedAt: (this.opts.now ?? Date.now)(),
      },
      socket,
      sk,
      ek,
      seq: 0,
      lastN: 0,
      queue: Promise.resolve(),
      pending: null,
      heartbeat: null,
    };
    this.sessions.set(hello.boardId, s);
    socket.write('ready\n');
    // The board's lease runs out without traffic; an idle link pings to keep it.
    s.heartbeat = setInterval(() => {
      if (!s.pending) void this.send(s.info.boardId, { op: 'ping' });
    }, this.opts.heartbeatMs ?? USBA_HEARTBEAT_MS);
    s.heartbeat.unref?.();
    this.emit('session', this.session(hello.boardId));
    return s;
  }

  private onSessionLine(s: Session, line: string): void {
    if (s.info.version === 1) {
      const state = parseState(line);
      if (state) return this.applyState(s, state);
      if (line.startsWith('r ') && s.pending) {
        const reply = parseReply(line);
        if (reply && reply.seq === s.pending.seq) s.pending.resolve(reply);
      }
      return;
    }
    if (line.startsWith('s ')) {
      const signed = parseState(line, 2, s.sk);
      if (!signed) return this.endSession(s, 'bad_tag');
      const { n, ...state } = signed;
      // A replayed or reordered state line never moves the state backwards.
      if (n === undefined || n <= s.lastN) return;
      s.lastN = n;
      return this.applyState(s, state);
    }
    if (line.startsWith('r ')) {
      const reply = parseReply(line, 2, s.sk);
      if (!reply) return this.endSession(s, 'bad_tag');
      if (s.pending && reply.seq === s.pending.seq) s.pending.resolve(reply);
    }
  }

  private applyState(s: Session, state: BoardState): void {
    s.info.state = state;
    this.emit('state', { boardId: s.info.boardId, state: { ...state } });
  }

  private endSession(s: Session, reason: string): void {
    if (s.heartbeat) clearInterval(s.heartbeat);
    s.pending?.resolve(null);
    if (this.sessions.get(s.info.boardId) === s) this.sessions.delete(s.info.boardId);
    s.socket.destroy();
    this.emit('ended', { boardId: s.info.boardId, reason });
  }
}
