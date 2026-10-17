#!/usr/bin/env python3
"""build_mix.py - the film's full soundtrack, in video time (before any intro/outro is added at assembly).
VO cue times are read from ../anim/src/film.js (one source of truth) and the lines from vo/<id>.wav (make_vo.py).
Music: bed.mp3 or bed.wav (your track, or synth_bed.py), extended by beat-aligned loops if it is too short, ducked under the
VO, with a tape-stop at MUSIC.bedEnd; an optional end.mp3/end.wav cue at MUSIC.piano. No bed = no music, which is fine.
Sound effects are synthesized here (numpy), so they cost nothing and can be re-timed freely: edit SFX below.
Output: ../anim/assets/mix.wav (48 kHz stereo). Needs numpy, scipy and ffmpeg."""
import os, re, subprocess, numpy as np
from scipy.signal import butter, sosfilt
HERE = os.path.dirname(os.path.abspath(__file__)); SR = 48000
def load(p, sr=SR):
    raw = subprocess.run(['ffmpeg', '-v', 'error', '-i', p, '-ac', '1', '-ar', str(sr), '-f', 'f32le', '-'], capture_output=True, check=True).stdout
    return np.frombuffer(raw, np.float32).copy()
film = '\n'.join(l for l in open(os.path.join(HERE, '..', 'anim', 'src', 'film.js')).read().splitlines() if not l.strip().startswith('//'))
VO = [(float(a), k) for a, k in re.findall(r"\[\s*([\d.]+),\s*'(v\w+)'", film)]
MUS = dict(re.findall(r"(bedEnd|piano):\s*([\d.]+)", film)); BED_END, PIANO = float(MUS['bedEnd']), float(MUS.get('piano', 0))
DUR = float(re.search(r'duration:\s*([\d.]+)', open(os.path.join(HERE, '..', 'anim', 'src', 'config.js')).read()).group(1))
BPM = float(re.search(r'bpm:\s*([\d.]+)', open(os.path.join(HERE, '..', 'anim', 'src', 'config.js')).read()).group(1))
BED_END = BED_END if 0 < BED_END < DUR else DUR
out = np.zeros(int(DUR * SR) + SR, np.float32)
def put(sig, t, g=1.0):
    i = int(t * SR); n = min(len(sig), len(out) - i)
    if n > 0: out[i:i + n] += sig[:n] * g
t_ = lambda d: np.arange(int(d * SR)) / SR
def bp(x, lo, hi): return sosfilt(butter(2, [lo, hi], 'band', fs=SR, output='sos'), x)
def lp(x, f): return sosfilt(butter(2, f, 'low', fs=SR, output='sos'), x)
rng = np.random.default_rng(7); noise = lambda d: rng.standard_normal(int(d * SR)).astype(np.float32)

# ---------- synthesized SFX ----------
def click():  t = t_(.12); return (bp(noise(.12), 1500, 6000) * np.exp(-t * 60) * .6 + np.sin(TAU * 110 * t) * np.exp(-t * 40) * .5)
TAU = 2 * np.pi
def slap():   t = t_(.25); return bp(noise(.25), 600, 4000) * np.exp(-t * 28) * .9 + np.sin(TAU * 180 * t) * np.exp(-t * 30) * .4
def stamp(g=1): t = t_(.45); return (np.sin(TAU * (70 + 40 * np.exp(-t * 20)) * t) * np.exp(-t * 9) * .9 + bp(noise(.45), 300, 2500) * np.exp(-t * 35) * .5) * g
def whoosh(d=.6, up=True):
    t = t_(d); env = np.sin(np.pi * t / d) ** 2; x = noise(d); seg = 8; y = np.zeros_like(x); L = len(x) // seg
    for i in range(seg): f = 400 + 3000 * ((i / seg) if up else 1 - i / seg); y[i * L:(i + 1) * L] = bp(x[i * L:(i + 1) * L], f * .6, f * 1.4)
    return y * env * .5
def sparkle():
    s = np.zeros(int(1.0 * SR), np.float32)
    for i, f in enumerate([1760, 2349, 2794, 3520, 4186]): t = t_(.6); s[int(i * .07 * SR):int(i * .07 * SR) + len(t)] += np.sin(TAU * f * t) * np.exp(-t * 7) * .18
    return s
def boink(f0=300, f1=900, d=.25): t = t_(d); return np.sin(TAU * np.cumsum(np.linspace(f0, f1, len(t))) / SR) * np.exp(-t * 8) * .35
def crash(d=.5): t = t_(d); return bp(noise(d), 200, 7000) * np.exp(-t * 7) * .5
def chaos():
    s = np.zeros(int(1.4 * SR), np.float32)
    for i in range(9):
        o = int(hash01(i) * 1.1 * SR); x = boink(200 + 600 * hash01(i + 3), 900 * hash01(i + 5) + 150) if i % 2 else crash(.3)
        s[o:o + len(x)] += x[:len(s) - o]
    return s
hash01 = lambda i: (np.sin(i * 127.1 + 311.7) * 43758.5453) % 1
def ring(d=2.2):
    t = t_(d); bell = sum(np.sin(TAU * f * t) * a for f, a in [(1180, .5), (1610, .3), (2350, .2), (3020, .1)])
    trem = (np.sin(TAU * 22 * t) > 0).astype(np.float32); gate = ((t % 1.0) < .6).astype(np.float32)
    return bell * trem * gate * .25
