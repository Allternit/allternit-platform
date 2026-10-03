/**
 * Pure helpers for cable-free Android pairing (Android 11+ Wireless debugging).
 *
 * Nothing here spawns a process or touches the network: the QR payload, the
 * `adb mdns services` parser and the `adb devices -l` parser are all string
 * functions so they can be unit tested without a phone.
 */

import { randomBytes } from 'node:crypto';

export const ADB_PAIRING_SERVICE = '_adb-tls-pairing._tcp';
export const ADB_CONNECT_SERVICE = '_adb-tls-connect._tcp';

export interface QrPairing {
  /** mDNS instance name the phone will advertise after scanning (the `S:` field). */
  name: string;
  /** One-time pairing secret (the `P:` field). */
  code: string;
  /** The exact string to encode in the QR image. */
  payload: string;
}

export interface MdnsService {
  name: string;
  type: string;
  host: string;
  port: number;
}

export type AdbDeviceState = 'device' | 'offline' | 'unauthorized' | 'connecting' | 'unknown';

export interface AdbDeviceRow {
  serial: string;
  state: AdbDeviceState;
  model?: string;
  /** True for `host:port` (wireless) serials and `adb-…._adb-tls-connect._tcp` mdns serials. */
  wireless: boolean;
}

const ALNUM = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ23456789';

function randomAlnum(length: number): string {
  const bytes = randomBytes(length);
  let out = '';
  for (let i = 0; i < length; i += 1) out += ALNUM[bytes[i] % ALNUM.length];
  return out;
}

/** Escape the characters the Android WIFI: QR grammar reserves. */
export function escapeQrField(value: string): string {
  return value.replace(/([\\;,:"])/g, '\\$1');
}

/** `WIFI:T:ADB;S:<name>;P:<code>;;` — what Settings → "Pair device with QR code" reads. */
export function buildQrPayload(name: string, code: string): string {
  return `WIFI:T:ADB;S:${escapeQrField(name)};P:${escapeQrField(code)};;`;
}

/** Fresh name+code per pairing attempt. Never reuse: the code is the credential. */
export function generateQrPairing(): QrPairing {
  const name = `allternit-${randomAlnum(8)}`;
  const code = randomAlnum(12);
  return { name, code, payload: buildQrPayload(name, code) };
}

export function isSixDigitCode(code: string): boolean {
  return /^\d{6}$/.test(code);
}

/** `192.168.1.20:37123` or `[fe80::1]:37123`. */
export function parseHostPort(value: string): { host: string; port: number } | null {
  const match = /^(\[[0-9a-fA-F:.%a-zA-Z0-9]+\]|[A-Za-z0-9.\-]+):(\d{1,5})$/.exec(value.trim());
  if (!match) return null;
  const port = Number(match[2]);
  if (port < 1 || port > 65535) return null;
  return { host: match[1], port };
}

/**
 * Parse `adb mdns services`:
 *
 *   List of discovered mdns services
 *   adb-R5CT1234	_adb-tls-connect._tcp	192.168.1.20:41234
 *   allternit-ab12CD34	_adb-tls-pairing._tcp	192.168.1.20:37123
 */
export function parseMdnsServices(output: string): MdnsService[] {
  const services: MdnsService[] = [];
  for (const rawLine of output.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith('List of')) continue;
    const parts = line.split(/\s+/);
    if (parts.length < 3) continue;
    const [name, type, endpoint] = parts;
    if (!type.startsWith('_')) continue;
    const hp = parseHostPort(endpoint);
    if (!hp) continue;
    services.push({ name, type, host: hp.host, port: hp.port });
  }
  return services;
}

export function findPairingService(services: MdnsService[], name: string): MdnsService | undefined {
  return services.find((s) => s.type === ADB_PAIRING_SERVICE && s.name === name);
}

/** Connect port for the phone at `host` (the port changes every time Wireless debugging is toggled). */
export function findConnectService(services: MdnsService[], host: string): MdnsService | undefined {
  return services.find((s) => s.type === ADB_CONNECT_SERVICE && s.host === host);
}

function stateOf(token: string): AdbDeviceState {
  switch (token) {
    case 'device':
    case 'offline':
    case 'unauthorized':
    case 'connecting':
      return token;
    default:
      return 'unknown';
  }
}

/** Parse `adb devices -l`. USB rows are returned too; callers filter on `wireless`. */
export function parseAdbDevices(output: string): AdbDeviceRow[] {
  const rows: AdbDeviceRow[] = [];
  for (const rawLine of output.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith('List of') || line.startsWith('*')) continue;
    const parts = line.split(/\s+/);
    if (parts.length < 2) continue;
    const [serial, state, ...rest] = parts;
    const model = rest.find((p) => p.startsWith('model:'))?.slice('model:'.length);
    rows.push({
      serial,
      state: stateOf(state),
      model,
      wireless: /:\d+$/.test(serial) || serial.includes('._adb-tls-connect._tcp'),
    });
  }
  return rows;
}

/** Output of `adb pair`: success is "Successfully paired to host:port [guid=…]". */
export function pairSucceeded(output: string): boolean {
  return /successfully paired/i.test(output);
}

/** Output of `adb connect`: "connected to h:p" or "already connected to h:p". */
export function connectSucceeded(output: string): boolean {
  return /\b(already )?connected to\b/i.test(output) && !/failed|cannot|unable/i.test(output);
}
