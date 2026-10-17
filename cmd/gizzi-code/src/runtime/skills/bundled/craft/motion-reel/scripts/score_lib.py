"""score_lib — synthesized score building blocks for motion-reel (numpy/scipy only, no samples, no licensing).
Import, call init(duration), place() instruments at cue times that match the scene timeline, then write_mix()."""
import numpy as np
from scipy import signal
from scipy.io import wavfile

SR = 48000
rng = np.random.default_rng(7)
N = 0
dry = wet = duck = None


def init(duration):
    """Allocate buffers. dry = direct mix, wet = reverb send, duck = sidechain gain (kick pulls bass down)."""
    global N, dry, wet, duck
    N = int(SR * duration)
    dry = np.zeros((N, 2)); wet = np.zeros((N, 2)); duck = np.ones(N)


def place(buf, t0, x, gain=1.0, pan=0.0):
    """Mix mono signal x into buf ('dry' or 'wet' buffer) at t0 seconds, constant-power pan -1..1."""
    if isinstance(buf, str):
        buf = dry if buf == 'dry' else wet
    i = int(t0 * SR)
    if i >= N:
        return
    x = x[: N - i]
    l, r = np.cos((pan + 1) * np.pi / 4), np.sin((pan + 1) * np.pi / 4)
    buf[i:i + len(x), 0] += x * gain * l * 1.414
    buf[i:i + len(x), 1] += x * gain * r * 1.414


def tt(d):
    return np.arange(int(d * SR)) / SR


def bp(x, lo, hi, order=2):
    return signal.sosfilt(signal.butter(order, [lo, hi], 'bandpass', fs=SR, output='sos'), x)


def lp(x, f, order=2):
    return signal.sosfilt(signal.butter(order, f, 'lowpass', fs=SR, output='sos'), x)


def hp(x, f, order=2):
    return signal.sosfilt(signal.butter(order, f, 'highpass', fs=SR, output='sos'), x)


def noise(d):
    return rng.standard_normal(int(d * SR))


# ---------- instruments ----------
def kick(amp=1.0):
    t = tt(0.45)
    f = 45 + 110 * np.exp(-t * 30)
    ph = 2 * np.pi * np.cumsum(f) / SR
    body = np.sin(ph) * np.exp(-t * 7)
    click = hp(noise(0.45), 2000) * np.exp(-t * 300) * 0.3
    return np.tanh((body + click) * 1.6) * amp


def impact(size=1.0):
    t = tt(2.5)
    f = 32 + 90 * np.exp(-t * 9)
    sub = np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-t * 2.4)
    crunch = lp(noise(2.5), 1800) * np.exp(-t * 6) * 0.8
    air = hp(noise(2.5), 5000) * np.exp(-t * 10) * 0.25
    return np.tanh((sub * 1.4 + crunch + air) * 1.3) * size


def clap():
    t = tt(0.35)
    n = bp(noise(0.35), 900, 3500)
    env = np.zeros_like(t)
    for k, o in enumerate([0, 0.011, 0.022]):
        env += (t >= o) * np.exp(-(t - o).clip(0) * (180 if k < 2 else 16))
    return n * env * 0.5


def hat(open_=False):
    d = 0.25 if open_ else 0.05
    t = tt(d)
    return hp(noise(d), 7500, 4) * np.exp(-t * (14 if open_ else 90)) * 0.35


def blip(freq, d=0.09, amp=0.25):
    t = tt(d)
    return (np.sin(2 * np.pi * freq * t) + 0.3 * np.sin(4 * np.pi * freq * t)) * np.exp(-t * 45) * amp


def tick():
    t = tt(0.012)
    return hp(noise(0.012), 3000) * np.exp(-t * 600) * 0.18


def riser(d, f0=300, f1=5000, amp=0.5):
    t = tt(d)
    n = noise(d)
    out = np.zeros_like(n)
    seg = 1024
    for i in range(0, len(n), seg):
        p = i / len(n)
        fc = f0 * (f1 / f0) ** p
        out[i:i + seg] = bp(n[max(0, i - 4096):i + seg], fc * 0.7, min(fc * 1.4, SR / 2 - 100))[-len(n[i:i + seg]):]
    tone = signal.sawtooth(2 * np.pi * np.cumsum(120 * (8 ** (t / d))) / SR) * 0.15
    env = (t / d) ** 2.2
    return (out * 0.8 + lp(tone, 3000)) * env * amp


