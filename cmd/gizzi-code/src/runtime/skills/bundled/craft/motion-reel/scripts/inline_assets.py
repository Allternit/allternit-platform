#!/usr/bin/env python3
"""Turn image files into assets.js (data URIs) so the canvas stays untainted for toDataURL.
usage: inline_assets.py <project_dir> name=path/to/file.svg [name2=path.png ...]
Also: blocks_from_svg=<name>:<svg> extracts a block-grid logo (rects on a regular pitch) into ASSETS_BLOCKS[name]."""
import base64, json, mimetypes, re, sys, pathlib
proj = pathlib.Path(sys.argv[1]); assets, blocks = {}, {}
for arg in sys.argv[2:]:
    k, v = arg.split('=', 1)
    if k == 'blocks_from_svg':
        name, path = v.split(':', 1); s = open(path).read()
        rects = [(float(m[1]), float(m[2]), m[3]) for m in re.finditer(r'<rect[^>]*?x="([\d.]+)"[^>]*?y="([\d.]+)"[^>]*?fill="(#\w+)"', s)]
        if not rects: sys.exit(f'no <rect x y fill> found in {path}')
        xs = sorted({r[0] for r in rects}); pitch = min(b - a for a, b in zip(xs, xs[1:]) if b - a > 0.5)
        x0, y0 = min(r[0] for r in rects), min(r[1] for r in rects)
        cols = {}; [cols.__setitem__(r[2].upper(), cols.get(r[2].upper(), 0) + 1) for r in rects]
        main = max(cols, key=cols.get)
        blocks[name] = [[round((x - x0) / pitch), round((y - y0) / pitch), 0 if c.upper() == main else 1] for x, y, c in rects]
        print(f'{name}: {len(rects)} blocks, pitch {pitch}, accent colors: {[c for c in cols if c != main]}')
        continue
    mime = mimetypes.guess_type(v)[0] or ('image/svg+xml' if v.endswith('.svg') else 'application/octet-stream')
    assets[k] = f'data:{mime};base64,' + base64.b64encode(open(v, 'rb').read()).decode()
(proj / 'assets.js').write_text(f'window.ASSETS = {json.dumps(assets)};\nwindow.ASSETS_BLOCKS = {json.dumps(blocks)};\n')
print('wrote', proj / 'assets.js', 'images:', list(assets), 'block logos:', list(blocks))
