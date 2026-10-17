#!/usr/bin/env python3
"""beat.py <music file> [from_s to_s] — estimate tempo (BPM) and first-beat offset for src/config.js (bpm, offset)."""
import subprocess, sys, numpy as np
f = sys.argv[1]; a, b = (float(sys.argv[2]), float(sys.argv[3])) if len(sys.argv) > 3 else (5, 50)
x = np.frombuffer(subprocess.run(['ffmpeg', '-v', 'error', '-i', f, '-ac', '1', '-ar', '11025', '-f', 'f32le', '-'], capture_output=True).stdout, np.float32)
hop = 128; fr = 11025 / hop; e = np.array([np.sum(x[i:i + 512] ** 2) for i in range(0, len(x) - 512, hop)]); on = np.maximum(0, np.diff(np.log(e + 1e-9))); on -= on.mean()
seg = on[int(a * fr):int(b * fr)]; ac = np.correlate(seg, seg, 'full')[len(seg) - 1:]; lags = np.arange(1, len(ac)) / fr; bpms = 60 / lags; m = (bpms > 70) & (bpms < 180)
bpm = bpms[m][np.argmax(ac[1:][m])]; per = 60 / bpm * fr
ph = [sum(on[int(a * fr + k + j * per)] for j in range(int((b - a) * fr / per) - 1)) for k in range(int(per))]
print(f'bpm {bpm:.1f}  offset {((a * fr + np.argmax(ph)) / fr) % (60 / bpm):.2f}')