def whoosh(d=0.45, amp=0.5):
    t = tt(d)
    n = noise(d)
    c = d * 0.6
    env = np.where(t < c, (t / c) ** 2, np.exp(-(t - c) * 14))
    return bp(n, 400, 4000) * env * amp


def saw_note(freq, d, cutoff=500, amp=0.3):
    t = tt(d)
    x = sum(signal.sawtooth(2 * np.pi * freq * (1 + det) * t) for det in (-0.006, 0, 0.007)) / 3
    return lp(x, cutoff, 2) * amp


def pad(freqs, d, cutoff=1400, amp=0.22):
    t = tt(d)
    x = sum(saw_note(f, d, cutoff, 1) for f in freqs) / len(freqs)
    env = np.minimum(1, t / 0.08) * np.exp(-t * 0.7)
    return x * env * amp


def glitch(d, amp=0.35):
    t = tt(d)
    sq = signal.square(2 * np.pi * (800 + 3000 * rng.random()) * t)
    gate = (np.floor(t * 60) % 2 == 0).astype(float)
    crushed = np.round(noise(d) * 4) / 4
    return lp(sq * 0.4 + crushed * 0.3, 6000) * gate * np.linspace(0.3, 1, len(t)) * amp




def groove(start, end, bpm=120, roots=None, kick_amp=1.45, bass_amp=0.5, pad_amp=0.07):
    """Four-on-the-floor kick, offbeat hats, clap on 2 and 4, 16th saw bass ducked by the kick.
    roots: {bar_start_time: root_hz}. Bars are 4 beats long."""
    beat = 60 / bpm
    b = start
    while b < end - 1e-6:
        place(dry, b, kick(kick_amp))
        i = int(b * SR); L = int(0.22 * SR)
        duck[i:i + L] = np.minimum(duck[i:i + L], np.linspace(0.25, 1, len(duck[i:i + L])))
        if int(round((b - start) / beat)) % 2 == 1:
            place(dry, b, clap() * 0.7); place(wet, b, clap() * 0.5)
        place(dry, b + beat / 2, hat(open_=(int(round(b / beat)) % 4 == 3)), pan=0.25)
        place(dry, b + beat / 4, hat() * 0.5, pan=-0.3); place(dry, b + 3 * beat / 4, hat() * 0.5, pan=-0.3)
        b += beat
    for bar, root in sorted((roots or {}).items()):
        bar_end = min(bar + 4 * beat, end); s = max(bar, start); k = 0; step = beat / 4
        while s + k * step < bar_end - 1e-6:
            st = s + k * step; f = root * (2 if k % 4 == 2 else 1)
            i = int(st * SR); g = duck[i:i + int(step * SR)].mean() if i < N else 1
            place(dry, st, saw_note(f, step * 0.96, 420 + 300 * (k % 2), bass_amp) * g); k += 1
        place(dry, bar, pad([root * 4, root * 4 * 1.2, root * 6], 4 * beat, 1200, pad_amp))


def typing(a, b, n):
    """n keyboard ticks spread across [a, b] — sync to typed() text reveals."""
    for k in range(n):
        place(dry, a + (b - a) * k / n, tick(), pan=rng.uniform(-0.3, 0.3))


def write_mix(path='score.wav', reverb=0.08, fade=0.25):
    """Convolution reverb on the wet bus, soft-clip, normalize to -1 dBFS, fade tail, write 16-bit WAV."""
    ir_t = tt(1.8)
    ir = np.stack([lp(rng.standard_normal(len(ir_t)) * np.exp(-ir_t * 3.2), 6000) for _ in range(2)], 1)
    rev = np.stack([signal.fftconvolve(wet[:, c], ir[:, c])[:N] for c in range(2)], 1) * reverb
    mix = np.tanh((dry + rev) * 0.9)
    mix /= np.max(np.abs(mix)) / 0.89
    fl = int(fade * SR); env = np.ones(N); env[-fl:] = np.linspace(1, 0, fl) ** 2
    mix *= env[:, None]
    wavfile.write(path, SR, (mix * 32767).astype(np.int16))
    print('wrote', path, f'{N / SR:.2f}s')
