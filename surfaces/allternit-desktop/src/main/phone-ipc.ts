/** IPC for the Settings → Computers → Phones UI. All channels are trusted-sender guarded by the caller. */

import { BrowserWindow, type IpcMainInvokeEvent, type WebContents } from 'electron';
import { phoneService } from './phone-service.js';
import type { PhoneDeviceManager } from './phone-device-manager.js';
import type { UsbAgentManager } from './phone-usb-agent.js';
import type { StreamConsumer } from './phone-stream.js';
import type { ChannelConversation } from './phone-channel-readers.js';

type Guarded = (channel: string, fn: (event: IpcMainInvokeEvent, ...args: any[]) => unknown) => void;

function broadcast(channel: string, payload: unknown): void {
  for (const win of BrowserWindow.getAllWindows()) {
    if (!win.isDestroyed()) win.webContents.send(channel, payload);
  }
}

let wired = false;
function wire(manager: PhoneDeviceManager): void {
  if (wired) return;
  wired = true;
  manager.on('devices', () => broadcast('phone:devices', manager.list()));
  manager.on('pairing', (status) => broadcast('phone:pairing', status));
}

let bridgeWired = false;
/** Call bridge states and needs-you notices go to every window ('phone:call-bridge-event'). */
function wireCallBridge(): void {
  if (bridgeWired) return;
  bridgeWired = true;
  phoneService.ensureCallBridge().on('event', (ev) => broadcast('phone:call-bridge-event', ev));
}

let usbWired = false;
function wireUsb(usb: UsbAgentManager): void {
  if (usbWired) return;
  usbWired = true;
  usb.on('devices', () => broadcast('phone:usb-devices', usb.list()));
}

/** The USB Agent lane runs on macOS and Linux (UxPlay and the serial setup are Unix-only). */
const USB_SUPPORTED = process.platform === 'darwin' || process.platform === 'linux';

async function usbSnapshot() {
  if (!USB_SUPPORTED) return { supported: false as const };
  try {
    const usb = await phoneService.ensureUsbReady();
    wireUsb(usb);
    return {
      supported: true as const,
      ready: true as const,
      uxplay: usb.airplayAvailable(),
      firmwareVersion: (() => {
        try {
          return phoneService.usbFirmware().version;
        } catch {
          return null;
        }
      })(),
      airplayPin: usb.airplayPin(),
      boards: usb.list(),
    };
  } catch (error) {
    return { supported: true as const, ready: false as const, error: error instanceof Error ? error.message : String(error), boards: [] };
  }
}

async function usb(): Promise<UsbAgentManager> {
  const m = await phoneService.ensureUsbReady();
  wireUsb(m);
  return m;
}

const errorResult = (error: unknown) => ({ ok: false as const, message: error instanceof Error ? error.message : String(error) });

// ── Live stream push channel ────────────────────────────────────────────────
//
// Renderers subscribe per serial (phone:stream-subscribe); main keeps one
// manager consumer per serial and fans meta/chunk/end out to the subscribed
// senders only. Chunks travel as Buffers (structured-cloned to Uint8Array in
// the renderer) — base64 would add a copy + ~33% size on the hot path.
// Subscriptions die with their webContents ('destroyed').
// Audio rides the same subscription: 'phone:stream-audio-meta'
// (PhoneAudioMeta) and 'phone:stream-audio-chunk' (one encoded packet as a
// Buffer). A renderer that subscribes after the format was sent reads it
// with the 'phone:stream-audio-meta-get' invoke.

interface StreamSub {
  sender: WebContents;
  serial: string;
}

const streamSubs: StreamSub[] = [];
const streamConsumers = new Map<string, () => void>();

function pushStream(serial: string, channel: string, payload: unknown): void {
  for (const sub of streamSubs) {
    if (sub.serial !== serial || sub.sender.isDestroyed()) continue;
    sub.sender.send(channel, serial, payload);
  }
}

function ensureStreamConsumer(serial: string): void {
  if (streamConsumers.has(serial)) return;
  const manager = phoneService.getStreamManager();
  const consumer: StreamConsumer = {
    onMeta: (s, meta) => pushStream(s, 'phone:stream-meta', meta),
    onChunk: (s, chunk) => pushStream(s, 'phone:stream-chunk', chunk),
    onAudioMeta: (s, meta) => pushStream(s, 'phone:stream-audio-meta', meta),
    onAudioChunk: (s, chunk) => pushStream(s, 'phone:stream-audio-chunk', chunk),
    onEnd: (s, info) => pushStream(s, 'phone:stream-end', info),
  };
  streamConsumers.set(serial, manager.subscribe(serial, consumer));
}

