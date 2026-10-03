/**
 * Phone device manager (Lane 2a): Android phones over Wi-Fi, no cable.
 *
 * Owns pairing (QR or 6-digit code), the paired-device store, the keep-alive
 * and reconnect loop, device info (model, Android version, battery, SIMs) and
 * the scrcpy window. adb/scrcpy are injected so the state machine is tested
 * with a fake adb and no phone.
 */

import type { ChildProcess } from 'node:child_process';
import { EventEmitter } from 'node:events';
import {
  connectSucceeded,
  findConnectService,
  findPairingService,
  generateQrPairing,
  isSixDigitCode,
  pairSucceeded,
  parseAdbDevices,
  parseHostPort,
  parseMdnsServices,
  type AdbDeviceRow,
  type AdbDeviceState,
  type QrPairing,
} from './phone-adb-pairing.js';
import type { AdbCall } from './phone-tools.js';

export type PhoneConnState = 'connecting' | 'online' | 'unauthorized' | 'lost' | 'reconnecting';

export type PhoneEvent =
  | { type: 'adb_seen'; adbState: AdbDeviceState }
  | { type: 'adb_missing' }
  | { type: 'reconnect_started' }
  | { type: 'reconnect_failed' };

/** Pure transition table; the manager only ever mutates state through this. */
export function nextPhoneState(state: PhoneConnState, event: PhoneEvent): PhoneConnState {
  switch (event.type) {
    case 'adb_seen':
      if (event.adbState === 'device') return 'online';
      if (event.adbState === 'unauthorized') return 'unauthorized';
      if (event.adbState === 'connecting') return state === 'online' ? 'online' : 'connecting';
      // offline / unknown: the transport is dead even though adb remembers it.
      return state === 'reconnecting' ? 'reconnecting' : 'lost';
    case 'adb_missing':
      return state === 'reconnecting' ? 'reconnecting' : 'lost';
    case 'reconnect_started':
      return state === 'online' ? 'online' : 'reconnecting';
    case 'reconnect_failed':
      return state === 'online' ? 'online' : 'lost';
  }
}

export interface StoredPhone {
  /** Stable id (hardware serial when readable, else the mDNS instance name). */
  id: string;
  name: string;
  host: string;
  port: number;
  pairedAt: number;
}

export interface PhoneStore {
  load(): StoredPhone[];
  save(phones: StoredPhone[]): void;
}

export interface PhoneSim {
  subscriptionId: number | null;
  slot: number | null;
  carrier: string;
}

export interface PhoneInfo {
  serial: string;
  id: string;
  name: string;
  model: string | null;
  androidVersion: string | null;
  battery: number | null;
  sims: PhoneSim[];
  state: PhoneConnState;
  host: string;
  port: number;
  streaming: boolean;
}

export type PairingPhase = 'idle' | 'waiting_for_scan' | 'pairing' | 'connecting' | 'done' | 'failed';

export interface PairingStatus {
  phase: PairingPhase;
  /** QR payload to render while waiting. Absent for code pairing. */
  qrPayload?: string;
  serial?: string;
  error?: string;
}

export interface PhoneManagerDeps {
  adb: AdbCall;
  store: PhoneStore;
  /** Spawns scrcpy as an owned sidecar; null when scrcpy isn't installed. */
  spawnScrcpy: ((serial: string, title: string) => ChildProcess) | null;
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
}

const POLL_MS = 5_000;
const MAX_BACKOFF_MS = 60_000;
export const QR_PAIRING_TIMEOUT_MS = 120_000;

interface Record_ {
  stored: StoredPhone;
  adbSerial: string;
  state: PhoneConnState;
  model: string | null;
  androidVersion: string | null;
  battery: number | null;
  sims: PhoneSim[];
  attempts: number;
  nextAttemptAt: number;
  scrcpy: ChildProcess | null;
}

export class PhoneDeviceManager extends EventEmitter {
  private phones = new Map<string, Record_>();
  private timer: ReturnType<typeof setInterval> | null = null;
  private polling = false;
  private pairing: PairingStatus = { phase: 'idle' };
  private pairingRun = 0;
  private readonly now: () => number;
  private readonly sleep: (ms: number) => Promise<void>;

  constructor(private readonly deps: PhoneManagerDeps) {
    super();
    this.now = deps.now ?? Date.now;
    this.sleep = deps.sleep ?? ((ms) => new Promise((r) => setTimeout(r, ms)));
    for (const stored of deps.store.load()) this.phones.set(stored.id, this.makeRecord(stored, `${stored.host}:${stored.port}`, 'lost'));
  }

