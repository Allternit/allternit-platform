// Fake Zoom sidecar for the protocol test. Speaks the real framed protocol on
// fd 3 (same as src/adapters/zoom/sidecar/src/main.cpp) and plays one scenario chosen by
// FAKE_SCENARIO. It checks the join command it receives, so the test proves
// what the adapter really sends (meeting number and passcode from the link,
// OBF token, SDK JWT). Frames go out in deliberately awkward chunks: one
// byte at a time, and several frames in one write.

import { Socket } from "node:net";
import { FrameDecoder, encodeAudioOut, encodeJson, type SidecarCommand } from "../../src/adapters/zoom/protocol.js";

const sock = new Socket({ fd: 3, readable: true, writable: true });
const scenario = process.env.FAKE_SCENARIO ?? "admit";
const expectNumber = process.env.FAKE_EXPECT_NUMBER;
const expectPasscode = process.env.FAKE_EXPECT_PASSCODE;
const decoder = new FrameDecoder();
let micBytes = 0;
let joinCount = 0;
/** owner_late: how many join attempts Zoom refuses because the authorising user is absent. */
const ownerAbsentTimes = Number(process.env.FAKE_OWNER_ABSENT_TIMES ?? "2");

const send = (msg: object) => sock.write(encodeJson(msg));
const later = (ms: number, fn: () => void) => setTimeout(fn, ms);

/** Write one frame a byte at a time, to exercise the adapter's reassembly. */
async function sendTrickled(msg: object): Promise<void> {
  const bytes = encodeJson(msg);
  for (let i = 0; i < bytes.length; i++) {
    sock.write(bytes.subarray(i, i + 1));
    if (i % 7 === 0) await new Promise((r) => setImmediate(r));
  }
}

const SELF = { id: "16778240", name: "Gizzi (Allternit)", self: true };
const JORDAN = { id: "16779264", name: "Jordan", self: false, host: true };

function goLive(): void {
  // Coalesce three frames into one write.
  const a = encodeJson({ type: "status", status: "in_meeting", code: 0 });
  const b = encodeJson({ type: "participants", list: [SELF, JORDAN] });
  const c = encodeJson({ type: "active_speaker", ids: [JORDAN.id] });
  const all = new Uint8Array(a.length + b.length + c.length);
  all.set(a, 0);
  all.set(b, a.length);
  all.set(c, a.length + b.length);
  sock.write(all);
  if (scenario === "kicked") {
    later(80, () => send({ type: "status", status: "ended", code: 1, name: "EndMeetingReason_KickByHost" }));
    return;
  }
  // TileCamera::onStartSend in the real sidecar.
  later(20, () => send({ type: "log", level: "info", msg: "camera started: tile frames are sent" }));
  later(30, () => {
    void sendTrickled({ type: "chat", msgId: "m-1", senderId: JORDAN.id, sender: "Jordan", text: "Gizzi, what's the plan? ✓", ts: 1763020800, self: false }).then(() => {
      send({ type: "chat", msgId: "m-0", senderId: SELF.id, sender: SELF.name, text: "notice", ts: 1763020801, self: true });
      // 10 ms of 32 kHz mono s16le: 320 samples, a ramp so the test can check the bytes.
      const pcm = new Uint8Array(640);
      for (let i = 0; i < pcm.length; i++) pcm[i] = i & 0xff;
      sock.write(encodeAudioOut({ sampleRate: 32000, channels: 1, pcm }));
    });
  });
}

