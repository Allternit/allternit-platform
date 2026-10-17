#!/usr/bin/env python3
"""tts_elevenlabs.py <out.mp3> <voice_id> "<text>" ['{"stability": .4, "style": .4, "model_id": "eleven_multilingual_v2"}']
ElevenLabs text-to-speech through the public API with YOUR key ($ELEVENLABS_API_KEY). METERED: every call spends
characters on your ElevenLabs plan. Use your own library or stock voices; never clone a real person's voice without their
documented permission. List your voices:  python3 tts_elevenlabs.py --voices"""
import json, os, sys, urllib.request
KEY = os.environ.get('ELEVENLABS_API_KEY') or sys.exit('set ELEVENLABS_API_KEY (this engine is metered: it spends your ElevenLabs characters)')
API = os.environ.get('ELEVENLABS_API_BASE', 'https://api.elevenlabs.io')
def call(path, body=None):
    req = urllib.request.Request(API + path, data=json.dumps(body).encode() if body else None, headers={'xi-api-key': KEY, 'Content-Type': 'application/json'}, method='POST' if body else 'GET')
    with urllib.request.urlopen(req, timeout=120) as r: return r.read()
if sys.argv[1:2] == ['--voices']:
    for v in json.loads(call('/v1/voices'))['voices']: print(v['voice_id'], v['name'], '-', (v.get('labels') or {}).get('description', ''))
    sys.exit()
out, voice, text = sys.argv[1:4]; o = json.loads(sys.argv[4]) if len(sys.argv) > 4 else {}
body = {'text': text, 'model_id': o.get('model_id', 'eleven_multilingual_v2'),
        'voice_settings': {'stability': o.get('stability', .4), 'similarity_boost': o.get('similarity', .75), 'style': o.get('style', .35)}}
open(out, 'wb').write(call(f'/v1/text-to-speech/{voice}?output_format=mp3_44100_128', body)); print('saved', out)
