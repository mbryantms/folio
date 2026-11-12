#!/usr/bin/env python3
"""Render synthetic comic-like pages with a known transcription.

For building and testing `ocr-bench` before real pages exist. Writes
`<out>/pages/synthetic/pNNN.jpg` (about 2000 x 3000 px) and
`<out>/truth/synthetic-pNNN.json` in the truth-file shape the harness
reads: a list of regions in reading order with `id`, `kind`, `text`,
`bbox_pct` (x, y, w, h in percent of the page), `colour_case`, `notes`.

Uses only fonts already installed on the machine; nothing is downloaded.

    python3 -I make_synthetic.py OUT_DIR
"""

import json
import random
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

W, H = 2000, 3000
FONTS = [
    "/usr/share/fonts/gsfonts/NimbusSansNarrow-Bold.otf",
    "/usr/share/fonts/TTF/Roboto-Bold.ttf",
    "/usr/share/fonts/noto/NotoSans-Bold.ttf",
    "/usr/share/fonts/TTF/RobotoCondensed-Bold.ttf",
    "/usr/share/fonts/gsfonts/URWGothic-Demi.otf",
]

LINES = [
    "WHAT IN BLAZES IS GOING ON OUT THERE?",
    "STAND BACK, FOLKS! THIS IS A JOB FOR THE POLICE!",
    "I'VE GOT TO REACH THE TOWER BEFORE MIDNIGHT...",
    "YOU FOOLS! YOU CAN'T STOP ME NOW!",
    "MEANWHILE, ACROSS TOWN...",
    "HOLD IT RIGHT THERE, MISTER!",
    "THE SIGNAL! IT CAME FROM THE OLD MILL!",
    "DON'T WORRY, KID. I'LL HAVE YOU HOME IN NO TIME.",
    "LATER THAT NIGHT, AT THE DOCKS...",
    "QUICK -- INTO THE CAR!",
    "SO THAT'S HOW HE DID IT! THE MIRROR WAS A TRICK!",
    "NOBODY ESCAPES FROM DOCTOR ZORN!",
    "LOOK OUT! THE BRIDGE IS COLLAPSING!",
    "HA! HA! HA! AFTER HIM, BOYS!",
    "WELL, I NEVER! WHAT A RUDE YOUNG MAN!",
    "THREE DAYS LATER...",
]


def wrap(text, font, max_w, draw):
    words, lines, cur = text.split(), [], ""
    for w in words:
        trial = (cur + " " + w).strip()
        if draw.textlength(trial, font=font) <= max_w or not cur:
            cur = trial
        else:
            lines.append(cur)
            cur = w
    if cur:
        lines.append(cur)
    return lines


def text_block(draw, lines, font, cx, cy, fill, spacing):
    asc, desc = font.getmetrics()
    lh = asc + desc + spacing
    total = lh * len(lines) - spacing
    y = cy - total / 2
    boxes = []
    for line in lines:
        tw = draw.textlength(line, font=font)
        x = cx - tw / 2
        draw.text((x, y), line, font=font, fill=fill)
        boxes.append((x, y, x + tw, y + asc + desc))
        y += lh
    xs = [b[0] for b in boxes] + [b[2] for b in boxes]
    ys = [b[1] for b in boxes] + [b[3] for b in boxes]
    return min(xs), min(ys), max(xs), max(ys)


