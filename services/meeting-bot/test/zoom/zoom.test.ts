// Zoom adapter: link parsing, frame codec, and the adapter driving a fake
// sidecar over a real child process and fd 3. No Zoom call.
// Run (from services/meeting-bot): tsc -p test/zoom/tsconfig.json && node --test dist/test/test/zoom/zoom.test.js

import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { describe, it } from "node:test";
import type { ActiveSpeakerEvent, ChatMessage, JoinPhase, JoinRequest, Participant } from "../../src/adapter.js";
import { ZoomAdapter, parseZoomJoinUrl } from "../../src/adapters/zoom/index.js";
import { FrameDecoder, MAX_FRAME_BYTES, encodeAudioIn, encodeJson, encodeVideoIn } from "../../src/adapters/zoom/protocol.js";

const FAKE = fileURLToPath(new URL("./fake-sidecar.js", import.meta.url));

describe("parseZoomJoinUrl (CONTRACT.md section 7)", () => {
  const ok: Array<[string, { meetingNumber?: string; vanityId?: string; passcode?: string; urlKey: string }]> = [
    ["https://zoom.us/j/81234567890?pwd=AbC123.1", { meetingNumber: "81234567890", passcode: "AbC123.1", urlKey: "zoom:81234567890" }],
    ["https://us02web.zoom.us/j/812345678", { meetingNumber: "812345678", urlKey: "zoom:812345678" }],
    ["https://Company.Zoom.US/j/8123456789#success", { meetingNumber: "8123456789", urlKey: "zoom:8123456789" }],
    ["https://zoom.com/wc/join/81234567890?pwd=x", { meetingNumber: "81234567890", passcode: "x", urlKey: "zoom:81234567890" }],
    ["https://zoom.us/wc/81234567890/join?pwd=y&uname=a", { meetingNumber: "81234567890", passcode: "y", urlKey: "zoom:81234567890" }],
    ["https://zoom.us/my/81234567890", { meetingNumber: "81234567890", urlKey: "zoom:81234567890" }],
    ["https://acme.zoom.us/my/Jordan.Lee?pwd=z", { vanityId: "Jordan.Lee", passcode: "z", urlKey: "zoom:my:jordan.lee" }],
    ["  https://zoom.us/j/81234567890/  ", { meetingNumber: "81234567890", urlKey: "zoom:81234567890" }],
  ];
  for (const [url, want] of ok) {
    it(`accepts ${url.trim()}`, () => {
      const got = parseZoomJoinUrl(url);
      assert.ok(got);
      assert.deepEqual(
        { meetingNumber: got.meetingNumber, vanityId: got.vanityId, passcode: got.passcode, urlKey: got.urlKey },
        { meetingNumber: want.meetingNumber, vanityId: want.vanityId, passcode: want.passcode, urlKey: want.urlKey },
      );
    });
  }
  const bad = [
    "http://zoom.us/j/81234567890", // not https
    "https://zoom.us.evil.com/j/81234567890", // look-alike host
    "https://evilzoom.us/j/81234567890",
    "https://zoom.us/j/12345678", // 8 digits
    "https://zoom.us/j/123456789012", // 12 digits
    "https://zoom.us/s/81234567890", // start link, not a join link
    "https://zoom.us/j/abc",
    "https://user:pw@zoom.us/j/81234567890",
    "https://zoom.us:8443/j/81234567890",
    "https://meet.google.com/abc-defg-hij",
    "not a url",
  ];
  for (const url of bad) it(`rejects ${url}`, () => assert.equal(parseZoomJoinUrl(url), null));
});

