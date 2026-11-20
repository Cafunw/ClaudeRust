# Generates the app icon (original artwork): warm rounded square with a white
# "C"-shaped ring with gear teeth (Rust nod). Run: python3 make_icon.py out.png
import math, sys
from PIL import Image, ImageDraw, ImageFilter

S = 1024
img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
# background gradient
bg = Image.new("RGBA", (S, S))
px = bg.load()
c1, c2 = (224, 123, 90), (196, 92, 62)
for y in range(S):
    for x in range(S):
        t = (x + y) / (2 * S)
        px[x, y] = tuple(int(c1[i] * (1 - t) + c2[i] * t) for i in range(3)) + (255,)
mask = Image.new("L", (S, S), 0)
ImageDraw.Draw(mask).rounded_rectangle([40, 40, S - 40, S - 40], radius=220, fill=255)
img.paste(bg, (0, 0), mask)

d = ImageDraw.Draw(img)
cx, cy = S / 2, S / 2
R_out, R_in = 330, 215
white = (255, 248, 242, 255)
# gear teeth around the ring, skipping the opening on the right
gap = math.radians(38)
teeth = 14
for k in range(teeth):
    a = 2 * math.pi * k / teeth
    if abs((a + math.pi) % (2 * math.pi) - math.pi) < gap + 0.12:
        continue
    w = math.radians(8)
    pts = []
    for (r, da) in [(R_out - 10, -w * 1.3), (R_out + 62, -w), (R_out + 62, w), (R_out - 10, w * 1.3)]:
        pts.append((cx + r * math.cos(a + da), cy + r * math.sin(a + da)))
    d.polygon(pts, fill=white)
# ring as thick arc ("C")
ring = Image.new("L", (S, S), 0)
rd = ImageDraw.Draw(ring)
rd.ellipse([cx - R_out, cy - R_out, cx + R_out, cy + R_out], fill=255)
rd.ellipse([cx - R_in, cy - R_in, cx + R_in, cy + R_in], fill=0)
# cut opening wedge
wedge = [(cx, cy)]
for i in range(41):
    a = -gap + 2 * gap * i / 40
    wedge.append((cx + 600 * math.cos(a), cy + 600 * math.sin(a)))
rd.polygon(wedge, fill=0)
img.paste(Image.new("RGBA", (S, S), white), (0, 0), ring)
# round caps at arc ends
rm = (R_out + R_in) / 2
cap = (R_out - R_in) / 2
for sgn in (-1, 1):
    a = sgn * gap
    x, y = cx + rm * math.cos(a), cy + rm * math.sin(a)
    d.ellipse([x - cap, y - cap, x + cap, y + cap], fill=white)
# small spark dot in the opening
d.ellipse([cx + 250 - 46, cy - 46, cx + 250 + 46, cy + 46], fill=white)
img.save(sys.argv[1] if len(sys.argv) > 1 else "icon-src.png")