def page(n, rng, out_pages, out_truth):
    paper = (246, 240, 222)
    img = Image.new("RGB", (W, H), paper)
    d = ImageDraw.Draw(img)
    # Three tiers of panels with simple "art": filled shapes in flat colours.
    tiers = [(80, 1000), (1040, 1960), (2000, 2920)]
    panels = []
    for top, bot in tiers:
        split = rng.randint(800, 1200)
        for x0, x1 in ((80, split - 20), (split + 20, 1920)):
            panels.append((x0, top, x1, bot))
            d.rectangle((x0, top, x1, bot), fill=(250, 248, 240), outline=(20, 20, 20), width=6)
            for _ in range(6):
                cx, cy = rng.randint(x0 + 40, x1 - 40), rng.randint(top + 300, bot - 40)
                r = rng.randint(30, 160)
                col = tuple(rng.randint(60, 220) for _ in range(3))
                if rng.random() < 0.5:
                    d.ellipse((cx - r, cy - r, cx + r, cy + r), fill=col, outline=(15, 15, 15), width=4)
                else:
                    d.rectangle((cx - r, cy - r // 2, cx + r, cy + r // 2), fill=col, outline=(15, 15, 15), width=4)
    regions = []
    texts = rng.sample(LINES, len(panels))
    for i, ((x0, top, x1, bot), text) in enumerate(zip(panels, texts)):
        font_path = FONTS[(n + i) % len(FONTS)]
        size = rng.choice([34, 38, 42, 46])
        font = ImageFont.truetype(font_path, size)
        pw = x1 - x0
        kind = "caption" if text.endswith("...") and i % 2 == 0 else "balloon"
        inverted = kind == "caption" and n % 2 == 1
        max_w = min(pw - 140, 560)
        lines = wrap(text, font, max_w, d)
        asc, desc = font.getmetrics()
        th = (asc + desc + 8) * len(lines)
        tw = max(d.textlength(line, font=font) for line in lines)
        cx = rng.randint(int(x0 + tw / 2 + 80), int(x1 - tw / 2 - 80)) if pw - tw > 200 else (x0 + x1) // 2
        cy = top + 60 + th / 2 + 40
        if kind == "balloon":
            rx, ry = tw / 2 + 70, th / 2 + 55
            d.polygon([(cx - 30, cy + ry - 10), (cx + 10, cy + ry - 10), (cx - 60, cy + ry + 70)], fill="white", outline="black")
            d.ellipse((cx - rx, cy - ry, cx + rx, cy + ry), fill="white", outline="black", width=5)
            colour = "black-on-white"
            fill = (10, 10, 10)
        else:
            rx, ry = tw / 2 + 30, th / 2 + 22
            bg = (20, 20, 20) if inverted else (250, 226, 120)
            d.rectangle((cx - rx, cy - ry, cx + rx, cy + ry), fill=bg, outline="black", width=4)
            colour = "white-on-black" if inverted else "black-on-yellow"
            fill = (250, 250, 250) if inverted else (10, 10, 10)
        bx0, by0, bx1, by1 = text_block(d, lines, font, cx, cy, fill, 8)
        # The reference box is approximate on purpose: a few px loose.
        jx, jy = rng.randint(-12, 12), rng.randint(-12, 12)
        bx0, by0, bx1, by1 = bx0 - 18 + jx, by0 - 14 + jy, bx1 + 18 + jx, by1 + 14 + jy
        regions.append(
            {
                "id": f"r{i + 1}",
                "kind": kind,
                "text": " ".join(lines),
                "bbox_pct": {
                    "x": round(100 * bx0 / W, 2),
                    "y": round(100 * by0 / H, 2),
                    "w": round(100 * (bx1 - bx0) / W, 2),
                    "h": round(100 * (by1 - by0) / H, 2),
                },
                "colour_case": colour,
                "notes": f"synthetic; font {Path(font_path).name} {size}px",
            }
        )
    # Mild scan softness and JPEG, like a real page.
    img = img.filter(ImageFilter.GaussianBlur(0.6))
    name = f"p{n:03d}"
    img.save(out_pages / f"{name}.jpg", quality=88)
    (out_truth / f"synthetic-{name}.json").write_text(json.dumps(regions, indent=2) + "\n")


def main():
    out = Path(sys.argv[1])
    pages = out / "pages" / "synthetic"
    truth = out / "truth"
    pages.mkdir(parents=True, exist_ok=True)
    truth.mkdir(parents=True, exist_ok=True)
    rng = random.Random(20261112)
    for n in range(1, 4):
        page(n, rng, pages, truth)


if __name__ == "__main__":
    main()
