#!/usr/bin/env python3
"""Draw a headless SIGPIPE frame as a PNG, in a real monospace font.

    cargo run -- --shot 30 --moves PRP --cells | python3 tools/shot.py out.png
"""
import re
import sys
from PIL import Image, ImageDraw, ImageFont

FONT = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"
CW, CH = 10, 20
font = ImageFont.truetype(FONT, 17)
lines = sys.stdin.read().splitlines()
w, h = map(int, lines[0].split())
img = Image.new("RGB", (w * CW, h * CH))
d = ImageDraw.Draw(img)
rgb = lambda s: tuple(int(s[i:i + 2], 16) for i in (0, 2, 4))
for line in lines[1:]:
    x, y, fg, bg, sym = line.split(" ", 4)
    x, y, fg, bg = int(x), int(y), rgb(fg), rgb(bg)
    sym = "".join(chr(int(c, 16)) for c in re.findall(r"\{([0-9a-f]+)\}", sym))
    box = (x * CW, y * CH, x * CW + CW - 1, y * CH + CH - 1)
    d.rectangle(box, fill=bg)
    if sym == "▀":
        d.rectangle((box[0], box[1], box[2], box[1] + CH // 2 - 1), fill=fg)
    elif sym == "▄":
        d.rectangle((box[0], box[1] + CH // 2, box[2], box[3]), fill=fg)
    elif sym == "█":
        d.rectangle(box, fill=fg)
    elif sym.strip():
        d.text((x * CW, y * CH), sym, font=font, fill=fg)
img.save(sys.argv[1])
