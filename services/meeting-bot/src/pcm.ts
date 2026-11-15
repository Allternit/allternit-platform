// PCM helpers for the call-audio bridge (livekit-node.ts): Zoom's 32 kHz mix
// and any page or sidecar rate become the 48 kHz mono LiveKit source.

/** s16 interleaved stereo to mono (average). Mono input is returned as is. */
export function downmixS16(pcm: Int16Array, channels: number): Int16Array {
  if (channels <= 1) return pcm;
  const n = Math.floor(pcm.length / channels);
  const out = new Int16Array(n);
  for (let i = 0; i < n; i++) {
    let s = 0;
    for (let c = 0; c < channels; c++) s += pcm[i * channels + c];
    out[i] = Math.round(s / channels);
  }
  return out;
}

/**
 * Streaming linear-interpolation resampler for mono s16 PCM (Zoom's 32 kHz
 * mix to the 48 kHz LiveKit source). Keeps its phase and the previous chunk's
 * last sample across calls, so chunk boundaries add no clicks or drift.
 */
export class PcmResampler {
  private readonly step: number;
  private last = 0;
  /** Input position of the next output sample, relative to the next chunk's first sample (>= -1). */
  private pos = 0;
  constructor(
    readonly from: number,
    readonly to: number,
  ) {
    if (!(from > 0) || !(to > 0)) throw new Error("bad sample rate");
    this.step = from / to;
  }

  process(input: Int16Array): Int16Array {
    const n = input.length;
    if (n === 0) return new Int16Array(0);
    if (this.from === this.to) return input.slice();
    const out = new Int16Array(Math.ceil((n - this.pos) / this.step) + 2);
    let k = 0;
    let p = this.pos;
    while (p < n - 1) {
      const i = Math.floor(p);
      const f = p - i;
      const a = i < 0 ? this.last : input[i];
      const b = input[i + 1];
      out[k++] = Math.round(a + (b - a) * f);
      p += this.step;
    }
    this.last = input[n - 1];
    this.pos = p - n;
    return out.subarray(0, k);
  }
}