describe("FrameDecoder", () => {
  it("reassembles frames split at every byte and splits coalesced frames", () => {
    const a = encodeJson({ type: "chat", text: "héllo ✓" });
    const b = encodeAudioIn({ sampleRate: 16000, channels: 1, pcm: new Uint8Array([1, 2, 3, 4]) });
    const both = new Uint8Array([...a, ...b, ...a]);
    const dec = new FrameDecoder();
    const got = [];
    for (let i = 0; i < both.length; i++) got.push(...dec.push(both.subarray(i, i + 1)));
    assert.equal(got.length, 3);
    assert.deepEqual(got[0], { kind: "json", msg: { type: "chat", text: "héllo ✓" } });
    assert.equal(got[1].kind, "audio_in");
    if (got[1].kind === "audio_in") {
      assert.equal(got[1].audio.sampleRate, 16000);
      assert.deepEqual([...got[1].audio.pcm], [1, 2, 3, 4]);
    }
    assert.equal(dec.pending, 0);

    const dec2 = new FrameDecoder();
    assert.equal(dec2.push(both).length, 3);
  });

  it("keeps a frame body valid after later pushes reuse the buffer", () => {
    const dec = new FrameDecoder();
    const [first] = dec.push(encodeAudioIn({ sampleRate: 8000, channels: 1, pcm: new Uint8Array([9, 9]) }));
    dec.push(encodeAudioIn({ sampleRate: 8000, channels: 1, pcm: new Uint8Array([7, 7]) }));
    assert.ok(first.kind === "audio_in");
    assert.deepEqual([...first.audio.pcm], [9, 9]);
  });

  it("rejects a zero or oversized length as a corrupt stream", () => {
    assert.throws(() => new FrameDecoder().push(new Uint8Array([0, 0, 0, 0])), /bad frame length 0/);
    const big = new Uint8Array(4);
    new DataView(big.buffer).setUint32(0, MAX_FRAME_BYTES + 1);
    assert.throws(() => new FrameDecoder().push(big), /bad frame length/);
  });

  it("refuses PCM that is not whole samples and I420 of the wrong size", () => {
    assert.throws(() => encodeAudioIn({ sampleRate: 16000, channels: 1, pcm: new Uint8Array(3) }), /whole s16/);
    assert.throws(() => encodeVideoIn({ width: 4, height: 2, i420: new Uint8Array(11) }), /expected 12/);
  });
});

// ---- adapter against the fake sidecar ---------------------------------------------

const URL_OK = "https://us05web.zoom.us/j/81234567890?pwd=AbC123.1";

function req(over: Partial<JoinRequest> = {}): JoinRequest {
  return {
    url: URL_OK,
    displayName: "Gizzi (Allternit)",
    identity: { kind: "zoom_obf", obfToken: "obf-test-token", sdkKey: "test-client-id", sdkSecretRef: "env:FAKE_ZOOM_SECRET" },
    admitTimeoutMs: 2000,
    prejoinTimeoutMs: 3000,
    ...over,
  };
}

function adapter(scenario: string, logs: string[] = [], extraEnv: Record<string, string> = {}): ZoomAdapter {
  process.env.FAKE_ZOOM_SECRET = "not-a-real-secret";
  return new ZoomAdapter({
    sidecarPath: process.execPath,
    sidecarArgs: [FAKE],
    sidecarEnv: { FAKE_SCENARIO: scenario, FAKE_EXPECT_NUMBER: "81234567890", FAKE_EXPECT_PASSCODE: "AbC123.1", ...extraEnv },
    activeSpeakerIntervalMs: 20,
    ownerRetryMs: 60,
    log: (level, msg) => logs.push(`${level} ${msg}`),
  });
}

const until = async (cond: () => boolean, ms = 2000) => {
  const end = Date.now() + ms;
  while (!cond()) {
    if (Date.now() > end) throw new Error("timed out waiting");
    await new Promise((r) => setTimeout(r, 10));
  }
};

