// Wire protocol between the Zoom adapter (Node) and the Meeting SDK sidecar (C++).
// Full description: sidecar/README.md, section "Protocol". Keep the two in step.
//
// Transport: the sidecar's fd 3, a bidirectional socketpair Node creates with
// spawn(..., { stdio: ["ignore", "pipe", "pipe", "pipe"] }). stdout and stderr
// stay free for logs, because the Zoom SDK and Qt print to stdout on their own.
//
// Framing (both directions): [u32 big-endian length N][u8 type][N-1 body bytes].
//   N counts the type byte plus the body. N >= 1 and N <= MAX_FRAME_BYTES.
// Types:
//   0x01 JSON      UTF-8 JSON object (commands, results, events)
//   0x02 AUDIO_OUT sidecar -> node: [u32 BE sampleRate][u8 channels][s16le PCM]  (mixed call audio)
//   0x03 AUDIO_IN  node -> sidecar: [u32 BE sampleRate][u8 channels][s16le PCM]  (TTS into the virtual mic)
//   0x04 VIDEO_IN  node -> sidecar: [u16 BE width][u16 BE height][I420 planes]   (camera tile frames)

export const FRAME_JSON = 0x01;
export const FRAME_AUDIO_OUT = 0x02;
export const FRAME_AUDIO_IN = 0x03;
export const FRAME_VIDEO_IN = 0x04;

/** 16 MiB. A 1080p I420 frame is about 3 MiB; anything bigger is a corrupt stream. */
export const MAX_FRAME_BYTES = 16 * 1024 * 1024;

// ---- messages node -> sidecar ---------------------------------------------------

export type SidecarCommand =
  | {
      op: "join";
      id: number;
      /** Meeting SDK JWT (HS256, appKey/iat/exp/tokenExp), minted from the client id and secret. */
      sdkJwt: string;
      meetingNumber?: string;
      vanityId?: string;
      passcode?: string;
      displayName: string;
      /** OBF token of the user who authorised the app (GET /v2/users/me/token?type=onbehalf). */
      obfToken: string;
      /** Optional local-recording join token (GET /v2/meetings/{id}/jointoken/local_recording); grants raw-audio permission without asking the host. */
      appPrivilegeToken?: string;
    }
  | { op: "leave"; id: number }
  | { op: "chat_send"; id: number; text: string }
  | { op: "participants"; id: number }
  | { op: "tile_state"; id: number; state: string }
  | { op: "audio_clear"; id: number }
  | { op: "shutdown"; id: number };

/** Command without its id; the client assigns ids. */
type DistributiveOmit<T, K extends PropertyKey> = T extends unknown ? Omit<T, K> : never;
export type SidecarCommandInput = DistributiveOmit<SidecarCommand, "id">;

// ---- messages sidecar -> node ---------------------------------------------------

export interface WireParticipant {
  /** SDK user id (unsigned int), as a string. */
  id: string;
  name: string;
  self: boolean;
  host?: boolean;
}

/** Meeting status names the sidecar sends (from SDK MeetingStatus). */
export type WireMeetingStatus =
  | "idle"
  | "connecting"
  | "waiting_for_host"
  | "in_waiting_room"
  | "in_meeting"
  | "disconnecting"
  | "reconnecting"
  | "failed"
  | "ended"
  | "unknown";

export type SidecarEvent =
  | { type: "ready"; sdkVersion?: string }
  | { type: "result"; id: number; ok: boolean; error?: string; data?: unknown }
  | { type: "auth"; ok: boolean; code: number; name?: string }
  | {
      type: "status";
      status: WireMeetingStatus;
      /** iResult from onMeetingStatusChanged: a MeetingFailCode when failed, an EndMeetingReason when ended. */
      code: number;
      /** Symbolic name for known codes, e.g. MEETING_FAIL_PASSWORD_ERR or EndMeetingReason_KickByHost. */
      name?: string;
    }
  | {
      type: "chat";
      /** IChatMsgInfo::GetMessageID(); may be empty on older SDKs. */
      msgId: string;
      senderId: string;
      sender: string;
      text: string;
      /** Epoch seconds from IChatMsgInfo::GetTimeStamp(). */
      ts: number;
      self: boolean;
    }
  | { type: "participants"; list: WireParticipant[] }
  /** SDK user ids currently talking (IUserInfo::IsTalking poll). Sent on change. */
  | { type: "active_speaker"; ids: string[] }
  | { type: "recording"; state: "granted" | "requested" | "denied" | "timeout" | "started" | "failed"; code?: number }
  | { type: "mic"; state: "ready" | "started" | "stopped" | "gone" }
  | { type: "log"; level: "debug" | "info" | "warn" | "error"; msg: string };

export interface AudioFrame {
  sampleRate: number;
  channels: number;
  /** Signed 16-bit little-endian PCM, interleaved when channels > 1. */
  pcm: Uint8Array;
}

export interface VideoFrame {
  width: number;
  height: number;
  /** I420: Y plane (w*h), then U and V planes (w/2*h/2 each). */
  i420: Uint8Array;
}

export type DecodedFrame =
  | { kind: "json"; msg: unknown }
  | { kind: "audio_out"; audio: AudioFrame }
  | { kind: "audio_in"; audio: AudioFrame }
  | { kind: "video_in"; video: VideoFrame }
  | { kind: "unknown"; type: number; body: Uint8Array };

// ---- encoding -------------------------------------------------------------------