  // ── lifecycle ─────────────────────────────────────────────────────────────

  start(): void {
    if (this.timer) return;
    void this.poll();
    this.timer = setInterval(() => void this.poll(), POLL_MS);
    this.timer.unref?.();
  }

  stopAll(): void {
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
    this.pairingRun += 1; // cancels any QR wait
    for (const rec of this.phones.values()) this.stopScrcpyRecord(rec);
  }

  // ── queries ───────────────────────────────────────────────────────────────

  list(): PhoneInfo[] {
    return [...this.phones.values()].map((r) => ({
      serial: r.adbSerial,
      id: r.stored.id,
      name: r.stored.name,
      model: r.model,
      androidVersion: r.androidVersion,
      battery: r.battery,
      sims: r.sims,
      state: r.state,
      host: r.stored.host,
      port: r.stored.port,
      streaming: Boolean(r.scrcpy && r.scrcpy.exitCode === null),
    }));
  }

  onlineSerials(): string[] {
    return [...this.phones.values()].filter((r) => r.state === 'online').map((r) => r.adbSerial);
  }

  pairingStatus(): PairingStatus {
    return { ...this.pairing };
  }

  // ── pairing ───────────────────────────────────────────────────────────────

  /** Begin QR pairing. Resolves immediately with the payload; progress via pairingStatus()/'pairing' events. */
  startQrPairing(): QrPairing {
    const qr = generateQrPairing();
    const run = ++this.pairingRun;
    this.setPairing({ phase: 'waiting_for_scan', qrPayload: qr.payload });
    void this.runQrPairing(run, qr);
    return qr;
  }

  cancelPairing(): void {
    this.pairingRun += 1;
    this.setPairing({ phase: 'idle' });
  }

  private async runQrPairing(run: number, qr: QrPairing): Promise<void> {
    const deadline = this.now() + QR_PAIRING_TIMEOUT_MS;
    try {
      while (this.now() < deadline) {
        if (run !== this.pairingRun) return;
        const mdns = await this.deps.adb(['mdns', 'services']);
        const svc = findPairingService(parseMdnsServices(mdns.stdout), qr.name);
        if (svc) {
          if (run !== this.pairingRun) return;
          this.setPairing({ phase: 'pairing' });
          await this.pairAndConnect(run, svc.host, svc.port, qr.code);
          return;
        }
        await this.sleep(1_000);
      }
      if (run === this.pairingRun) this.setPairing({ phase: 'failed', error: 'The QR code was not scanned in time. Try again.' });
    } catch (error) {
      if (run === this.pairingRun) this.setPairing({ phase: 'failed', error: error instanceof Error ? error.message : String(error) });
    }
  }

  /** Fallback: the six-digit code from "Pair device with pairing code" plus its `host:port`. */
  async pairWithCode(hostPort: string, code: string): Promise<PairingStatus> {
    const hp = parseHostPort(hostPort);
    if (!hp) {
      this.setPairing({ phase: 'failed', error: 'Enter the pairing address as shown on the phone, like 192.168.1.20:37123.' });
      return this.pairingStatus();
    }
    if (!isSixDigitCode(code)) {
      this.setPairing({ phase: 'failed', error: 'The pairing code is six digits.' });
      return this.pairingStatus();
    }
    const run = ++this.pairingRun;
    this.setPairing({ phase: 'pairing' });
    try {
      await this.pairAndConnect(run, hp.host, hp.port, code);
    } catch (error) {
      this.setPairing({ phase: 'failed', error: error instanceof Error ? error.message : String(error) });
    }
    return this.pairingStatus();
  }

  private async pairAndConnect(run: number, host: string, pairPort: number, code: string): Promise<void> {
    const pair = await this.deps.adb(['pair', `${host}:${pairPort}`, code], { timeoutMs: 20_000 });
    if (!pairSucceeded(pair.stdout + pair.stderr)) {
      throw new Error('The phone rejected the pairing. Check the code and that both are on the same Wi-Fi.');
    }
    if (run !== this.pairingRun) return;
    this.setPairing({ phase: 'connecting' });

    const connected = await this.connectHost(host, null);
    if (!connected) throw new Error('Paired, but could not connect. Make sure Wireless debugging is still on.');
    const row = await this.waitForOnline(connected);
    if (!row) throw new Error('Paired and connected, but the phone did not authorize this computer.');

    const id = (await this.readProp(connected, 'ro.serialno')) ?? connected;
    const hp = parseHostPort(connected) ?? { host, port: pairPort };
    const model = await this.readProp(connected, 'ro.product.model');
    const stored: StoredPhone = { id, name: model ?? 'Android phone', host: hp.host, port: hp.port, pairedAt: this.now() };
    const rec = this.makeRecord(stored, connected, 'online');
    this.phones.set(id, rec);
    this.persist();
    await this.refreshInfo(rec);
    this.setPairing({ phase: 'done', serial: connected });
    this.emit('devices');
  }

