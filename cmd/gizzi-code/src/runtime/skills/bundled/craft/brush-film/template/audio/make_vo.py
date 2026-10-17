#!/usr/bin/env python3
"""make_vo.py [vo-lines.json] [--engine=NAME] - record every VO line to vo/<id>.wav (48 kHz mono) and print DUR_VO for
anim/src/film.js. Lines already recorded are skipped (delete vo/<id>.wav to redo one).

vo-lines.json = [[id, voice, text, {options}?], ...]   e.g. ["v01", "Samantha", "Does your code only work on Fridays?"]
  id      v01, v02, ... (film.js VO cues refer to it)
  voice   whatever the engine calls a voice (macOS `say -v '?'`, a piper .onnx model path, an espeak voice, an
          ElevenLabs voice_id, or anything your TTS_CMD understands)
  text    what is SPOKEN. Spell names phonetically here if your TTS mispronounces them; the on-screen subtitle lives in
          film.js, so captions keep the real spelling.
  options optional per-line settings passed to the engine (rate, stability, style, ...)

Engines (--engine=, or $TTS_ENGINE; default: the first available of say, piper, espeak-ng):
  say         macOS built-in voices. Free, offline.
  piper       Piper neural TTS (`piper` on PATH; voice = path to a .onnx model). Free, offline.
  espeak-ng   espeak-ng / espeak. Free, offline, robotic (fine for a robot character).
  elevenlabs  tts_elevenlabs.py with $ELEVENLABS_API_KEY. METERED: it spends your ElevenLabs characters.
  cmd         any TTS you have: $TTS_CMD with {voice} {out} {text_file} placeholders, e.g.
              TTS_CMD='mytts --voice {voice} --in {text_file} --out {out}'
  manual      record or generate the lines however you like and drop them in vo/<id>.wav (or .mp3); this just measures.
Never clone a real person's voice without their documented permission."""
import json, os, shlex, shutil, subprocess, sys, tempfile
HERE = os.path.dirname(os.path.abspath(__file__))
args = [a for a in sys.argv[1:] if not a.startswith('--')]
opts = dict(a[2:].split('=', 1) for a in sys.argv[1:] if a.startswith('--') and '=' in a)
lines = json.load(open(args[0] if args else os.path.join(HERE, 'vo-lines.json')))
engine = opts.get('engine') or os.environ.get('TTS_ENGINE') or next((e for e in ['say', 'piper', 'espeak-ng', 'espeak'] if shutil.which(e)), 'manual')
os.makedirs(os.path.join(HERE, 'vo'), exist_ok=True)

def to_wav(src, out):
    subprocess.run(['ffmpeg', '-v', 'error', '-y', '-i', src, '-ar', '48000', '-ac', '1', out], check=True)

def record(k, voice, text, o, out):
    tmp = tempfile.mkdtemp()
    tf = os.path.join(tmp, 'line.txt'); open(tf, 'w').write(text)
    if engine == 'say':
        raw = os.path.join(tmp, 'a.aiff'); cmd = ['say', '-o', raw, '-f', tf] + (['-v', voice] if voice else []) + (['-r', str(o['rate'])] if 'rate' in o else [])
        subprocess.run(cmd, check=True)
    elif engine == 'piper':
        raw = os.path.join(tmp, 'a.wav'); subprocess.run(['piper', '--model', voice, '--output_file', raw], input=text, text=True, check=True)
    elif engine in ('espeak-ng', 'espeak'):
        raw = os.path.join(tmp, 'a.wav'); subprocess.run([engine, '-w', raw, '-f', tf] + (['-v', voice] if voice else []) + (['-s', str(o['rate'])] if 'rate' in o else []), check=True)
    elif engine == 'elevenlabs':
        raw = os.path.join(tmp, 'a.mp3'); subprocess.run([sys.executable, os.path.join(HERE, 'tts_elevenlabs.py'), raw, voice, text, json.dumps(o)], check=True)
    elif engine == 'cmd':
        if not os.environ.get('TTS_CMD'): sys.exit('engine=cmd needs $TTS_CMD (placeholders {voice} {out} {text_file})')
        raw = os.path.join(tmp, 'a.wav')
        subprocess.run(os.environ['TTS_CMD'].format(voice=shlex.quote(voice), out=shlex.quote(raw), text_file=shlex.quote(tf)), shell=True, check=True)
    else:
        return False
    to_wav(raw, out); shutil.rmtree(tmp, ignore_errors=True); return True

dur, missing = {}, []
for k, voice, text, *rest in lines:
    o = rest[0] if rest else {}; out = os.path.join(HERE, 'vo', f'{k}.wav'); mp3 = out[:-4] + '.mp3'
    if not os.path.exists(out) and os.path.exists(mp3): to_wav(mp3, out)
    if not os.path.exists(out):
        try: print(k, 'recorded' if record(k, voice, text, o, out) else 'missing (manual)')
        except subprocess.CalledProcessError as e: print(k, 'FAILED:', e)
    if os.path.exists(out):
        dur[k] = round(float(subprocess.run(['ffprobe', '-v', 'error', '-show_entries', 'format=duration', '-of', 'csv=p=0', out], capture_output=True, text=True).stdout), 2)
    else: missing.append(k)
print(f'engine: {engine}')
print('const DUR_VO = ' + json.dumps(dur).replace('"', '') + ';'); print('total VO', round(sum(dur.values()), 1), 's')
if missing: print('missing:', ' '.join(missing))
