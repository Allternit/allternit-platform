/**
 * Port forwarding for the Computers console (step 3.10, contract "Port forward").
 *
 * Desktop listens on 127.0.0.1:<local port>. Each TCP connection mints one
 * single-use ticket (`POST /api/v1/computers/:id/forward {remotePort}` ->
 * `{url, ticket, expiresInSeconds}`), opens one WebSocket to `url?ticket=` and
 * pumps raw bytes both ways (binary frames, no ready frame). The relay never
 * wakes a computer: 409 = asleep, 503 = offline. A WebSocket closed with 1011
 * means the node could not dial 127.0.0.1:<port> (the service stopped).
 *
 * The renderer reaches this only through `window.allternit.computerForward`
 * (preload); the types match allternit-ai `src/lib/computers/port-forward.ts`.
 */
import net from 'node:net';
import { BrowserWindow, app, ipcMain } from 'electron';
import log from 'electron-log';
import WebSocket from 'ws';
import { authManager } from './auth-manager.js';
import { URLS } from './config.js';

export type ForwardFailure = 'busy' | 'asleep' | 'offline' | 'service_stopped' | 'signed_out' | 'unavailable' | 'error';
export type ClosedReason = 'stopped' | 'service_stopped' | 'asleep' | 'offline' | 'quit';

export interface ForwardTunnel {
  id: string;
  computerId: string;
  computerName?: string;
  remotePort: number;
  localPort: number;
  label?: string;
  openedAt: string;
  bytesIn: number;
  bytesOut: number;
}

export interface ForwardRequest { computerId: string; remotePort: number; localPort?: number; label?: string }

export type ForwardOpenResult = { ok: true; tunnel: ForwardTunnel } | { ok: false; reason: ForwardFailure; port?: number };

const NEXT_FREE_TRIES = 20;

/**
 * Choose the local port. The default (same as the remote port) may slide to the
 * next free one; a port the person typed is taken exactly or reported busy.
 */
export async function pickLocalPort(
  tryListen: (port: number) => Promise<boolean>,
  want: number,
  explicit: boolean,
): Promise<number | null> {
  const tries = explicit ? 1 : NEXT_FREE_TRIES;
  for (let i = 0; i < tries && want + i <= 65535; i++) {
    if (await tryListen(want + i)) return want + i;
  }
  return null;
}

export function failureFromStatus(status: number, error?: string): ForwardFailure | null {
  if (status >= 200 && status < 300) return null;
  if (status === 401 || status === 403) return 'signed_out';
  if (status === 409 && (error === undefined || error === 'asleep')) return 'asleep';
  if (status === 503) return 'offline';
  if (status === 404 || status === 405 || status === 501) return 'unavailable';
  return 'error';
}

interface Ticket { url: string; ticket: string }

function cloudBase(): string {
  return (process.env.ALLTERNIT_CLOUD_API_URL || URLS.CLOUD_API).replace(/\/$/, '');
}