def clunk():
    t = t_(2.0); thump = np.sin(TAU * (55 + 30 * np.exp(-t * 15)) * t) * np.exp(-t * 6) * 1.0 + bp(noise(2.0), 80, 1200) * np.exp(-t * 18) * .6
    hum = np.sin(TAU * np.cumsum(60 * np.exp(-t * 1.6)) / SR) * np.exp(-t * 2.2) * .25 * (t > .05)
    return thump + hum
def chalk(d=1.2): t = t_(d); return bp(noise(d), 2500, 7000) * (0.4 + 0.6 * (np.sin(TAU * 9 * t) > .2)) * .18
def pour(d=1.0): t = t_(d); return bp(noise(d), 300, 1500) * (.5 + .5 * np.sin(TAU * 13 * t) ** 2) * .22 * np.minimum(1, t * 8) * np.minimum(1, (d - t) * 6)
def creak(d=.9): t = t_(d); f = 220 + 80 * np.sin(TAU * 1.3 * t); return np.sign(np.sin(TAU * np.cumsum(f) / SR)) * bp(noise(d), 200, 900) * .06 + np.sin(TAU * np.cumsum(f) / SR) * .08 * np.sin(np.pi * t / d)
def roomtone(d): return lp(noise(d), 180) * .05

SFX = [   # (time, sound, gain): EDIT PER FILM. Time each hit to the frame the chapter animates it on (ask chapter builders
          # to report their hit times). Library above: click slap stamp whoosh sparkle boink crash chaos ring clunk chalk pour creak.
    # These are the hits of the bundled example ("The Balloon"); replace them with your film's.
    (2.05, boink(500, 700, .2), .5), (2.55, boink(300, 1100), .8), (3.1, boink(200, 600, .3), .6), (3.5, whoosh(.6), .7),
    (5.25, sparkle(), .7), (5.75, boink(250, 900, .35), .7), (6.55, whoosh(.5, False), .5),
]
for t0, s, g in SFX: put(s.astype(np.float32), t0, g * .55)
# optional room tone for silent stretches: put(roomtone(d) * fade, start, 1.0)

# ---------- music ----------
BED = next((os.path.join(HERE, f) for f in ('bed.mp3', 'bed.wav') if os.path.exists(os.path.join(HERE, f))), None)
bed = load(BED) if BED else np.zeros(SR, np.float32); L = 8 * 60 / BPM; xf = int(.25 * SR); bed2 = bed
while BED and len(bed2) < BED_END * SR + SR:   # too short: splice in beat-aligned 8-beat loops from near the end
    n = len(bed2) / SR
    if n < L + .5: bed2 = np.concatenate([bed2, bed2]); continue
    cut = max(L, n - min(6, n * .25)); a, b = bed2[:int(cut * SR)].copy(), bed2[int((cut - L) * SR):]
    ramp = np.linspace(0, 1, xf, dtype=np.float32); a[-xf:] = a[-xf:] * (1 - ramp) + b[:xf] * ramp; bed2 = np.concatenate([a, b[xf:]])
bed2 = bed2[:int(BED_END * SR)]
if BED_END < DUR - .5:   # a story beat kills the music: tape-stop (1.2 s of bed played slower and slower)
    tail = bed2[int((BED_END - 1.2) * SR):int(BED_END * SR)]
    rate = np.linspace(1, .05, int(.9 * SR)); pos = np.cumsum(rate); pos = pos[pos < len(tail) - 1]
    stop = np.interp(pos, np.arange(len(tail)), tail) * np.linspace(1, 0, len(pos)) ** 1.5
    bedfull = np.concatenate([bed2, stop.astype(np.float32)])
else:                    # the music runs to the end: a short fade
    bedfull = bed2.copy(); f = min(len(bedfull), int(.8 * SR)); bedfull[-f:] *= np.linspace(1, 0, f, dtype=np.float32)
duck = np.ones(len(out), np.float32)                                  # duck the bed under VO
for t0, k in VO:
    d = load(os.path.join(HERE, 'vo', k + '.wav'))
    i0, i1 = int((t0 - .12) * SR), int((t0 + len(d) / SR + .15) * SR); duck[max(0, i0):i1] = np.minimum(duck[max(0, i0):i1], .42)
kern = np.ones(int(.12 * SR), np.float32) / int(.12 * SR); duck = np.convolve(duck, kern, 'same')
mus = np.zeros(len(out), np.float32); mus[:len(bedfull)] = bedfull[:len(out)]
if BED: put(mus * duck * .55, 0)
END = next((os.path.join(HERE, f) for f in ('end.mp3', 'end.wav') if os.path.exists(os.path.join(HERE, f))), None)
if PIANO and END:
    piano = load(END); put(piano * np.minimum(1, np.arange(len(piano)) / SR * 1.5) * .6, PIANO)

# ---------- VO ----------
for t0, k in VO:
    v = load(os.path.join(HERE, 'vo', k + '.wav')); put(v * 1.15, t0)

out = out[:int(DUR * SR)]; peak = np.max(np.abs(out)); out = out / max(peak, 1e-6) * .89
st = np.stack([out, out], 1).astype(np.float32).tobytes()
dst = os.path.join(HERE, '..', 'anim', 'assets', 'mix.wav'); os.makedirs(os.path.dirname(dst), exist_ok=True)
subprocess.run(['ffmpeg', '-v', 'error', '-y', '-f', 'f32le', '-ar', str(SR), '-ac', '2', '-i', '-', '-c:a', 'pcm_s16le', dst], input=st, check=True)
print('wrote', dst, f'{DUR}s', len(VO), 'VO cues', len(SFX), 'sfx')