function frame(type: number, body: Uint8Array): Uint8Array {
  const n = body.length + 1;
  if (n > MAX_FRAME_BYTES) throw new Error(`frame too large: ${n} bytes`);
  const out = new Uint8Array(4 + n);
  const dv = new DataView(out.buffer);
  dv.setUint32(0, n, false);
  out[4] = type;
  out.set(body, 5);
  return out;
}

export function encodeJson(msg: object): Uint8Array {
  return frame(FRAME_JSON, new TextEncoder().encode(JSON.stringify(msg)));
}

function encodeAudio(type: number, a: AudioFrame): Uint8Array {
  if (!Number.isInteger(a.sampleRate) || a.sampleRate <= 0) throw new Error("bad sampleRate");
  if (!Number.isInteger(a.channels) || a.channels < 1 || a.channels > 2) throw new Error("bad channels");
  if (a.pcm.length % (2 * a.channels) !== 0) throw new Error("pcm length is not whole s16 samples");
  const body = new Uint8Array(5 + a.pcm.length);
  const dv = new DataView(body.buffer);
  dv.setUint32(0, a.sampleRate, false);
  body[4] = a.channels;
  body.set(a.pcm, 5);
  return frame(type, body);
}

export const encodeAudioIn = (a: AudioFrame): Uint8Array => encodeAudio(FRAME_AUDIO_IN, a);
/** Used by the fake sidecar in tests; the real sidecar writes this frame in C++. */
export const encodeAudioOut = (a: AudioFrame): Uint8Array => encodeAudio(FRAME_AUDIO_OUT, a);

export function encodeVideoIn(v: VideoFrame): Uint8Array {
  const { width: w, height: h } = v;
  if (!Number.isInteger(w) || !Number.isInteger(h) || w <= 0 || h <= 0 || w > 0xffff || h > 0xffff) throw new Error("bad frame size");
  if (w % 2 !== 0 || h % 2 !== 0) throw new Error("I420 needs even width and height");
  const expect = w * h + 2 * ((w / 2) * (h / 2));
  if (v.i420.length !== expect) throw new Error(`I420 length ${v.i420.length}, expected ${expect}`);
  const body = new Uint8Array(4 + v.i420.length);
  const dv = new DataView(body.buffer);
  dv.setUint16(0, w, false);
  dv.setUint16(2, h, false);
  body.set(v.i420, 4);
  return frame(FRAME_VIDEO_IN, body);
}

// ---- decoding -------------------------------------------------------------------

/**
 * Incremental frame decoder. Feed it chunks as they arrive (any split, any
 * coalescing); it returns every complete frame. Throws on a frame length of 0
 * or above MAX_FRAME_BYTES, or on JSON that does not parse: the stream is then
 * corrupt and the caller must drop the connection.
 */
export class FrameDecoder {
  private buf: Uint8Array = new Uint8Array(0);
  private len = 0;

  push(chunk: Uint8Array): DecodedFrame[] {
    this.append(chunk);
    const out: DecodedFrame[] = [];
    let off = 0;
    while (this.len - off >= 4) {
      const dv = new DataView(this.buf.buffer, this.buf.byteOffset + off, 4);
      const n = dv.getUint32(0, false);
      if (n < 1 || n > MAX_FRAME_BYTES) throw new Error(`bad frame length ${n}`);
      if (this.len - off < 4 + n) break;
      const type = this.buf[off + 4];
      // Copy the body out so callers may keep it after later pushes reuse the buffer.
      const body = this.buf.slice(off + 5, off + 4 + n);
      out.push(decodeBody(type, body));
      off += 4 + n;
    }
    if (off > 0) {
      this.buf.copyWithin(0, off, this.len);
      this.len -= off;
    }
    return out;
  }

  /** Bytes held waiting for the rest of a frame. */
  get pending(): number {
    return this.len;
  }

  private append(chunk: Uint8Array): void {
    const need = this.len + chunk.length;
    if (need > this.buf.length) {
      const next = new Uint8Array(Math.max(need, this.buf.length * 2, 4096));
      next.set(this.buf.subarray(0, this.len));
      this.buf = next;
    }
    this.buf.set(chunk, this.len);
    this.len = need;
  }
}

function decodeBody(type: number, body: Uint8Array): DecodedFrame {
  switch (type) {
    case FRAME_JSON:
      return { kind: "json", msg: JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(body)) };
    case FRAME_AUDIO_OUT:
    case FRAME_AUDIO_IN: {
      if (body.length < 5) throw new Error("short audio frame");
      const dv = new DataView(body.buffer, body.byteOffset, body.length);
      const audio: AudioFrame = { sampleRate: dv.getUint32(0, false), channels: body[4], pcm: body.subarray(5) };
      return type === FRAME_AUDIO_OUT ? { kind: "audio_out", audio } : { kind: "audio_in", audio };
    }
    case FRAME_VIDEO_IN: {
      if (body.length < 4) throw new Error("short video frame");
      const dv = new DataView(body.buffer, body.byteOffset, body.length);
      return { kind: "video_in", video: { width: dv.getUint16(0, false), height: dv.getUint16(2, false), i420: body.subarray(4) } };
    }
    default:
      return { kind: "unknown", type, body };
  }
}

/** Narrow an untrusted JSON value to a sidecar event (shape check of the discriminant only). */
export function asSidecarEvent(msg: unknown): SidecarEvent | null {
  if (!msg || typeof msg !== "object") return null;
  const t = (msg as { type?: unknown }).type;
  return typeof t === "string" ? (msg as SidecarEvent) : null;
}
