// Zoom media path: the pure pieces the tile pump and the audio bridge rely on.

import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { i420ToRgba, rgbaToI420 } from "../../src/zoom-media.js";
import { PcmResampler, downmixS16 } from "../../src/pcm.js";
import { parseWorkerData } from "../../src/livekit-node.js";

function solid(w: number, h: number, r: number, g: number, b: number): Uint8Array {
  const px = new Uint8Array(w * h * 4);
  for (let i = 0; i < w * h; i++) px.set([r, g, b, 255], i * 4);
  return px;
}

/** Y, U, V of the first pixel / chroma sample. */
function yuv(i420: Uint8Array, w: number, h: number): [number, number, number] {
  return [i420[0], i420[w * h], i420[w * h + (w * h) / 4]];
}

describe("rgbaToI420 (BT.601 limited range, what the sidecar's FrameDataFormat_I420_LIMITED expects)", () => {
  it("maps known colours to the standard limited-range values", () => {
    const w = 4;
    const h = 2;
    // Reference values: ITU-R BT.601, 8-bit studio swing.
    assert.deepEqual(yuv(rgbaToI420(solid(w, h, 0, 0, 0), w, h), w, h), [16, 128, 128]);
    assert.deepEqual(yuv(rgbaToI420(solid(w, h, 255, 255, 255), w, h), w, h), [235, 128, 128]);
    assert.deepEqual(yuv(rgbaToI420(solid(w, h, 255, 0, 0), w, h), w, h), [81, 90, 240]);
    assert.deepEqual(yuv(rgbaToI420(solid(w, h, 0, 255, 0), w, h), w, h), [145, 54, 34]);
    assert.deepEqual(yuv(rgbaToI420(solid(w, h, 0, 0, 255), w, h), w, h), [41, 240, 110]);
  });

  it("lays out Y, then U, then V, with chroma averaged per 2x2 block", () => {
    const w = 4;
    const h = 2;
    // Left 2x2 block white, right 2x2 block black.
    const px = new Uint8Array(w * h * 4);
    for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) px.set(x < 2 ? [255, 255, 255, 255] : [0, 0, 0, 255], (y * w + x) * 4);
    const out = rgbaToI420(px, w, h);
    assert.equal(out.length, w * h * 1.5);
    assert.deepEqual([...out.subarray(0, 8)], [235, 235, 16, 16, 235, 235, 16, 16]);
    assert.deepEqual([...out.subarray(8)], [128, 128, 128, 128]);
  });

  it("writes into a reused buffer and round-trips through i420ToRgba within rounding", () => {
    const w = 8;
    const h = 4;
    const buf = new Uint8Array(w * h * 1.5);
    const px = solid(w, h, 200, 120, 40);
    assert.equal(rgbaToI420(px, w, h, buf), buf);
    const back = i420ToRgba(buf, w, h);
    for (const [i, want] of [200, 120, 40].entries()) assert.ok(Math.abs(back[i] - want) <= 3, `channel ${i}: ${back[i]} vs ${want}`);
  });

  it("is self-contained, so its source can run in the converter page", () => {
    const again = new Function(`return (${rgbaToI420.toString()})`)() as typeof rgbaToI420;
    assert.deepEqual(yuv(again(solid(2, 2, 255, 0, 0), 2, 2), 2, 2), [81, 90, 240]);
  });
});

describe("PcmResampler (Zoom 32 kHz mix to the 48 kHz call-audio source)", () => {
  const sine = (n: number, rate: number, hz: number, from = 0) =>
    Int16Array.from({ length: n }, (_, i) => Math.round(10000 * Math.sin((2 * Math.PI * hz * (i + from)) / rate)));

  it("makes 1.5x the samples and keeps a constant level", () => {
    const r = new PcmResampler(32000, 48000);
    const out = r.process(new Int16Array(3200).fill(1234));
    assert.ok(Math.abs(out.length - 4800) <= 2, String(out.length));
    assert.ok(out.every((v) => v === 1234));
  });

  it("gives the same stream whether fed in 10 ms chunks or at once (no seams)", () => {
    const input = sine(32000, 32000, 440);
    const whole = new PcmResampler(32000, 48000).process(input);
    const r = new PcmResampler(32000, 48000);
    const parts: number[] = [];
    for (let i = 0; i < input.length; i += 320) parts.push(...r.process(input.subarray(i, i + 320)));
    assert.ok(Math.abs(parts.length - whole.length) <= 1, `${parts.length} vs ${whole.length}`);
    for (let i = 0; i < Math.min(parts.length, whole.length); i++) assert.ok(Math.abs(parts[i] - whole[i]) <= 1, `sample ${i}`);
    // 1 s in, 1 s out, within one sample.
    assert.ok(Math.abs(parts.length - 48000) <= 2, String(parts.length));
  });

  it("keeps the pitch: a 440 Hz tone at 32 kHz is a 440 Hz tone at 48 kHz", () => {
    const out = new PcmResampler(32000, 48000).process(sine(32000, 32000, 440));
    const want = sine(out.length, 48000, 440);
    let err = 0;
    for (let i = 0; i < out.length; i++) err = Math.max(err, Math.abs(out[i] - want[i]));
    assert.ok(err < 150, `max error ${err} of 10000`);
  });

  it("downsamples too (48 kHz TTS to 16 kHz)", () => {
    const out = new PcmResampler(48000, 16000).process(new Int16Array(4800).fill(-500));
    assert.ok(Math.abs(out.length - 1600) <= 1);
    assert.ok(out.every((v) => v === -500));
  });

  it("downmixes stereo by averaging", () => {
    assert.deepEqual([...downmixS16(Int16Array.from([100, 300, -200, 0]), 2)], [200, -100]);
  });
});

describe("parseWorkerData (worker -> pod topics, same rules as the in-page bridge)", () => {
  const enc = (v: unknown) => new TextEncoder().encode(JSON.stringify(v));

  it("keeps well-formed STT segments, single or batched, and drops malformed rows", () => {
    assert.deepEqual(parseWorkerData("allternit.meeting.stt", enc({ start_ms: 0, end_ms: 900, text: "hi", start_at: 5 }))?.data, [
      { start_ms: 0, end_ms: 900, text: "hi", start_at: 5 },
    ]);
    const batch = parseWorkerData("allternit.meeting.stt", enc({ segments: [{ start_ms: 1, end_ms: 2, text: "a" }, { start_ms: 1, text: "no end" }] }));
    assert.deepEqual(batch?.data.map((d) => d.text), ["a"]);
  });

  it("reads wake-word turns and ignores other topics", () => {
    const t = parseWorkerData("allternit.meeting.turn", enc({ turn_id: "wake-1", text: "what's next?", confidence: 0.8, start_ms: 10 }));
    assert.equal(t?.turn?.turn_id, "wake-1");
    assert.equal(parseWorkerData("allternit.meeting.turn", enc({ text: "no id" }))?.turn, null);
    assert.equal(parseWorkerData("something.else", enc({})), null);
    assert.throws(() => parseWorkerData("allternit.meeting.stt", new TextEncoder().encode("{")));
  });
});