  /** `adb connect` to the phone's connect port (found over mDNS; falls back to a stored port). Returns the adb serial. */
  private async connectHost(host: string, fallbackPort: number | null): Promise<string | null> {
    const mdns = await this.deps.adb(['mdns', 'services']);
    const svc = findConnectService(parseMdnsServices(mdns.stdout), host);
    const port = svc?.port ?? fallbackPort;
    if (!port) return null;
    const target = `${host}:${port}`;
    const res = await this.deps.adb(['connect', target], { timeoutMs: 15_000 });
    return connectSucceeded(res.stdout + res.stderr) ? target : null;
  }

  private async waitForOnline(serial: string): Promise<AdbDeviceRow | null> {
    for (let i = 0; i < 10; i += 1) {
      const rows = parseAdbDevices((await this.deps.adb(['devices', '-l'])).stdout);
      const row = rows.find((r) => r.serial === serial);
      if (row?.state === 'device') return row;
      await this.sleep(500);
    }
    return null;
  }

  // ── keep-alive ────────────────────────────────────────────────────────────

  /** One reconcile pass: read adb's view, apply transitions, retry lost phones with backoff. */
  async poll(): Promise<void> {
    if (this.polling) return;
    this.polling = true;
    try {
      const rows = parseAdbDevices((await this.deps.adb(['devices', '-l'])).stdout).filter((r) => r.wireless);
      for (const rec of this.phones.values()) {
        const row = rows.find((r) => r.serial === rec.adbSerial) ?? rows.find((r) => r.serial.startsWith(`${rec.stored.host}:`));
        if (row) {
          rec.adbSerial = row.serial;
          this.adoptPort(rec, row.serial);
        }
        const before = rec.state;
        rec.state = nextPhoneState(rec.state, row ? { type: 'adb_seen', adbState: row.state } : { type: 'adb_missing' });
        if (rec.state === 'online') {
          rec.attempts = 0;
          if (before !== 'online') await this.refreshInfo(rec);
        } else if (rec.state === 'lost') {
          await this.tryReconnect(rec);
        }
        if (before !== rec.state) this.emit('devices');
      }
    } finally {
      this.polling = false;
    }
  }

  private async tryReconnect(rec: Record_): Promise<void> {
    if (this.now() < rec.nextAttemptAt) return;
    rec.state = nextPhoneState(rec.state, { type: 'reconnect_started' });
    this.emit('devices');
    const serial = await this.connectHost(rec.stored.host, rec.stored.port);
    if (serial) {
      rec.adbSerial = serial;
      this.adoptPort(rec, serial);
      const row = await this.waitForOnline(serial);
      rec.state = nextPhoneState(rec.state, row ? { type: 'adb_seen', adbState: 'device' } : { type: 'reconnect_failed' });
    } else {
      rec.state = nextPhoneState(rec.state, { type: 'reconnect_failed' });
    }
    if (rec.state === 'online') {
      rec.attempts = 0;
      await this.refreshInfo(rec);
    } else {
      rec.attempts += 1;
      rec.nextAttemptAt = this.now() + Math.min(MAX_BACKOFF_MS, POLL_MS * 2 ** Math.min(rec.attempts, 4));
    }
    this.emit('devices');
  }

  /** Wireless debugging picks a new connect port each time it is toggled; remember the latest. */
  private adoptPort(rec: Record_, serial: string): void {
    const hp = parseHostPort(serial);
    if (hp && hp.port !== rec.stored.port) {
      rec.stored = { ...rec.stored, host: hp.host, port: hp.port };
      this.persist();
    }
  }

  forget(id: string): void {
    const rec = this.phones.get(id);
    if (!rec) return;
    this.stopScrcpyRecord(rec);
    void this.deps.adb(['disconnect', rec.adbSerial]);
    this.phones.delete(id);
    this.persist();
    this.emit('devices');
  }

  // ── device info ───────────────────────────────────────────────────────────

  private async readProp(serial: string, prop: string): Promise<string | null> {
    const res = await this.deps.adb(['shell', 'getprop', prop], { serial });
    const value = res.stdout.trim();
    return res.code === 0 && value ? value : null;
  }