function dropStreamConsumer(serial: string): void {
  if (streamSubs.some((sub) => sub.serial === serial)) return;
  streamConsumers.get(serial)?.();
  streamConsumers.delete(serial);
}

function subscribeStream(sender: WebContents, serial: string): void {
  // The manager exists without either lane (it costs nothing), so Android
  // serials and linked iPhones (`link:<deviceId>`) subscribe the same way;
  // startStream brings the right lane up.
  ensureStreamConsumer(serial);
  if (streamSubs.some((sub) => sub.sender === sender && sub.serial === serial)) return;
  streamSubs.push({ sender, serial });
  // A sender can hold many subscriptions; clean them all when it dies.
  sender.once('destroyed', () => {
    const serials = new Set<string>();
    for (let i = streamSubs.length - 1; i >= 0; i -= 1) {
      if (streamSubs[i].sender === sender) {
        serials.add(streamSubs[i].serial);
        streamSubs.splice(i, 1);
      }
    }
    for (const s of serials) dropStreamConsumer(s);
  });
}

function unsubscribeStream(sender: WebContents, serial: string): void {
  for (let i = streamSubs.length - 1; i >= 0; i -= 1) {
    if (streamSubs[i].sender === sender && streamSubs[i].serial === serial) streamSubs.splice(i, 1);
  }
  dropStreamConsumer(serial);
}

// ── User comms events ───────────────────────────────────────────────────────
//
// Global (not per serial): CommsEvent is `{ type: 'sms', address }` and the UI
// filters. The sms watcher in PhoneUserComms only polls while it has
// subscribers, so the main-side listener attaches on first subscriber and
// detaches when the last one leaves.

const commsSubs: WebContents[] = [];
let commsUnsub: (() => void) | null = null;

function pushComms(ev: unknown): void {
  for (const sender of commsSubs) {
    if (!sender.isDestroyed()) sender.send('phone:comms-event', ev);
  }
}

function subscribeComms(sender: WebContents): void {
  if (commsSubs.includes(sender)) return;
  commsSubs.push(sender);
  if (!commsUnsub) {
    phoneService
      .ensureComms()
      .then((comms) => {
        if (commsSubs.length === 0) return; // everyone left while adb came up
        commsUnsub = comms.onCommsEvent(pushComms);
      })
      .catch(() => {
        commsUnsub = null; // a later subscribe retries
      });
  }
  sender.once('destroyed', () => unsubscribeComms(sender));
}

function unsubscribeComms(sender: WebContents): void {
  const i = commsSubs.indexOf(sender);
  if (i >= 0) commsSubs.splice(i, 1);
  if (commsSubs.length === 0) {
    commsUnsub?.();
    commsUnsub = null;
  }
}

/** Renderer-supplied dial options: only confirmEmergency survives. */
function sanitizeDialOpts(raw: unknown): { confirmEmergency?: boolean } | undefined {
  if (!raw || typeof raw !== 'object') return undefined;
  const confirmEmergency = (raw as Record<string, unknown>).confirmEmergency;
  return confirmEmergency === undefined ? undefined : { confirmEmergency: Boolean(confirmEmergency) };
}

/** Renderer-supplied options are coerced and clamped; unknown fields are dropped. */
function sanitizeStreamOpts(raw: unknown): { maxSize?: number; videoBitrate?: number; maxFps?: number } {
  const opts: { maxSize?: number; videoBitrate?: number; maxFps?: number } = {};
  if (!raw || typeof raw !== 'object') return opts;
  const o = raw as Record<string, unknown>;
  const num = (value: unknown, min: number, max: number): number | undefined => {
    const n = Number(value);
    return Number.isFinite(n) && n >= min && n <= max ? n : undefined;
  };
  const maxSize = num(o.maxSize, 240, 4096);
  if (maxSize !== undefined) opts.maxSize = Math.trunc(maxSize);
  const videoBitrate = num(o.videoBitrate, 100_000, 80_000_000);
  if (videoBitrate !== undefined) opts.videoBitrate = Math.trunc(videoBitrate);
  const maxFps = num(o.maxFps, 1, 120);
  if (maxFps !== undefined) opts.maxFps = Math.trunc(maxFps);
  return opts;
}

