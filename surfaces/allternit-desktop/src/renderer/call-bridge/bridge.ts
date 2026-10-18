/**
 * The hidden call bridge window's script (bundled by scripts/build-call-bridge.cjs).
 *
 * Per live carrier call: join the LiveKit room as the caller, publish the
 * cable's input (the caller's voice) raw — no echo cancellation, noise
 * suppression or gain control, the phone already did that — and play every
 * remote audio track (the bot) into the cable's output with setSinkId, so it
 * goes into the phone's mic. Main drives it over `window.callBridgeHost`.
 */

import { Room, RoomEvent, Track, type RemoteTrack } from 'livekit-client';
import { pickAudioDevice, type AudioDeviceLike } from '../../main/phone-call-bridge-config.js';

interface Host {
  onCommand(cb: (cmd: { id: string; op: 'devices' | 'join' | 'leave'; args: unknown }) => void): void;
  reply(msg: { id: string; ok: boolean; value?: unknown; error?: string }): void;
  event(msg: { type: 'disconnected'; serial: string; reason: string } | { type: 'log'; message: string }): void;
  ready(): void;
}

interface DeviceWanted {
  inputDeviceId: string | null;
  inputLabel: string | null;
  outputDeviceId: string | null;
  outputLabel: string | null;
}

interface JoinArgs extends DeviceWanted {
  serial: string;
  room: string;
  livekitUrl: string;
  token: string;
  identity: string;
}

interface Live {
  room: Room;
  mic: MediaStreamTrack | null;
  players: HTMLMediaElement[];
}

type SinkElement = HTMLMediaElement & { setSinkId?: (id: string) => Promise<void> };

const host = (window as unknown as { callBridgeHost: Host }).callBridgeHost;
const live = new Map<string, Live>();

const log = (message: string) => host.event({ type: 'log', message });

let unlocked = false;

async function devices(): Promise<AudioDeviceLike[]> {
  let list = await navigator.mediaDevices.enumerateDevices();
  // Labels (and the full list) only appear once this page has used the microphone.
  if (!unlocked && list.every((d) => !d.label)) {
    unlocked = true;
    try {
      const probe = await navigator.mediaDevices.getUserMedia({ audio: true, video: false });
      for (const t of probe.getTracks()) t.stop();
      list = await navigator.mediaDevices.enumerateDevices();
    } catch (error) {
      log(`microphone access for device labels failed: ${String(error)}`);
    }
  }
  return list.map((d) => ({ deviceId: d.deviceId, label: d.label, kind: d.kind }));
}

async function resolveDevices(want: DeviceWanted) {
  const list = await devices();
  return {
    input: pickAudioDevice(list, 'audioinput', want.inputDeviceId, want.inputLabel),
    output: pickAudioDevice(list, 'audiooutput', want.outputDeviceId, want.outputLabel),
  };
}

function cleanup(entry: Live): void {
  entry.mic?.stop();
  for (const el of entry.players) {
    el.pause();
    el.srcObject = null;
    el.remove();
  }
  entry.players = [];
}

async function leave(serial: string): Promise<void> {
  const entry = live.get(serial);
  if (!entry) return;
  // Removed first: an intentional leave must not read as "room disconnected".
  live.delete(serial);
  cleanup(entry);
  await entry.room.disconnect(true).catch(() => {});
}

async function join(args: JoinArgs): Promise<void> {
  await leave(args.serial);
  const { input, output } = await resolveDevices(args);
  if (!input || !output) throw new Error('Call audio cable not found');

  const room = new Room({
    adaptiveStream: false,
    dynacast: false,
    disconnectOnPageLeave: false,
    stopLocalTrackOnUnpublish: true,
  });
  const entry: Live = { room, mic: null, players: [] };
  live.set(args.serial, entry);

  room.on(RoomEvent.TrackSubscribed, (track: RemoteTrack) => {
    if (track.kind !== Track.Kind.Audio) return;
    const el = track.attach() as SinkElement;
    el.autoplay = true;
    document.body.appendChild(el);
    entry.players.push(el);
    const sink = el.setSinkId?.(output.deviceId);
    void Promise.resolve(sink)
      .then(() => el.play())
      .catch((error: unknown) => log(`could not play the bot into the cable: ${String(error)}`));
  });
  room.on(RoomEvent.TrackUnsubscribed, (track: RemoteTrack) => {
    for (const el of track.detach()) {
      entry.players = entry.players.filter((p) => p !== el);
      el.remove();
    }
  });
  room.on(RoomEvent.Disconnected, (reason?: unknown) => {
    if (live.get(args.serial) !== entry) return; // our own leave
    live.delete(args.serial);
    cleanup(entry);
    host.event({ type: 'disconnected', serial: args.serial, reason: String(reason ?? 'disconnected') });
  });

  try {
    await room.connect(args.livekitUrl, args.token, { autoSubscribe: true });
    const stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        deviceId: { exact: input.deviceId },
        echoCancellation: false,
        noiseSuppression: false,
        autoGainControl: false,
      },
      video: false,
    });
    const mic = stream.getAudioTracks()[0];
    if (!mic) throw new Error('The cable input gave no audio');
    entry.mic = mic;
    await room.localParticipant.publishTrack(mic, { source: Track.Source.Microphone, name: 'caller', dtx: false });
    log(`joined ${args.room} as ${args.identity} (in: ${input.label}, out: ${output.label})`);
  } catch (error) {
    await leave(args.serial);
    throw error;
  }
}

host.onCommand((cmd) => {
  void (async () => {
    try {
      let value: unknown;
      if (cmd.op === 'devices') {
        const { input, output } = await resolveDevices(cmd.args as DeviceWanted);
        value = { input: input !== null, output: output !== null };
      } else if (cmd.op === 'join') {
        await join(cmd.args as JoinArgs);
      } else if (cmd.op === 'leave') {
        await leave((cmd.args as { serial: string }).serial);
      } else {
        throw new Error(`unknown command ${String((cmd as { op: unknown }).op)}`);
      }
      host.reply({ id: cmd.id, ok: true, value });
    } catch (error) {
      host.reply({ id: cmd.id, ok: false, error: error instanceof Error ? error.message : String(error) });
    }
  })();
});

host.ready();