  async refreshInfo(rec: Record_): Promise<void> {
    const serial = rec.adbSerial;
    rec.model = (await this.readProp(serial, 'ro.product.model')) ?? rec.model;
    rec.androidVersion = (await this.readProp(serial, 'ro.build.version.release')) ?? rec.androidVersion;
    const battery = await this.deps.adb(['shell', 'dumpsys', 'battery'], { serial });
    const level = /level:\s*(\d+)/.exec(battery.stdout);
    rec.battery = level ? Number(level[1]) : rec.battery;
    rec.sims = await this.readSims(serial);
  }

  /** SIMs from the telephony provider; falls back to the gsm.* properties on builds that restrict it. */
  async readSims(serial: string): Promise<PhoneSim[]> {
    const q = await this.deps.adb(
      ['shell', 'content', 'query', '--uri', 'content://telephony/siminfo', '--projection', '_id:sim_id:display_name:carrier_name'],
      { serial },
    );
    const sims: PhoneSim[] = [];
    for (const line of q.stdout.split(/\r?\n/)) {
      if (!line.startsWith('Row:')) continue;
      const field = (name: string) => new RegExp(`${name}=([^,]*)`).exec(line)?.[1]?.trim();
      const slot = Number(field('sim_id'));
      if (!Number.isFinite(slot) || slot < 0) continue; // sim_id -1 = no SIM in a slot
      sims.push({
        subscriptionId: Number.isFinite(Number(field('_id'))) ? Number(field('_id')) : null,
        slot,
        carrier: field('carrier_name') || field('display_name') || 'SIM',
      });
    }
    if (sims.length) return sims;
    const alpha = await this.readProp(serial, 'gsm.sim.operator.alpha');
    return alpha
      ? alpha.split(',').filter(Boolean).map((carrier, slot) => ({ subscriptionId: null, slot, carrier }))
      : [];
  }

  /** Default SMS subscription: the platform setting if present, else the only/first SIM with a known id. */
  async defaultSmsSubscription(serial: string): Promise<number | null> {
    const res = await this.deps.adb(['shell', 'settings', 'get', 'global', 'multi_sim_sms_subscription'], { serial });
    const configured = Number(res.stdout.trim());
    if (res.code === 0 && Number.isInteger(configured) && configured > 0) return configured;
    const rec = [...this.phones.values()].find((r) => r.adbSerial === serial);
    const known = rec?.sims.filter((s) => s.subscriptionId !== null) ?? [];
    return known.length === 1 ? known[0].subscriptionId : null;
  }

  // ── scrcpy (watch and take over) ──────────────────────────────────────────

  scrcpyAvailable(): boolean {
    return this.deps.spawnScrcpy !== null;
  }

  startScrcpy(serial: string): { ok: true } | { ok: false; error: string } {
    const rec = [...this.phones.values()].find((r) => r.adbSerial === serial);
    if (!rec || rec.state !== 'online') return { ok: false, error: 'That phone is not connected.' };
    if (!this.deps.spawnScrcpy) {
      return { ok: false, error: 'scrcpy is not installed. Install it (brew install scrcpy) to watch and control the phone; the bot works without it.' };
    }
    if (rec.scrcpy && rec.scrcpy.exitCode === null) return { ok: true };
    const child = this.deps.spawnScrcpy(serial, `Allternit · ${rec.stored.name}`);
    rec.scrcpy = child;
    child.once('exit', () => {
      if (rec.scrcpy === child) rec.scrcpy = null;
      this.emit('devices');
    });
    this.emit('devices');
    return { ok: true };
  }

  stopScrcpy(serial: string): void {
    const rec = [...this.phones.values()].find((r) => r.adbSerial === serial);
    if (rec) this.stopScrcpyRecord(rec);
  }

  private stopScrcpyRecord(rec: Record_): void {
    if (rec.scrcpy && rec.scrcpy.exitCode === null) rec.scrcpy.kill('SIGTERM');
    rec.scrcpy = null;
  }

  // ── internals ─────────────────────────────────────────────────────────────

  private makeRecord(stored: StoredPhone, adbSerial: string, state: PhoneConnState): Record_ {
    return { stored, adbSerial, state, model: null, androidVersion: null, battery: null, sims: [], attempts: 0, nextAttemptAt: 0, scrcpy: null };
  }

  private persist(): void {
    this.deps.store.save([...this.phones.values()].map((r) => r.stored));
  }

  private setPairing(status: PairingStatus): void {
    this.pairing = status;
    this.emit('pairing', this.pairingStatus());
  }
}
