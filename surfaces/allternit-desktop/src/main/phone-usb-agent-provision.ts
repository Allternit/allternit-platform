/**
 * Setting up a USB Agent board over its USB serial port (the board plugged
 * into this computer, not the phone). This is the only way a key gets onto a
 * board, so pairing needs the board in hand.
 *
 *   desk  → prov info
 *   board → prov info <boardId> <fw> provisioned=<0|1>
 *   desk  → prov set ssid <hex> | pass <hex> | host <ipv4> | port <n> | desk <id> | key <64 hex>
 *   board → prov ok | prov err <reason>
 *   desk  → prov commit      (board saves to flash and restarts, disarmed)
 *
 * Values are hex so spaces and odd characters in Wi-Fi names survive. The
 * port I/O is injected; serialPort() below is the production one.
 */

import { spawn } from 'node:child_process';
import * as fs from 'node:fs';
import * as os from 'node:os';

export interface SerialPort {
  writeLine(line: string): Promise<void>;
  readLine(timeoutMs: number): Promise<string | null>;
  close(): void;
}

export interface BoardIdentity {
  boardId: string;
  firmware: string;
  provisioned: boolean;
}

export interface ProvisionInput {
  ssid: string;
  password: string;
  host: string;
  port: number;
  /** This Desktop's id, advertised over mDNS so the board finds it if its IP changes. */
  deskId: string;
  keyHex: string;
}

const toHex = (s: string) => Buffer.from(s, 'utf8').toString('hex');

export function validateProvision(p: ProvisionInput): string | null {
  if (!p.ssid || Buffer.byteLength(p.ssid) > 32) return 'The Wi-Fi name must be 1–32 bytes.';
  if (Buffer.byteLength(p.password) > 63) return 'The Wi-Fi password is too long (63 bytes at most).';
  if (p.password.length > 0 && p.password.length < 8) return 'Wi-Fi passwords are at least 8 characters.';
  if (!/^(\d{1,3}\.){3}\d{1,3}$/.test(p.host) || p.host.split('.').some((o) => Number(o) > 255)) return 'This computer has no usable IPv4 address on the network.';
  if (!Number.isInteger(p.port) || p.port < 1024 || p.port > 65535) return 'Bad port.';
  if (!/^[a-z0-9-]{4,40}$/.test(p.deskId)) return 'Bad desktop id.';
  if (!/^[0-9a-f]{64}$/.test(p.keyHex)) return 'Bad key.';
  return null;
}

async function ask(port: SerialPort, line: string, timeoutMs = 3_000): Promise<string> {
  await port.writeLine(line);
  const deadline = Date.now() + timeoutMs;
  // Skip unrelated chatter (boot logs) until a prov reply.
  for (;;) {
    const left = deadline - Date.now();
    if (left <= 0) throw new Error('The board did not answer. Is the USB Agent firmware on it?');
    const reply = await port.readLine(left);
    if (reply === null) throw new Error('The board did not answer. Is the USB Agent firmware on it?');
    if (reply.startsWith('prov ')) return reply.trim();
  }
}

export async function readIdentity(port: SerialPort): Promise<BoardIdentity> {
  const reply = await ask(port, 'prov info');
  const m = /^prov info (usba-[0-9a-f]{12}) ([0-9A-Za-z.\-]{1,24}) provisioned=([01])$/.exec(reply);
  if (!m) throw new Error(`Unexpected answer from the board: ${reply.slice(0, 80)}`);
  return { boardId: m[1], firmware: m[2], provisioned: m[3] === '1' };
}

export async function provision(port: SerialPort, p: ProvisionInput): Promise<BoardIdentity> {
  const problem = validateProvision(p);
  if (problem) throw new Error(problem);
  const identity = await readIdentity(port);
  const lines = [
    `prov set ssid ${toHex(p.ssid)}`,
    `prov set pass ${toHex(p.password)}`,
    `prov set host ${p.host}`,
    `prov set port ${p.port}`,
    `prov set desk ${p.deskId}`,
    `prov set key ${p.keyHex}`,
  ];
  for (const line of lines) {
    const reply = await ask(port, line);
    if (reply !== 'prov ok') throw new Error(`The board refused a setting: ${reply.replace(/^prov err ?/, '') || 'unknown error'}.`);
  }
  const done = await ask(port, 'prov commit');
  if (done !== 'prov ok') throw new Error(`The board could not save: ${done.replace(/^prov err ?/, '')}.`);
  return identity;
}

// ── Production I/O ──────────────────────────────────────────────────────────

/** USB serial ports that could be a board. */
export function candidatePorts(platform: NodeJS.Platform = process.platform, list: (dir: string) => string[] = (d) => fs.readdirSync(d)): string[] {
  try {
    const entries = list('/dev');
    if (platform === 'darwin') return entries.filter((e) => /^cu\.usbmodem/.test(e)).map((e) => `/dev/${e}`);
    if (platform === 'linux') return entries.filter((e) => /^ttyACM\d+$/.test(e)).map((e) => `/dev/${e}`);
  } catch {
    /* no /dev */
  }
  return [];
}

/** Opens a USB CDC port raw (no echo, no line editing). macOS and Linux only. */
export async function serialPort(path: string): Promise<SerialPort> {
  const fd = fs.openSync(path, fs.constants.O_RDWR | fs.constants.O_NOCTTY | fs.constants.O_NONBLOCK);
  // stty works on its stdin, so configure the descriptor we already hold.
  await new Promise<void>((resolve, reject) => {
    const child = spawn('stty', ['raw', '-echo', '115200'], { stdio: [fd, 'ignore', 'pipe'] });
    let err = '';
    child.stderr?.on('data', (d: Buffer) => (err += d.toString()));
    child.once('error', reject);
    child.once('exit', (code) => (code === 0 ? resolve() : reject(new Error(`Could not configure ${path}: ${err.trim() || code}`))));
  });
  let buf = '';
  let closed = false;
  const chunk = Buffer.alloc(1024);
  return {
    async writeLine(line) {
      fs.writeSync(fd, `${line}\n`);
    },
    async readLine(timeoutMs) {
      const deadline = Date.now() + timeoutMs;
      while (!closed && Date.now() < deadline) {
        const i = buf.indexOf('\n');
        if (i >= 0) {
          const line = buf.slice(0, i).replace(/\r$/, '');
          buf = buf.slice(i + 1);
          return line;
        }
        try {
          const n = fs.readSync(fd, chunk, 0, chunk.length, null);
          if (n > 0) {
            buf += chunk.subarray(0, n).toString('latin1');
            if (buf.length > 64 * 1024) buf = buf.slice(-4096);
            continue;
          }
        } catch (error) {
          if ((error as NodeJS.ErrnoException).code !== 'EAGAIN') throw error;
        }
        await new Promise((r) => setTimeout(r, 20));
      }
      return null;
    },
    close() {
      if (closed) return;
      closed = true;
      fs.closeSync(fd);
    },
  };
}

/** This computer's IPv4 addresses on real networks (the board dials one of them). */
export function lanAddresses(ifaces = os.networkInterfaces()): string[] {
  const out: string[] = [];
  for (const [name, list] of Object.entries(ifaces)) {
    if (/^(lo|utun|awdl|llw|bridge|docker|veth|tailscale|vmnet)/.test(name)) continue;
    for (const a of list ?? []) if (a.family === 'IPv4' && !a.internal && !a.address.startsWith('169.254.')) out.push(a.address);
  }
  return out;
}
