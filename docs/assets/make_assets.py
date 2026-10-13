#!/usr/bin/env python3
"""Generates Klip's logo files (SVG + PNG) from one description, so they never drift apart.

    python3 docs/assets/make_assets.py

Brand in one paragraph: ink (#0B0E10) and one signal colour, lime (#C8F25A). The mark is two devices
joined by a line, the same picture the Android home screen draws live. The wordmark is lowercase,
geometric, drawn from strokes (no font needed); the dot of the i is the signal colour.
"""
import os
from PIL import Image, ImageDraw, ImageFont

INK, LIME, TEXT, DIM = "#0B0E10", "#C8F25A", "#E8EDF0", "#8C979F"
HERE = os.path.dirname(os.path.abspath(__file__))
FONT = "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf"


class Canvas:
    """Collects shapes once; renders to SVG text and to a PIL image."""

    def __init__(self, w, h, bg=None):
        self.w, self.h, self.bg, self.items = w, h, bg, []

    def rrect(self, x, y, w, h, r, fill):
        self.items.append(("rrect", x, y, w, h, r, fill))

    def line(self, x1, y1, x2, y2, width, color):
        self.items.append(("line", x1, y1, x2, y2, width, color))

    def circle(self, cx, cy, r, fill=None, stroke=None, width=0, alpha=1.0):
        self.items.append(("circle", cx, cy, r, fill, stroke, width, alpha))

    def text(self, x, y, s, size, color, anchor="start"):
        self.items.append(("text", x, y, s, size, color, anchor))

    def svg(self):
        o = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{self.w}" height="{self.h}" viewBox="0 0 {self.w} {self.h}">']
        if self.bg:
            o.append(f'<rect width="{self.w}" height="{self.h}" fill="{self.bg}"/>')
        for it in self.items:
            k = it[0]
            if k == "rrect":
                _, x, y, w, h, r, f = it
                o.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" fill="{f}"/>')
            elif k == "line":
                _, x1, y1, x2, y2, w, c = it
                o.append(f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{c}" stroke-width="{w}" stroke-linecap="round"/>')
            elif k == "circle":
                _, cx, cy, r, f, s, w, a = it
                fill = f'fill="{f}"' if f else 'fill="none"'
                stroke = f'stroke="{s}" stroke-width="{w}"' if s else ""
                o.append(f'<circle cx="{cx}" cy="{cy}" r="{r}" {fill} {stroke} opacity="{a}"/>')
            elif k == "text":
                _, x, y, s, size, c, a = it
                o.append(f'<text x="{x}" y="{y}" fill="{c}" font-size="{size}" text-anchor="{a}" '
                         f'font-family="Inter, -apple-system, Segoe UI, Roboto, Helvetica, Arial, sans-serif">{s}</text>')
        o.append("</svg>")
        return "\n".join(o)

    def png(self, path, scale=3):
        S = scale
        im = Image.new("RGBA", (self.w * S, self.h * S), self.bg or (0, 0, 0, 0))
        d = ImageDraw.Draw(im, "RGBA")

        def rgba(c, a=1.0):
            c = c.lstrip("#")
            return (int(c[0:2], 16), int(c[2:4], 16), int(c[4:6], 16), int(255 * a))

        for it in self.items:
            k = it[0]
            if k == "rrect":
                _, x, y, w, h, r, f = it
                d.rounded_rectangle([x * S, y * S, (x + w) * S, (y + h) * S], r * S, fill=rgba(f))
            elif k == "line":
                _, x1, y1, x2, y2, w, c = it
                d.line([x1 * S, y1 * S, x2 * S, y2 * S], fill=rgba(c), width=int(w * S))
                for (px, py) in ((x1, y1), (x2, y2)):  # round caps
                    d.ellipse([(px - w / 2) * S, (py - w / 2) * S, (px + w / 2) * S, (py + w / 2) * S], fill=rgba(c))
            elif k == "circle":
                _, cx, cy, r, f, s, w, a = it
                box = [(cx - r) * S, (cy - r) * S, (cx + r) * S, (cy + r) * S]
                # Draw on its own layer and composite, so translucent shapes blend instead of replacing.
                layer = Image.new("RGBA", im.size, (0, 0, 0, 0))
                ld = ImageDraw.Draw(layer, "RGBA")
                if f:
                    ld.ellipse(box, fill=rgba(f, a))
                if s:
                    ld.ellipse(box, outline=rgba(s, a), width=int(w * S))
                im = Image.alpha_composite(im, layer)
                d = ImageDraw.Draw(im, "RGBA")
            elif k == "text":
                _, x, y, s, size, c, a = it
                font = ImageFont.truetype(FONT, int(size * S))
                tw = d.textlength(s, font=font)
                px = x * S - (tw / 2 if a == "middle" else 0)
                d.text((px, y * S), s, font=font, fill=rgba(c), anchor="ls")
        im = im.resize((self.w, self.h), Image.LANCZOS)
        im.save(path)


def mark(c, cx, cy, scale=1.0, color=LIME, halo=True):
    """Two devices and the link between them."""
    gap, r = 100 * scale, 19 * scale
    left, right = cx - gap, cx + gap
    c.line(left, cy, right, cy, 11 * scale, color)
    if halo:
        c.circle(left, cy, r + 14 * scale, fill=color, alpha=0.18)
    c.circle(left, cy, r, fill=color)
    c.circle(right, cy, r, fill=color)


def wordmark(c, x, baseline, h, color=TEXT, accent=LIME):
    """'klip' from strokes. h = ascender height (baseline - top of k/l)."""
    u = h / 100.0
    w = 15 * u
    xh = 36 * u                       # x-height offset from the top
    top = baseline - h
    xt = top + xh
    cx = x

    def L(x1, y1, x2, y2, col=color):
        c.line(cx + x1 * u, top + y1 * u, cx + x2 * u, top + y2 * u, w, col)

    # k
    L(0, 0, 0, 100)
    L(2, 76, 44, 36)
    L(16, 60, 46, 100)
    cx += 70 * u
    # l
    L(0, 0, 0, 100)
    cx += 40 * u
    # i (dot in the signal colour)
    L(0, 36, 0, 100)
    c.circle(cx, top + 6 * u, 9 * u, fill=accent)
    cx += 40 * u
    # p
    L(0, 36, 0, 136)
    c.circle(cx + 33 * u, top + 68 * u, 31 * u, stroke=color, width=w)
    return cx + 70 * u


def write(c, name, png=True):
    with open(os.path.join(HERE, name + ".svg"), "w") as f:
        f.write(c.svg())
    if png:
        c.png(os.path.join(HERE, name + ".png"))


# 1) App icon / avatar, 512 x 512.
icon = Canvas(512, 512)
icon.rrect(0, 0, 512, 512, 112, INK)
mark(icon, 256, 256, scale=1.55)
write(icon, "logo-mark")

# 2) Wordmark alone (light on transparent), used in docs.
wm = Canvas(420, 180)
wordmark(wm, 20, 120, 100)
write(wm, "wordmark", png=False)

# Width of the wordmark in units of its ascender height (see wordmark()).
WM_W = 2.2

# 3) README banner, 1280 x 260: mark and wordmark as one centred group, tagline under it.
b = Canvas(1280, 260, bg=INK)
h = 96
group = 2 * (100 * 1.1 + 33) + 56 + WM_W * h
x0 = (1280 - group) / 2
mark(b, x0 + 100 * 1.1 + 33, 108, scale=1.1)
wordmark(b, x0 + 2 * (100 * 1.1 + 33) + 56, 108 + h * 0.34, h)   # centre of the x-height sits on the link line
b.text(640, 226, "Your clipboard, on all your devices.", 28, DIM, anchor="middle")
write(b, "banner")

# 4) GitHub social preview, 1280 x 640.
s = Canvas(1280, 640, bg=INK)
mark(s, 640, 165, scale=1.6)
h = 130
wordmark(s, 640 - WM_W * h / 2, 390, h)
s.text(640, 530, "Your clipboard, on all your devices.", 40, TEXT, anchor="middle")
s.text(640, 590, "No account  ·  peer-to-peer  ·  open source", 26, DIM, anchor="middle")
write(s, "social-preview")
print("written to", HERE)
