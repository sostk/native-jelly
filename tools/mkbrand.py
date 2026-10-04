#!/usr/bin/env python3
"""Draw the Native Jelly brand masters: `assets/logo-master.png` and `assets/splash-master.png`.

    python3 tools/mkbrand.py
    python3 tools/mkicons.py assets/logo-master.png --splash=assets/splash-master.png

The mark is a soft, rounded play triangle — a jelly sweet that is also a play button — in a
purple-to-cyan gradient with a glossy highlight. It is drawn here from geometry, not traced from
anyone's logo, and the wordmark is set in the app's own Inter so the splash and the UI share one
typeface. Everything is rendered at 4x and downsampled, so edges come out anti-aliased.
"""
import math
import pathlib
import sys

from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont

ROOT = pathlib.Path(__file__).resolve().parent.parent
SS = 4

BG = (9, 8, 16)
PURPLE = (170, 92, 195)
VIOLET = (120, 86, 220)
CYAN = (0, 170, 225)
INK = (244, 242, 250)
MUTED = (150, 146, 172)


def lerp(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def gradient(size, stops, angle_deg):
    """A linear gradient across `size` at `angle_deg`, through `stops` [(t, rgb), …]."""
    w, h = size
    small = (max(2, w // 8), max(2, h // 8))
    img = Image.new("RGB", small)
    px = img.load()
    a = math.radians(angle_deg)
    dx, dy = math.cos(a), math.sin(a)
    span = abs(small[0] * dx) + abs(small[1] * dy)
    for y in range(small[1]):
        for x in range(small[0]):
            t = ((x - small[0] / 2) * dx + (y - small[1] / 2) * dy) / span + 0.5
            t = min(1.0, max(0.0, t))
            for (t0, c0), (t1, c1) in zip(stops, stops[1:]):
                if t <= t1:
                    px[x, y] = lerp(c0, c1, (t - t0) / max(1e-6, t1 - t0))
                    break
    return img.resize(size, Image.BICUBIC)


def rounded_triangle_mask(size, box, radius):
    """A play triangle (pointing right) inside `box`, every corner rounded by `radius`."""
    x0, y0, x1, y1 = box
    verts = [(x0, y0), (x1, (y0 + y1) / 2), (x0, y1)]
    cx = sum(v[0] for v in verts) / 3
    cy = sum(v[1] for v in verts) / 3
    # Inset each vertex toward the centroid so the rounded result still fills `box`.
    inset = []
    for vx, vy in verts:
        d = math.hypot(vx - cx, vy - cy)
        k = radius / math.sin(math.radians(30)) if d else 0
        inset.append((vx + (cx - vx) * k / d, vy + (cy - vy) * k / d))
    m = Image.new("L", size, 0)
    d = ImageDraw.Draw(m)
    pts = []
    for i in range(3):
        (ax, ay), (bx, by) = inset[i], inset[(i + 1) % 3]
        ex, ey = bx - ax, by - ay
        n = math.hypot(ex, ey)
        nx, ny = ey / n, -ex / n
        if (ax + nx - cx) ** 2 + (ay + ny - cy) ** 2 < (ax - cx) ** 2 + (ay - cy) ** 2:
            nx, ny = -nx, -ny
        pts += [(ax + nx * radius, ay + ny * radius), (bx + nx * radius, by + ny * radius)]
    d.polygon(pts, fill=255)
    for vx, vy in inset:
        d.ellipse((vx - radius, vy - radius, vx + radius, vy + radius), fill=255)
    return m


def mark(side):
    """The mark alone on transparency, `side` px square (already downsampled)."""
    S = side * SS
    pad = S * 0.08
    box = (pad + S * 0.06, pad, S - pad, S - pad)
    shape = rounded_triangle_mask((S, S), box, S * 0.12)
    fill = gradient((S, S), [(0.0, PURPLE), (0.45, VIOLET), (0.85, CYAN), (1.0, CYAN)], 20)
    out = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    out.paste(fill, (0, 0), shape)

    # Gloss: a soft white lens in the upper-left, kept inside the shape.
    gloss = Image.new("L", (S, S), 0)
    gd = ImageDraw.Draw(gloss)
    gd.ellipse((box[0] + S * 0.06, box[1] + S * 0.12, box[0] + S * 0.36, box[1] + S * 0.30), fill=95)
    gloss = gloss.filter(ImageFilter.GaussianBlur(S * 0.03))
    gloss = ImageChops.multiply(gloss, shape)
    white = Image.new("RGBA", (S, S), (255, 255, 255, 0))
    white.putalpha(gloss)
    out = Image.alpha_composite(out, white)

    # Depth: a faint darker rim along the lower edge.
    rim = shape.filter(ImageFilter.GaussianBlur(S * 0.02))
    rim = ImageChops.subtract(shape, ImageChops.offset(rim, 0, -int(S * 0.025)))
    shade = Image.new("RGBA", (S, S), (20, 10, 60, 0))
    shade.putalpha(rim.point(lambda v: v * 0.5))
    out = Image.alpha_composite(out, shade)
    return out.resize((side, side), Image.LANCZOS)


def glow(size, center, radius, color, strength):
    g = Image.new("L", size, 0)
    d = ImageDraw.Draw(g)
    cx, cy = center
    d.ellipse((cx - radius, cy - radius, cx + radius, cy + radius), fill=int(255 * strength))
    g = g.filter(ImageFilter.GaussianBlur(radius * 0.6))
    layer = Image.new("RGBA", size, color + (0,))
    layer.putalpha(g)
    return layer


def logo_master(path, side=1254):
    # Flat panel: the launcher paints iconColor behind the tile, and mkicons.py both samples the
    # corner for it and measures "ink" as anything off-panel, so a glow would read as logo.
    img = Image.new("RGBA", (side, side), BG + (255,))
    m = mark(int(side * 0.62))
    img.alpha_composite(m, ((side - m.width) // 2 + int(side * 0.02), (side - m.height) // 2))
    img.convert("RGB").save(path)


def splash_master(path, w=1920, h=1080):
    img = Image.new("RGBA", (w, h), BG + (255,))
    img = Image.alpha_composite(img, glow((w, h), (w / 2, h * 0.42), 360, VIOLET, 0.30))
    img = Image.alpha_composite(img, glow((w, h), (w * 0.58, h * 0.46), 260, CYAN, 0.12))
    m = mark(250)
    img.alpha_composite(m, ((w - m.width) // 2 + 6, int(h * 0.42 - m.height / 2) - 40))

    bold = ImageFont.truetype(str(ROOT / "pkg/appfont-bold.ttf"), 96 * SS)
    reg = ImageFont.truetype(str(ROOT / "pkg/appfont.ttf"), 30 * SS)
    text = Image.new("RGBA", (w * SS, h * SS), (0, 0, 0, 0))
    td = ImageDraw.Draw(text)
    word_y = int(h * 0.42 + 130) * SS
    a, b = "Native ", "Jelly"
    wa = td.textlength(a, font=bold)
    wb = td.textlength(b, font=bold)
    x = (w * SS - (wa + wb)) / 2
    td.text((x, word_y), a, font=bold, fill=INK)
    # "Jelly" carries the mark's gradient.
    jmask = Image.new("L", text.size, 0)
    ImageDraw.Draw(jmask).text((x + wa, word_y), b, font=bold, fill=255)
    jbox = jmask.getbbox()
    if jbox:
        grad = gradient((jbox[2] - jbox[0], jbox[3] - jbox[1]), [(0.0, PURPLE), (1.0, CYAN)], 0)
        text.paste(grad, jbox[:2], jmask.crop(jbox))
    tag = "A NATIVE JELLYFIN CLIENT"
    spaced = " ".join(tag)
    wt = td.textlength(spaced, font=reg)
    td.text(((w * SS - wt) / 2, word_y + 150 * SS), spaced, font=reg, fill=MUTED)
    text = text.resize((w, h), Image.LANCZOS)
    img = Image.alpha_composite(img, text)
    img.convert("RGB").save(path)


def main():
    out = ROOT / "assets"
    logo_master(out / "logo-master.png")
    splash_master(out / "splash-master.png")
    print("wrote", out / "logo-master.png", "and", out / "splash-master.png")
    return 0


if __name__ == "__main__":
    sys.exit(main())