async function mintTicket(computerId: string, remotePort: number): Promise<{ ok: true; ticket: Ticket } | { ok: false; reason: ForwardFailure }> {
  const session = await authManager.getSession();
  if (!session) return { ok: false, reason: 'signed_out' };
  try {
    const res = await fetch(`${cloudBase()}/api/v1/computers/${encodeURIComponent(computerId)}/forward`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${session.accessToken}`, 'Content-Type': 'application/json' },
      body: JSON.stringify({ remotePort }),
      signal: AbortSignal.timeout(15_000),
    });
    const body = (await res.json().catch(() => ({}))) as { url?: string; ticket?: string; error?: string };
    const failure = failureFromStatus(res.status, body.error);
    if (failure) return { ok: false, reason: failure };
    if (!body.url || !body.ticket) return { ok: false, reason: 'error' };
    return { ok: true, ticket: { url: body.url, ticket: body.ticket } };
  } catch {
    return { ok: false, reason: 'error' };
  }
}

interface Live { tunnel: ForwardTunnel; server: net.Server; sockets: Set<net.Socket>; pre?: { ticket: Ticket; at: number }; notify: ReturnType<typeof setTimeout> | null }

class ForwardManager {
  private tunnels = new Map<string, Live>();
  private seq = 0;

  register(): void {
    ipcMain.handle('computer-forward:open', (_e, req: ForwardRequest) => this.open(req));
    ipcMain.handle('computer-forward:close', (_e, id: string) => this.close(id, 'stopped'));
    ipcMain.handle('computer-forward:list', () => [...this.tunnels.values()].map((l) => l.tunnel));
    app.on('before-quit', () => { for (const id of [...this.tunnels.keys()]) this.close(id, 'quit'); });
  }

  async open(req: ForwardRequest): Promise<ForwardOpenResult> {
    const remotePort = Number(req?.remotePort);
    if (!req?.computerId || !Number.isInteger(remotePort) || remotePort < 1 || remotePort > 65535) return { ok: false, reason: 'error' };
    // One tunnel per computer port: opening it again returns the one that is open.
    for (const l of this.tunnels.values()) if (l.tunnel.computerId === req.computerId && l.tunnel.remotePort === remotePort) return { ok: true, tunnel: l.tunnel };

    // Ask the relay first, so asleep and offline are told before a local port is taken.
    const first = await mintTicket(req.computerId, remotePort);
    if (!first.ok) return { ok: false, reason: first.reason };

    const want = Number.isInteger(req.localPort) && req.localPort! >= 1 && req.localPort! <= 65535 ? req.localPort! : remotePort;
    const server = net.createServer();
    const listen = (port: number) => new Promise<boolean>((resolve) => {
      const onError = () => resolve(false);
      server.once('error', onError);
      server.listen(port, '127.0.0.1', () => { server.off('error', onError); resolve(true); });
    });
    const localPort = await pickLocalPort(listen, want, want !== remotePort);
    if (localPort === null) { server.close(); return { ok: false, reason: 'busy', port: want }; }

    const id = `fwd_${Date.now().toString(36)}_${++this.seq}`;
    const tunnel: ForwardTunnel = { id, computerId: req.computerId, remotePort, localPort, label: req.label, openedAt: new Date().toISOString(), bytesIn: 0, bytesOut: 0 };
    const live: Live = { tunnel, server, sockets: new Set(), pre: { ticket: first.ticket, at: Date.now() }, notify: null };
    this.tunnels.set(id, live);
    server.on('error', (err) => log.warn(`[Forward] ${id} listener error:`, err));
    server.on('connection', (socket) => void this.carry(live, socket));
    this.emitUpdate(live);
    return { ok: true, tunnel };
  }

  close(id: string, reason: ClosedReason): void {
    const live = this.tunnels.get(id);
    if (!live) return;
    this.tunnels.delete(id);
    if (live.notify) clearTimeout(live.notify);
    live.server.close();
    for (const s of live.sockets) s.destroy();
    this.send({ type: 'closed', id, reason });
  }

  /** One TCP connection: a ticket, one WebSocket, bytes both ways. */
  private async carry(live: Live, socket: net.Socket): Promise<void> {
    live.sockets.add(socket);
    socket.on('close', () => live.sockets.delete(socket));
    socket.on('error', () => socket.destroy());
    socket.pause();
    let ticket: Ticket | null = null;
    if (live.pre && Date.now() - live.pre.at < 25_000) { ticket = live.pre.ticket; }
    live.pre = undefined;
    if (!ticket) {
      const minted = await mintTicket(live.tunnel.computerId, live.tunnel.remotePort);
      if (!minted.ok) {
        socket.destroy();
        if (minted.reason === 'asleep') this.close(live.tunnel.id, 'asleep');
        else if (minted.reason === 'offline') this.close(live.tunnel.id, 'offline');
        return;
      }
      ticket = minted.ticket;
    }
    const url = `${ticket.url}${ticket.url.includes('?') ? '&' : '?'}ticket=${encodeURIComponent(ticket.ticket)}`;
    const ws = new WebSocket(url);
    ws.binaryType = 'nodebuffer';
    ws.on('open', () => {
      socket.on('data', (buf) => { if (ws.readyState === WebSocket.OPEN) { ws.send(buf, { binary: true }); live.tunnel.bytesOut += buf.length; this.touch(live); } });
      socket.resume();
    });
    ws.on('message', (data) => {
      const buf = Buffer.isBuffer(data) ? data : Buffer.from(data as ArrayBuffer);
      live.tunnel.bytesIn += buf.length;
      socket.write(buf);
      this.touch(live);
    });
    ws.on('close', (code) => {
      // 1011: the computer could not reach 127.0.0.1:<port>. The service stopped, so the tunnel closes.
      if (code === 1011) this.close(live.tunnel.id, 'service_stopped');
      socket.end();
    });
    ws.on('error', () => socket.destroy());
    socket.on('close', () => { if (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING) ws.close(); });
  }

  /** Counters change on every packet: tell the renderer at most once a second. */
  private touch(live: Live): void {
    if (live.notify) return;
    live.notify = setTimeout(() => { live.notify = null; if (this.tunnels.has(live.tunnel.id)) this.emitUpdate(live); }, 1000);
  }

  private emitUpdate(live: Live): void { this.send({ type: 'update', tunnel: { ...live.tunnel } }); }

  private send(event: unknown): void {
    for (const w of BrowserWindow.getAllWindows()) if (!w.isDestroyed()) w.webContents.send('computer-forward:event', event);
  }
}

export const forwardManager = new ForwardManager();

// Registers its IPC channels when imported (like device-pairing-manager).
forwardManager.register();
