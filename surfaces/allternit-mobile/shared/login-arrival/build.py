#!/usr/bin/env python3
"""Build the offline illustration; --web copies the exact same bytes into the web repository."""
from pathlib import Path
import argparse,hashlib,base64
root=Path(__file__).resolve().parent
parser=argparse.ArgumentParser();parser.add_argument('--web',type=Path);args=parser.parse_args()
script=(root/'scene.js').read_text()+'\n'+(root/'player.js').read_text()
script_hash=base64.b64encode(hashlib.sha256(script.encode()).digest()).decode()
html='<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta http-equiv="Content-Security-Policy" content="default-src \'none\'; script-src \'sha256-HASH\'; style-src \'unsafe-inline\'; img-src data:; connect-src \'none\'; base-uri \'none\'; form-action \'none\'"><title>Allternit welcome animation</title><style>html,body{margin:0;width:100%;height:100%;overflow:hidden;background:#141413;color:#f0eee6}canvas{display:block;width:100%;height:100%;object-fit:contain}</style></head><body><canvas role="img" aria-label="The Allternit wordmark assembles, Gizzi welcomes the bot team, then the team disappears and the loop repeats." width="720" height="500"></canvas><script>SCRIPT</script></body></html>'
html=html.replace('HASH',script_hash).replace('SCRIPT',script)
output=root/'login-arrival.html';output.write_text(html)
if args.web:
 dest=args.web/'public/brand/login-arrival.html';dest.parent.mkdir(parents=True,exist_ok=True);dest.write_bytes(output.read_bytes())
print('login-arrival.html sha256='+hashlib.sha256(output.read_bytes()).hexdigest())
