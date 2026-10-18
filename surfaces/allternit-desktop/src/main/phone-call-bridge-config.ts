/**
 * Per-phone "Call audio" settings for the call bridge (phone-call-bridge.ts),
 * CONTRACT-linked-phone-voice C5. Kept on the phone's own stored record:
 * Android `StoredPhone.callBridge` (phones.json), iPhone
 * `IosLinkRecord.callBridge` (phone-link-ios.json).
 *
 * No Node or Electron imports: the hidden bridge window's script bundles
 * pickAudioDevice from here too.
 */

export interface CallBridgeConfig {
  /** "Bot answers calls": answer after one ring and put the bot on the line. */
  enabled: boolean;
  botId: string | null;
  /** Mac input that carries the caller's voice (the USB sound card's mic in). */
  inputDeviceId: string | null;
  inputLabel: string | null;
  /** Mac output that carries the bot's voice into the phone's mic. */
  outputDeviceId: string | null;
  outputLabel: string | null;
}

export const DEFAULT_CALL_BRIDGE: CallBridgeConfig = {
  enabled: false,
  botId: null,
  inputDeviceId: null,
  inputLabel: null,
  outputDeviceId: null,
  outputLabel: null,
};

const str = (v: unknown, max = 512): string | null => {
  if (typeof v !== 'string') return null;
  const t = v.trim();
  return t ? t.slice(0, max) : null;
};

/** Whatever the renderer sent, as a well-formed config (unknown keys dropped). */
export function sanitizeCallBridge(raw: unknown): CallBridgeConfig {
  const r = (raw && typeof raw === 'object' ? raw : {}) as Record<string, unknown>;
  return {
    enabled: r.enabled === true,
    botId: str(r.botId, 128),
    inputDeviceId: str(r.inputDeviceId),
    inputLabel: str(r.inputLabel, 256),
    outputDeviceId: str(r.outputDeviceId),
    outputLabel: str(r.outputLabel, 256),
  };
}

/** What still has to be set before the bridge may answer, or null when ready. */
export function callBridgeMissing(cfg: CallBridgeConfig | null | undefined): string | null {
  if (!cfg?.enabled) return 'Bot answers calls is off.';
  if (!cfg.botId) return 'Pick the bot that answers in Call audio.';
  if (!cfg.inputDeviceId && !cfg.inputLabel) return 'Pick the input (caller voice) in Call audio.';
  if (!cfg.outputDeviceId && !cfg.outputLabel) return 'Pick the output (bot voice) in Call audio.';
  return null;
}

export interface AudioDeviceLike {
  deviceId: string;
  label: string;
  kind: string;
}

/** Chromium prefixes the system default with "Default - "; the device is the same. */
function bareLabel(label: string): string {
  return label.replace(/^(default|communications)\s*-\s*/i, '').trim().toLowerCase();
}

/**
 * The device to use for one side of the cable. Device ids are salted per
 * origin and partition, so the id the settings sheet saved (app origin) does
 * not match the bridge window's ids: an exact id match wins, then the saved
 * label. Never falls back to "any device" — the wrong device would put the
 * Mac's own mic or speakers on a stranger's call.
 */
export function pickAudioDevice(
  devices: AudioDeviceLike[],
  kind: 'audioinput' | 'audiooutput',
  wantedId: string | null,
  wantedLabel: string | null,
): AudioDeviceLike | null {
  const ofKind = devices.filter((d) => d.kind === kind && d.deviceId !== 'default' && d.deviceId !== 'communications');
  if (wantedId) {
    const byId = ofKind.find((d) => d.deviceId === wantedId);
    if (byId) return byId;
  }
  if (wantedLabel) {
    const want = bareLabel(wantedLabel);
    const byLabel = ofKind.find((d) => bareLabel(d.label) === want);
    if (byLabel) return byLabel;
  }
  return null;
}
