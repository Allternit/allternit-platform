#!/usr/bin/env python3
"""synth_bed.py [seconds] [--style=bouncy|calm|tense] - a free, synthesized music bed at the film's tempo (bpm from
../anim/src/config.js), written to bed.wav for build_mix.py. A placeholder or a deliberate lo-fi score: kick, hats, a bass
line and a soft pad on a I-V-vi-IV loop. Swap in a real track any time (bed.mp3 or bed.wav; build_mix.py takes either).
Needs numpy + ffmpeg."""
import os, re, subprocess, sys, numpy as np
HERE = os.path.dirname(os.path.abspath(__file__)); SR = 48000; TAU = 2 * np.pi
cfg = open(os.path.join(HERE, '..', 'anim', 'src', 'config.js')).read()
BPM = float(re.search(r'bpm:\s*([\d.]+)', cfg).group(1)); DUR = float(re.search(r'duration:\s*([\d.]+)', cfg).group(1))
args = [a for a in sys.argv[1:] if not a.startswith('--')]; dur = float(args[0]) if args else DUR + 2
style = next((a.split('=', 1)[1] for a in sys.argv[1:] if a.startswith('--style=')), 'bouncy')
beat = 60 / BPM; n = int(dur * SR); out = np.zeros(n, np.float32); rng = np.random.default_rng(3)
def put(sig, t, g=1.0):
    i = int(t * SR); k = min(len(sig), n - i)
    if k > 0: out[i:i + k] += sig[:k].astype(np.float32) * g
tt = lambda d: np.arange(int(d * SR)) / SR
def kick(): t = tt(.35); return np.sin(TAU * np.cumsum(50 + 90 * np.exp(-t * 30)) / SR) * np.exp(-t * 9)
def hat(): t = tt(.06); x = rng.standard_normal(len(t)); x = np.diff(x, prepend=0); return x * np.exp(-t * 70) * .25
def tone(f, d, k=3.0, harm=(1, .4, .2)): t = tt(d); return sum(a * np.sin(TAU * f * (i + 1) * t) for i, a in enumerate(harm)) * np.exp(-t * k) * np.minimum(1, t * 200)
def pad(fs, d): t = tt(d); env = np.minimum(1, t / .4) * np.minimum(1, (d - t) / .4); return sum(np.sin(TAU * f * t + np.sin(TAU * .3 * t)) for f in fs) / len(fs) * env
key = 261.63 if style != 'tense' else 220.0
prog = [[0, 4, 7], [7, 11, 14], [9, 12, 16], [5, 9, 12]] if style != 'tense' else [[0, 3, 7], [8, 12, 15], [5, 8, 12], [7, 11, 14]]
hz = lambda semi: key * 2 ** (semi / 12)
bars = int(dur / (4 * beat)) + 1
for b in range(bars):
    ch = prog[b % 4]; t0 = b * 4 * beat
    put(pad([hz(s) for s in ch], 4 * beat), t0, .10 if style != 'calm' else .14)
    for q in range(4):
        tb = t0 + q * beat
        if style != 'calm': put(kick(), tb, .55 if q in (0, 2) else .35)
        put(tone(hz(ch[0] - 24 + (7 if q == 3 else 0)), beat * .9, 4), tb, .22)
        if style == 'bouncy': put(hat(), tb + beat / 2, 1.0); put(hat(), tb, .5)
        if style == 'calm' and q % 2 == 0: put(tone(hz(ch[(q // 2) % 3] + 12), beat * 1.8, 2, (1, .1)), tb, .08)
out = out / max(1e-6, np.max(np.abs(out))) * .8
fade = int(.8 * SR); out[-fade:] *= np.linspace(1, 0, fade)
dst = os.path.join(HERE, 'bed.wav')
subprocess.run(['ffmpeg', '-v', 'error', '-y', '-f', 'f32le', '-ar', str(SR), '-ac', '1', '-i', '-', '-ac', '2', dst], input=out.astype(np.float32).tobytes(), check=True)
print('wrote', dst, f'{dur:.1f}s at {BPM:g} bpm ({style})')
