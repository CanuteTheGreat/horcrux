"""Derive web/installer/boot brand assets from an OFFICIAL logo by crop + scale + background tile only.
usage: derive_logo.py SRC.png OUTDIR NAME MARKBOX WORDBOX [HEADBOX]   boxes = x0,y0,x1,y1 (source px)
"""
import sys, os
from PIL import Image, ImageDraw

src, out, name, mb, wb = sys.argv[1:6]
hb = sys.argv[6] if len(sys.argv) > 6 else None
box = lambda s: tuple(int(v) for v in s.split(","))
im = Image.open(src).convert("RGB")
os.makedirs(out, exist_ok=True)
bg = im.getpixel((5, 5))

def tile(img, size, radius_frac=0.18):
    """crop already square; scale (LANCZOS, downscale only) and round the corners on a transparent canvas"""
    t = img.resize((size, size), Image.LANCZOS).convert("RGBA")
    m = Image.new("L", (size * 4, size * 4), 0)
    ImageDraw.Draw(m).rounded_rectangle((0, 0, size * 4 - 1, size * 4 - 1), radius=int(size * 4 * radius_frac), fill=255)
    t.putalpha(m.resize((size, size), Image.LANCZOS))
    return t

def crop(b):
    x0, y0, x1, y1 = b
    c = Image.new("RGB", (x1 - x0, y1 - y0), bg)
    c.paste(im.crop((max(x0, 0), max(y0, 0), min(x1, im.size[0]), min(y1, im.size[1]))), (max(-x0, 0), max(-y0, 0)))
    cut = int(os.environ.get("CUTY", "0"))   # wordmark overlaps the crest: paint bg below this source row
    if cut:
        ImageDraw.Draw(c).rectangle((0, max(cut - y0, 0), c.size[0], c.size[1]), fill=bg)
    return c

mark = crop(box(mb))
assert mark.size[0] == mark.size[1], mark.size
for s in (512, 256, 180, 128, 64):
    tile(mark, s).save(f"{out}/{name}-mark-{s}.png", optimize=True)
small = crop(box(hb)) if hb else mark
for s in (48, 32, 16):
    tile(small, s, 0.15).save(f"{out}/{name}-mark-{s}.png", optimize=True)
# favicon.ico: 16/32/48 (tight crop) + 64
ico = [tile(small, s, 0.15) for s in (16, 32, 48)] + [tile(mark, 64)]
ico[-1].save(f"{out}/favicon.ico", format="ICO", sizes=[(16, 16), (32, 32), (48, 48), (64, 64)], append_images=ico[:-1])
w = crop(box(wb))
w.save(f"{out}/{name}-wordmark.png", optimize=True)
w.resize((w.size[0] // 2, w.size[1] // 2), Image.LANCZOS).save(f"{out}/{name}-wordmark-half.png", optimize=True)
full = im
full.resize((im.size[0] // 2, im.size[1] // 2), Image.LANCZOS).save(f"{out}/{name}-full-half.png", optimize=True)
# contact sheet for review
sheet = Image.new("RGB", (1500, 640), (255, 255, 255))
x = 10
for s in (256, 128, 64, 48, 32, 16):
    t = Image.open(f"{out}/{name}-mark-{s}.png"); sheet.paste(t, (x, 10), t); x += s + 10
x = 10
for s in (64, 32, 16):  # 1:1 and 4x nearest to judge mush
    t = Image.open(f"{out}/{name}-mark-{s}.png"); sheet.paste(t.resize((s * 4, s * 4), Image.NEAREST), (x, 300), t.resize((s * 4, s * 4), Image.NEAREST)); x += s * 4 + 20
ws = w.resize((w.size[0] // 2, w.size[1] // 2)); sheet.paste(ws, (700, 300))
sheet.save(f"{out}/_sheet-{name}.png")
print("bg", bg, "mark", mark.size, "word", w.size)