function onCommand(cmd: SidecarCommand): void {
  switch (cmd.op) {
    case "join": {
      const problems: string[] = [];
      if (expectNumber !== undefined && cmd.meetingNumber !== expectNumber) problems.push(`meetingNumber=${cmd.meetingNumber}`);
      if (expectPasscode !== undefined && cmd.passcode !== expectPasscode) problems.push(`passcode=${cmd.passcode}`);
      if (cmd.obfToken !== "obf-test-token") problems.push("obfToken");
      if (cmd.sdkJwt.split(".").length !== 3) problems.push("sdkJwt");
      if (cmd.displayName !== "Gizzi (Allternit)") problems.push("displayName");
      if (problems.length) {
        send({ type: "result", id: cmd.id, ok: false, error: `bad join: ${problems.join(", ")}` });
        return;
      }
      joinCount++;
      send({ type: "log", level: "info", msg: `join attempt ${joinCount}` });
      send({ type: "result", id: cmd.id, ok: true });
      send({ type: "status", status: "connecting", code: 0 });
      const ownerAbsent = { type: "status", status: "failed", code: 63, name: "MEETING_FAIL_AUTHORIZED_USER_NOT_INMEETING" };
      if (scenario === "owner_never" || (scenario === "owner_late" && joinCount <= ownerAbsentTimes)) {
        later(10, () => send(ownerAbsent));
        return;
      }
      if (scenario === "owner_late") {
        later(10, goLive);
        return;
      }
      if (scenario === "obf_conflict") {
        later(10, () => send({ type: "status", status: "failed", code: 64, name: "MEETING_FAIL_ON_BEHALF_TOKEN_CONFLICT_LOGIN_ERROR" }));
        return;
      }
      if (scenario === "garbage") {
        // Length 0 is never valid: the adapter must drop the stream.
        later(10, () => sock.write(new Uint8Array([0, 0, 0, 0, 1])));
        return;
      }
      if (scenario === "open") {
        later(10, goLive);
        return;
      }
      later(10, () => send({ type: "status", status: "in_waiting_room", code: 0 }));
      if (scenario === "decline") later(60, () => send({ type: "status", status: "ended", code: 1, name: "EndMeetingReason_KickByHost" }));
      if (scenario === "admit" || scenario === "kicked") later(60, goLive);
      // "lobby": stay in the waiting room until the adapter leaves.
      return;
    }
    case "chat_send":
      send({ type: "result", id: cmd.id, ok: true });
      send({ type: "chat", msgId: `out-${cmd.id}`, senderId: SELF.id, sender: SELF.name, text: cmd.text, ts: 1763020900, self: true });
      return;
    case "participants":
      send({ type: "result", id: cmd.id, ok: true, data: { list: [SELF, JORDAN, { id: "16780288", name: "Sam", self: false }] } });
      return;
    case "tile_state":
    case "audio_clear":
      send({ type: "result", id: cmd.id, ok: true, data: { micBytes } });
      return;
    case "leave":
      send({ type: "result", id: cmd.id, ok: true });
      later(5, () => send({ type: "status", status: "ended", code: 0, name: "EndMeetingReason_None" }));
      return;
    case "shutdown":
      send({ type: "result", id: cmd.id, ok: true });
      later(5, () => process.exit(0));
      return;
  }
}

sock.on("data", (chunk: Buffer) => {
  for (const f of decoder.push(chunk)) {
    if (f.kind === "json") onCommand(f.msg as SidecarCommand);
    else if (f.kind === "audio_in") {
      // Loop mic audio back as call audio so the test sees both directions decode.
      micBytes += f.audio.pcm.length;
      sock.write(encodeAudioOut(f.audio));
    } else if (f.kind === "video_in") {
      // Echo what arrived (size + first Y/U/V bytes) so the test sees the frame decode.
      const v = f.video;
      const ySize = v.width * v.height;
      send({ type: "log", level: "info", msg: `video_in ${v.width}x${v.height} len=${v.i420.length} yuv=${v.i420[0]},${v.i420[ySize]},${v.i420[ySize + ySize / 4]}` });
    }
  }
});
sock.on("close", () => process.exit(0));

// SDK and Qt chatter goes to stdout in the real sidecar; the adapter must not mistake it for protocol.
process.stdout.write("Qt: fake log line on stdout\n");
send({ type: "ready", sdkVersion: "fake-6.6.10" });