describe("ZoomAdapter with a fake sidecar", () => {
  it("joins through the waiting room, then chat, roster, speaker and audio flow both ways", async () => {
    const logs: string[] = [];
    const z = adapter("admit", logs);
    const phases: JoinPhase[] = [];
    const chats: ChatMessage[] = [];
    const rosters: Participant[][] = [];
    const speakers: ActiveSpeakerEvent[] = [];
    const audio: Array<{ sampleRate: number; pcm: number[] }> = [];
    const ended: string[] = [];
    z.onChat((m) => chats.push(m));
    z.onParticipants((all) => rosters.push(all));
    z.onActiveSpeaker((e) => speakers.push(e));
    z.onAudio((a) => audio.push({ sampleRate: a.sampleRate, pcm: [...a.pcm] }));
    z.onEnded((r) => ended.push(r));
    try {
      const res = await z.join(req(), (p) => phases.push(p));
      assert.equal(res.ok, true, JSON.stringify(res));
      assert.deepEqual(phases, ["prejoin", "join_clicked", "waiting_admit", "live"]);

      await until(() => chats.length >= 1 && audio.length >= 1 && speakers.some((s) => s.names.length > 0));
      // The bot's own notice is filtered; the trickled frame arrives whole, unicode intact.
      assert.equal(chats.length, 1);
      assert.equal(chats[0].id, "zoom:m-1");
      assert.equal(chats[0].sender, "Jordan");
      assert.equal(chats[0].text, "Gizzi, what's the plan? ✓");
      assert.equal(chats[0].self, false);
      assert.deepEqual(rosters[0], [{ id: "16779264", name: "Jordan" }]);
      assert.deepEqual(speakers.find((s) => s.names.length)!.names, ["Jordan"]);
      assert.equal(audio[0].sampleRate, 32000);
      assert.equal(audio[0].pcm.length, 640);
      assert.equal(audio[0].pcm[255], 255);

      assert.equal(await z.postChat("On it."), true);
      assert.equal(chats.length, 1, "own posted line stays out of onChat");

      // 1.5 s of 16 kHz mono goes out as two frames and loops back intact.
      const pcm = new Uint8Array(16000 * 2 * 1.5);
      for (let i = 0; i < pcm.length; i++) pcm[i] = (i * 7) & 0xff;
      assert.equal(z.sendAudio(pcm, 16000), true);
      await until(() => audio.filter((a) => a.sampleRate === 16000).length === 2);
      const back = audio.filter((a) => a.sampleRate === 16000).flatMap((a) => a.pcm);
      assert.equal(back.length, pcm.length);
      assert.deepEqual(back.slice(31990, 32010), [...pcm.subarray(31990, 32010)]);

      const people = await z.participants();
      assert.deepEqual(people.map((p) => p.name), ["Jordan", "Sam"]);

      assert.ok(logs.some((l) => l.includes("Qt: fake log line on stdout")), "stdout chatter is logged, not parsed");

      // The camera: the sidecar's "camera started" line turns cameraOn on; a tile frame arrives whole.
      await until(() => z.cameraOn);
      const w = 64;
      const h = 36;
      const i420 = new Uint8Array(w * h * 1.5).fill(235, 0, w * h).fill(128, w * h);
      assert.equal(z.sendVideoFrame({ width: w, height: h, i420 }), true);
      await until(() => logs.some((l) => l.includes("video_in 64x36")));
      assert.ok(logs.some((l) => l.includes(`video_in 64x36 len=${w * h * 1.5} yuv=235,128,128`)), logs.join("\n"));
    } finally {
      await z.leave();
      await z.close();
    }
    assert.deepEqual(ended, ["left_by_user"]);
    assert.equal(await z.postChat("after leave"), false);
  });

  it("reports a host decline in the waiting room as not_admitted", async () => {
    const z = adapter("decline");
    const res = await z.join(req());
    await z.close();
    assert.equal(res.ok, false);
    if (!res.ok) assert.equal(res.reason, "not_admitted");
  });

  it("gives up after the admit timeout and leaves the lobby", async () => {
    const z = adapter("lobby");
    const t0 = Date.now();
    const res = await z.join(req({ admitTimeoutMs: 150 }));
    assert.equal(res.ok, false);
    if (!res.ok) assert.equal(res.reason, "not_admitted");
    assert.ok(Date.now() - t0 < 2000);
    await z.close();
  });

  it("retries the OBF join while the person who connected Zoom is absent, then goes live when they join", async () => {
    const logs: string[] = [];
    const z = adapter("owner_late", logs, { FAKE_OWNER_ABSENT_TIMES: "2" });
    const phases: JoinPhase[] = [];
    const res = await z.join(req({ admitTimeoutMs: 3000 }), (p) => phases.push(p));
    const attempts = logs.filter((l) => /join attempt \d/.test(l)).length;
    await z.close();
    assert.equal(res.ok, true, JSON.stringify(res));
    // One join() for the runner; waiting_admit reported once, not per attempt.
    assert.deepEqual(phases, ["prejoin", "join_clicked", "waiting_admit", "live"]);
    assert.equal(attempts, 3);
  });

  it("gives up with not_admitted when the person who connected Zoom never joins", async () => {
    const logs: string[] = [];
    const z = adapter("owner_never", logs);
    const phases: JoinPhase[] = [];
    const t0 = Date.now();
    const res = await z.join(req({ admitTimeoutMs: 400 }), (p) => phases.push(p));
    const elapsed = Date.now() - t0;
    const attempts = logs.filter((l) => /join attempt \d/.test(l)).length;
    await z.close();
    assert.equal(res.ok, false);
    if (!res.ok) {
      assert.equal(res.reason, "not_admitted");
      assert.match(res.detail ?? "", /person who connected Zoom did not join/);
    }
    assert.deepEqual(phases, ["prejoin", "join_clicked", "waiting_admit"]);
    assert.ok(attempts >= 3, `expected several attempts, got ${attempts}`);
    assert.ok(elapsed < 2500, `took ${elapsed} ms`);
  });

  it("does not retry other OBF refusals: a token conflict is guest_blocked at once", async () => {
    const logs: string[] = [];
    const z = adapter("obf_conflict", logs);
    const res = await z.join(req());
    await z.close();
    assert.equal(res.ok, false);
    if (!res.ok) assert.equal(res.reason, "guest_blocked");
    assert.equal(logs.filter((l) => /join attempt \d/.test(l)).length, 1);
  });

  it("fires onEnded(removed) when the host removes the bot", async () => {
    const z = adapter("kicked");
    const ended: string[] = [];
    z.onEnded((r) => ended.push(r));
    const res = await z.join(req());
    assert.equal(res.ok, true);
    await until(() => ended.length > 0);
    await z.close();
    assert.deepEqual(ended, ["removed"]);
  });

  it("drops a corrupt stream instead of hanging", async () => {
    const logs: string[] = [];
    const z = adapter("garbage", logs);
    const res = await z.join(req());
    await z.close();
    assert.equal(res.ok, false);
    if (!res.ok) assert.equal(res.reason, "error");
    assert.ok(logs.some((l) => l.includes("corrupt sidecar stream")));
  });

  it("sends the meeting number and passcode from the link (fake refuses anything else)", async () => {
    const z = adapter("open");
    const res = await z.join(req({ url: "https://zoom.us/j/81234567890?pwd=WRONG" }));
    await z.close();
    assert.equal(res.ok, false);
    if (!res.ok) {
      assert.equal(res.reason, "join_failed");
      assert.match(res.detail ?? "", /passcode=WRONG/);
    }
  });

  it("refuses a non-OBF identity and a non-Zoom link without starting the sidecar", async () => {
    const z1 = adapter("open");
    const r1 = await z1.join(req({ identity: { kind: "anonymous" } }));
    assert.ok(!r1.ok && r1.reason === "join_failed" && /zoom_obf/.test(r1.detail ?? ""));
    const z2 = adapter("open");
    const r2 = await z2.join(req({ url: "https://meet.google.com/abc-defg-hij" }));
    assert.ok(!r2.ok && r2.reason === "join_failed");
  });
});
