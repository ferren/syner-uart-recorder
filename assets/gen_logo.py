#!/usr/bin/env python3
"""Generate Syner UART Recorder logo assets: logo.png, logo.ico, logo.svg."""

from pathlib import Path
import math
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).parent
W, H = 512, 512
RADIUS = 96

BG = "#0f172a"
BORDER = "#22d3ee"
S_FILL = "#a5f3fc"
SHADOW = "#000000"
WAVE = "#f8fafc"


def find_font(size):
    candidates = [
        r"C:\Windows\Fonts\segoeuib.ttf",
        r"C:\Windows\Fonts\arialbd.ttf",
        r"C:\Windows\Fonts\msyhbd.ttc",
    ]
    for p in candidates:
        try:
            return ImageFont.truetype(str(p), size)
        except Exception:
            pass
    return ImageFont.load_default()


def make_png():
    im = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    draw = ImageDraw.Draw(im)

    # background + border
    draw.rounded_rectangle([0, 0, W, H], radius=RADIUS, fill=BG, outline=BORDER, width=12)

    # big "S"
    font = find_font(280)
    bbox = draw.textbbox((0, 0), "S", font=font)
    tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
    cx, cy = W / 2, H / 2
    tx, ty = cx - tw / 2, cy - th / 2 - 16
    draw.text((tx + 8, ty + 8), "S", font=font, fill=(0, 0, 0, 80))
    draw.text((tx, ty), "S", font=font, fill=S_FILL)

    # sine wave under the S
    wave_y = cy + th / 2 + 28
    pts = []
    for i in range(100):
        x = (W - 80) * i / 99 + 40
        y = wave_y + math.sin(i * 0.28) * 20
        pts.append((x, y))
    draw.line(pts, fill=WAVE, width=10, joint="curve")

    png = im.resize((256, 256), Image.Resampling.LANCZOS)
    png.save(ROOT / "logo.png")

    ico = im.resize((256, 256), Image.Resampling.LANCZOS)
    (ROOT / "logo.ico").unlink(missing_ok=True)
    ico.save(
        ROOT / "logo.ico",
        format="ICO",
        sizes=[(16, 16), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )

    return im


def make_svg():
    from xml.sax.saxutils import escape

    wave_y = H / 2 + 110
    d = [f"M {40:.1f} {wave_y:.1f}"]
    for i in range(1, 100):
        x = (W - 80) * i / 99 + 40
        y = wave_y + math.sin(i * 0.28) * 20
        d.append(f"L {x:.1f} {y:.1f}")
    path = " ".join(d)

    svg = f"""<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="{W}" y2="{H}">
      <stop offset="0%" stop-color="#0f172a"/>
      <stop offset="100%" stop-color="#1e293b"/>
    </linearGradient>
  </defs>
  <rect x="0" y="0" width="{W}" height="{H}" rx="{RADIUS}" fill="url(#bg)" stroke="{BORDER}" stroke-width="12"/>
  <text x="{W/2}" y="{H/2}" font-family="Segoe UI, Arial, sans-serif" font-weight="700" font-size="280" fill="{S_FILL}" text-anchor="middle" dominant-baseline="central">S</text>
  <path d="{path}" stroke="{WAVE}" stroke-width="10" fill="none" stroke-linecap="round" stroke-linejoin="round"/>
</svg>
"""
    (ROOT / "logo.svg").write_text(svg, encoding="utf-8")


if __name__ == "__main__":
    make_png()
    make_svg()
    print("generated logo.png, logo.ico, logo.svg")