async function snapshot() {
  const art = phoneService.getArtemis()?.status() ?? null;
  // Opening Settings shouldn't download adb for someone with no Android phone;
  // pairing (phone:pair-*) brings the lane up when they add one.
  if (!phoneService.androidInUse()) {
    return { ready: true as const, phones: [], pairing: { phase: 'idle' as const }, scrcpy: phoneService.scrcpyInstalled(), artemis: art };
  }
  try {
    const manager = await phoneService.ensureReady();
    wire(manager);
    return { ready: true as const, phones: manager.list(), pairing: manager.pairingStatus(), scrcpy: manager.scrcpyAvailable(), artemis: art };
  } catch (error) {
    return { ready: false as const, error: error instanceof Error ? error.message : String(error), phones: [], artemis: art };
  }
}

export function registerPhoneIpc(handle: Guarded): void {
  handle('phone:status', () => snapshot());
  handle('phone:pair-qr-start', async () => {
    const manager = await phoneService.ensureReady();
    wire(manager);
    return { payload: manager.startQrPairing().payload };
  });
  handle('phone:pair-code', async (_e, hostPort: unknown, code: unknown) => {
    const manager = await phoneService.ensureReady();
    wire(manager);
    return manager.pairWithCode(String(hostPort ?? ''), String(code ?? ''));
  });
  handle('phone:pair-cancel', async () => (await phoneService.ensureReady()).cancelPairing());
  handle('phone:forget', async (_e, id: unknown) => (await phoneService.ensureReady()).forget(String(id)));
  handle('phone:scrcpy-start', async (_e, serial: unknown) => (await phoneService.ensureReady()).startScrcpy(String(serial)));
  handle('phone:scrcpy-stop', async (_e, serial: unknown) => (await phoneService.ensureReady()).stopScrcpy(String(serial)));
  handle('phone:try-open-app', (_e, serial: unknown, app: unknown) => phoneService.tryOpenApp(String(serial), String(app)));
  handle('phone:screenshot', (_e, serial: unknown) => phoneService.tryScreenshot(String(serial)));
  handle('phone:home', (_e, serial: unknown) => phoneService.tryHome(String(serial)));

  // Channel readers: messaging apps on the linked phone, read by driving their UI.
  handle('phone:channel-apps', () => phoneService.channelApps());
  handle('phone:channel-cached', (_e, app: unknown, conversationId: unknown) =>
    phoneService.channelCached(String(app ?? ''), typeof conversationId === 'string' ? conversationId : undefined));
  handle('phone:channel-conversations', (_e, app: unknown) => phoneService.channelConversations(String(app ?? '')));
  handle('phone:channel-messages', (_e, app: unknown, conversation: unknown) => {
    const c = sanitizeConversation(conversation);
    return c ? phoneService.channelMessages(String(app ?? ''), c) : { ok: false, error: 'invalid_args', message: 'Pick a conversation.' };
  });
  handle('phone:channel-clip', (_e, app: unknown, conversation: unknown) => {
    const c = sanitizeConversation(conversation);
    return c ? phoneService.channelClip(String(app ?? ''), c) : { ok: false, error: 'invalid_args', message: 'Pick a conversation.' };
  });
  handle('phone:channel-cached-clip', (_e, app: unknown, id: unknown) => phoneService.channelCachedClip(String(app ?? ''), String(id ?? '')));
  handle('phone:channel-send', (_e, app: unknown, conversation: unknown, text: unknown) => {
    const c = sanitizeConversation(conversation);
    if (!c || typeof text !== 'string') return { ok: false, error: 'invalid_args', message: 'Pick a conversation and write a message.' };
    return phoneService.channelSend(String(app ?? ''), c, text);
  });

  // In-app live view (scrcpy-server video + input). A stream starts only on
  // explicit renderer request, never by itself.
  handle('phone:stream-start', (_e, serial: unknown, opts: unknown) => phoneService.startStream(String(serial ?? ''), sanitizeStreamOpts(opts)));
  handle('phone:stream-stop', (_e, serial: unknown) => phoneService.stopStream(String(serial ?? '')));
  handle('phone:stream-status', (_e, serial: unknown) => phoneService.streamStatus(String(serial ?? '')));
  handle('phone:stream-input', (_e, serial: unknown, events: unknown) => phoneService.sendStreamInput(String(serial ?? ''), events));
  handle('phone:stream-display-power', (_e, serial: unknown, on: unknown) => phoneService.setStreamDisplayPower(String(serial ?? ''), Boolean(on)));
  handle('phone:stream-audio-meta-get', (_e, serial: unknown) => phoneService.getStreamManager().audioMeta(String(serial ?? '')));
  handle('phone:stream-subscribe', (e, serial: unknown) => subscribeStream(e.sender, String(serial ?? '')));
  handle('phone:stream-unsubscribe', (e, serial: unknown) => unsubscribeStream(e.sender, String(serial ?? '')));

  // User comms (Phones 2.0 Phase 3): texts/calls through the linked phone's
  // own SIM. No serial on the bridge — the service resolves the user's phone.
  handle('phone:comms-sms-list', () => phoneService.smsList());
  handle('phone:comms-sms-messages', (_e, address: unknown, after: unknown) =>
    phoneService.smsMessages(String(address ?? ''), typeof after === 'number' && Number.isFinite(after) ? { afterTimestamp: after } : undefined));
  handle('phone:comms-sms-send', (_e, to: unknown, text: unknown) => phoneService.smsSendUser(String(to ?? ''), String(text ?? '')));
  handle('phone:comms-call-dial', (_e, number: unknown, opts: unknown) => phoneService.callDialUser(String(number ?? ''), sanitizeDialOpts(opts)));
  handle('phone:comms-call-end', () => phoneService.callEnd());
  handle('phone:comms-call-state', () => phoneService.callState());
  handle('phone:comms-list-apps', (_e, serial: unknown) => phoneService.listApps(String(serial ?? '')));
  handle('phone:comms-subscribe', (e) => subscribeComms(e.sender));

  // Call bridge (CONTRACT-linked-phone-voice C5): the bot answers carrier
  // calls on a phone wired to this Mac's sound card. `serial` is the phone
  // mirror's key (adb serial, or link:<deviceId> for an iPhone).
  handle('phone:call-bridge-get', async (_e, serial: unknown) => {
    const res = await phoneService.callBridgeGet(String(serial ?? ''));
    wireCallBridge();
    return res;
  });
  handle('phone:call-bridge-set', async (_e, serial: unknown, cfg: unknown) => {
    wireCallBridge();
    return phoneService.callBridgeSet(String(serial ?? ''), cfg);
  });
  handle('phone:call-bridge-status', async () => {
    const res = await phoneService.callBridgeStatus();
    wireCallBridge();
    return res;
  });
  handle('phone:comms-unsubscribe', (e) => unsubscribeComms(e.sender));

  // iPhone over an Allternit USB Agent
  handle('phone:usb-status', () => usbSnapshot());
  handle('phone:usb-ports', async () => (USB_SUPPORTED ? (await usb()).serialPorts() : []));
  handle('phone:usb-identify', async (_e, port: unknown) => {
    try {
      return { ok: true as const, ...(await (await usb()).identify(String(port))) };
    } catch (error) {
      return errorResult(error);
    }
  });
  handle('phone:usb-setup', async (_e, port: unknown, ssid: unknown, password: unknown, name: unknown) => {
    try {
      const board = await (await usb()).setUp(String(port), { ssid: String(ssid ?? ''), password: String(password ?? '') }, String(name ?? ''));
      return { ok: true as const, board };
    } catch (error) {
      return errorResult(error);
    }
  });
  // The firmware image for the setup screen to flash (esptool-js over Web Serial, Espressif boards only).
  handle('phone:usb-firmware', () => {
    try {
      const fw = phoneService.usbFirmware();
      return { ok: true as const, version: fw.version, sha256: fw.sha256, offset: fw.offset, data: fw.data };
    } catch (error) {
      return errorResult(error);
    }
  });
  handle('phone:usb-calibrate', async (_e, boardId: unknown) => (await usb()).calibrate(String(boardId)));
  handle('phone:usb-stop', async (_e, boardId: unknown) => (await usb()).stop(String(boardId)));
  handle('phone:usb-forget', async (_e, boardId: unknown) => (await usb()).forget(String(boardId)));

  handle('phone:artemis-install', async () => {
    await phoneService.ensureReady();
    return phoneService.getArtemis()?.install();
  });
}

/** A renderer-supplied conversation, reduced to the fields the reader uses. */
function sanitizeConversation(v: unknown): ChannelConversation | null {
  if (!v || typeof v !== 'object') return null;
  const c = v as Record<string, unknown>;
  const str = (x: unknown, max = 200) => (typeof x === 'string' && x.length > 0 && x.length <= max ? x : null);
  const id = str(c.id), title = str(c.title), group = str(c.group);
  if (!id || !title || !group) return null;
  return { id, title, group, preview: null, time: null, unread: 0 };
}
