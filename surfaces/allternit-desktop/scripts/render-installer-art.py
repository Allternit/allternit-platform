#!/usr/bin/env python3
"""Render the installer art in build/ from the A://TERNIT wordmark.

Writes dmg-background.png + dmg-background@2x.png (macOS DMG window;
electron-builder merges the pair into a Retina TIFF) and installer-sidebar.bmp +
installer-header.bmp (Windows NSIS, 24-bit). Icon slots match dmg.contents in
package.json (app at 130,200, Applications at 410,200, 80 px icons).
Also writes trayTemplate.png + @2x: the menu bar icon, the app-icon A alone in
an 18 pt square, black on transparent (macOS tints template images).

Needs Google Chrome and Pillow. Run: python3 scripts/render-installer-art.py
"""
import re
import subprocess
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent
BUILD = HERE.parent / "build"
WORDMARK = HERE.parents[1] / "platform.allternit.com/public/brand/allternit-wordmark.svg"
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
INK, CORAL, PLATE = "#141413", "#D97757", "#FDF8F3"

rects = "".join(re.findall(r"<rect[^>]*/>", WORDMARK.read_text()))
# The A block of the wordmark (first 5 columns) is the app mark.
a_rects = "".join(r for r in re.findall(r"<rect[^>]*/>", rects) if float(re.search(r'x="([\d.]+)"', r)[1]) < 50)

BASE_CSS = """
html,body{margin:0;overflow:hidden}
body{background:linear-gradient(180deg,#FFFFFF 0%,#F6F6F4 100%);position:relative;
  font-family:-apple-system,"SF Pro Text","Segoe UI",system-ui,sans-serif}
.mono{font-family:ui-monospace,"SF Mono",Menlo,Consolas,monospace;letter-spacing:.08em;color:#A3A29C}
svg{position:absolute;display:block}
"""


def wordmark(css: str) -> str:
    return f'<svg style="{css}" viewBox="0 0 510 50" xmlns="http://www.w3.org/2000/svg">{rects}</svg>'


def arrow() -> str:
    # Block trail fading in toward a block chevron with a coral tip, on the wordmark's 10 px grid.
    y = 196.5
    cells = [
        f'<rect x="{x}" y="{y}" width="7" height="7" rx="1.5" fill="{INK}" fill-opacity="{0.08 + 0.42 * i / 12:.2f}"/>'
        for i, x in enumerate(range(200, 330, 10))
    ]
    for dx, dy in [(-10, -10), (-10, 10), (-20, -20), (-20, 20)]:
        cells.append(f'<rect x="{340 + dx}" y="{y + dy}" width="7" height="7" rx="1.5" fill="{INK}"/>')
    cells.append(f'<rect x="340" y="{y}" width="7" height="7" rx="1.5" fill="{CORAL}"/>')
    return f'<svg style="left:0;top:0" width="540" height="380">{"".join(cells)}</svg>'


DMG = f"""
{wordmark("left:130px;top:38px;width:280px")}
<div style="position:absolute;top:86px;width:100%;text-align:center;font-size:12.5px;letter-spacing:.01em;color:#73726C">
  Drag Allternit Desktop into Applications to install</div>
<div style="position:absolute;left:97px;top:167px;width:66px;height:66px;border-radius:15px;background:{PLATE};
  box-shadow:0 1px 2px rgba(20,20,19,.08),0 6px 16px rgba(20,20,19,.10)"></div>
{arrow()}
<div class="mono" style="position:absolute;bottom:22px;width:100%;text-align:center;font-size:10.5px">ALLTERNIT.COM</div>
"""

SIDEBAR = f"""
<div style="position:absolute;left:42px;top:64px;width:80px;height:80px;border-radius:18px;background:{PLATE};
  box-shadow:0 1px 2px rgba(20,20,19,.08),0 6px 16px rgba(20,20,19,.10)"></div>
<svg style="left:57px;top:79px;width:50px;height:50px" viewBox="0 0 50 50" xmlns="http://www.w3.org/2000/svg">{a_rects}</svg>
{wordmark("left:22px;top:178px;width:120px")}
<div style="position:absolute;top:196px;width:100%;text-align:center;font-size:11px;color:#73726C">Desktop</div>
<div class="mono" style="position:absolute;bottom:18px;width:100%;text-align:center;font-size:8.5px">ALLTERNIT.COM</div>
"""

HEADER = f"""
<style>body{{background:#FFFFFF}}</style>
{wordmark("left:14px;top:23.5px;width:122px")}
"""


def render(body: str, w: int, h: int, scale: int, out: Path) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        page = Path(tmp) / "page.html"
        page.write_text(f'<!doctype html><meta charset="utf-8"><style>{BASE_CSS}html,body{{width:{w}px;height:{h}px}}</style>{body}')
        subprocess.run(
            [CHROME, "--headless=new", "--disable-gpu", "--hide-scrollbars", f"--force-device-scale-factor={scale}",
             f"--window-size={w},{h}", f"--screenshot={out}", page.as_uri()],
            check=True, capture_output=True,
        )


def bmp(body: str, w: int, h: int, name: str) -> None:
    png = BUILD / f"{name}.tmp.png"
    render(body, w, h, 1, png)
    Image.open(png).convert("RGB").save(BUILD / f"{name}.bmp")
    png.unlink()


def tray(scale: int) -> None:
    # The A glyph on a 5x5 grid: 3.5 pt cells, 3 pt blocks, centred in 18 pt.
    cells = [(2, 0), (1, 1), (3, 1), (0, 2), (1, 2), (2, 2), (3, 2), (4, 2), (0, 3), (4, 3), (0, 4), (4, 4)]
    size, cell, block = 18 * scale, 3.5 * scale, 3 * scale
    origin = (size - (4 * cell + block)) / 2
    im = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(im)
    for col, row in cells:
        x, y = origin + col * cell, origin + row * cell
        draw.rounded_rectangle([x, y, x + block - 1, y + block - 1], radius=0.75 * scale, fill=(0, 0, 0, 255))
    im.save(BUILD / ("trayTemplate@2x.png" if scale == 2 else "trayTemplate.png"))


render(DMG, 540, 380, 1, BUILD / "dmg-background.png")
render(DMG, 540, 380, 2, BUILD / "dmg-background@2x.png")
bmp(SIDEBAR, 164, 314, "installer-sidebar")
bmp(HEADER, 150, 57, "installer-header")
tray(1)
tray(2)
print("wrote", ", ".join(p.name for p in sorted(BUILD.glob("dmg-background*.png")) + sorted(BUILD.glob("installer-*.bmp")) + sorted(BUILD.glob("trayTemplate*.png"))))
