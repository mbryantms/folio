#!/usr/bin/env python3
"""
Synthesize the dev CBZ fixtures from scratch.

These are *placeholder* fixtures — colored panels with the series name + page
number stamped on them — sufficient for verifying the scanner, thumbnail
pipeline, search, and reader UI. Real public-domain comics (the spec's §13.4
plan) come later, behind Git LFS so the repo stays cheap to clone.

Run via `just regen-fixtures` (preferred) or `python3 fixtures/build.py`.
Idempotent: rewrites the .cbz files in-place.

Two scales:
  * default (`--scale dev`)     — 5 issues across 3 series, full-size pages.
                                  Used by the dev seed flow + UI smoke tests.
  * `--scale stress`            — ~50 series × ~20 issues = ~1000 CBZs in
                                  fixtures/library-stress/. Tiny pages so
                                  generation stays under a minute and the
                                  scanner — not the renderer — is the hot
                                  path. ~10 series get a series.json sidecar
                                  so the sidecar reconcile path is exercised.
                                  Gitignored. Used by `just perf-scan` (see
                                  docs/dev/scanner-perf.md).
"""

from __future__ import annotations

import argparse
import io
import json
import os
import random
import zipfile
from dataclasses import dataclass
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent
LIBRARY = ROOT / "library"
STRESS_LIBRARY = ROOT / "library-stress"

# Comic-page aspect: classic 2:3, sized so the cover thumbnail at 600px wide
# downsamples cleanly (the source is 1.5x the target).
PAGE_W, PAGE_H = 900, 1350


def _font(size: int) -> ImageFont.ImageFont:
    """Best-effort font load. Falls back to PIL default (small) if no TTF found."""
    for path in (
        "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        "/System/Library/Fonts/Helvetica.ttc",
    ):
        if os.path.exists(path):
            return ImageFont.truetype(path, size)
    return ImageFont.load_default()


def _png(text_top: str, text_bottom: str, accent: tuple[int, int, int]) -> bytes:
    """Render a placeholder page and return PNG bytes."""
    img = Image.new("RGB", (PAGE_W, PAGE_H), color=(20, 20, 24))
    draw = ImageDraw.Draw(img)

    # Accent block at the top, like a panel border
    draw.rectangle((0, 0, PAGE_W, 24), fill=accent)
    draw.rectangle((0, PAGE_H - 24, PAGE_W, PAGE_H), fill=accent)

    # Series + issue label, big.
    f_big = _font(96)
    f_small = _font(36)
    bbox_top = draw.textbbox((0, 0), text_top, font=f_big)
    tw = bbox_top[2] - bbox_top[0]
    th = bbox_top[3] - bbox_top[1]
    draw.text(
        ((PAGE_W - tw) / 2, (PAGE_H - th) / 2 - 80),
        text_top,
        font=f_big,
        fill=(245, 245, 245),
    )

    bbox_bot = draw.textbbox((0, 0), text_bottom, font=f_small)
    bw = bbox_bot[2] - bbox_bot[0]
    draw.text(
        ((PAGE_W - bw) / 2, (PAGE_H - th) / 2 + 80),
        text_bottom,
        font=f_small,
        fill=(180, 180, 200),
    )

    buf = io.BytesIO()
    img.save(buf, format="PNG", optimize=True)
    return buf.getvalue()


@dataclass
class Issue:
    series: str
    number: str
    year: int
    pages: int
    accent: tuple[int, int, int]
    publisher: str
    writer: str
    penciller: str
    age_rating: str
    title: str | None = None
    summary: str | None = None
    manga: str | None = None
    include_comicinfo: bool = True


def _comicinfo(iss: Issue) -> bytes:
    parts = [
        '<?xml version="1.0" encoding="UTF-8"?>',
        "<ComicInfo>",
    ]
    def add(tag: str, val: object | None) -> None:
        if val is None:
            return
        parts.append(f"  <{tag}>{val}</{tag}>")

    add("Title", iss.title)
    add("Series", iss.series)
    add("Number", iss.number)
    add("Volume", 1)
    add("Year", iss.year)
    add("Summary", iss.summary)
    add("Writer", iss.writer)
    add("Penciller", iss.penciller)
    add("Publisher", iss.publisher)
    add("PageCount", iss.pages)
    add("LanguageISO", "en")
    add("Manga", iss.manga or "No")
    add("AgeRating", iss.age_rating)
    parts.append("  <Pages>")
    for i in range(iss.pages):
        kind = ' Type="FrontCover"' if i == 0 else ""
        parts.append(
            f'    <Page Image="{i}"{kind} ImageWidth="{PAGE_W}" ImageHeight="{PAGE_H}"/>'
        )
    parts.append("  </Pages>")
    parts.append("</ComicInfo>")
    return ("\n".join(parts) + "\n").encode()


def write_cbz(out: Path, iss: Issue) -> None:
    out.parent.mkdir(parents=True, exist_ok=True)
    cover_label = iss.title or f"{iss.series} #{iss.number}"
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as z:
        if iss.include_comicinfo:
            z.writestr("ComicInfo.xml", _comicinfo(iss))
        for n in range(iss.pages):
            top = cover_label if n == 0 else iss.series
            bottom = (
                f"#{iss.number} · {iss.year} · {iss.publisher}"
                if n == 0
                else f"page {n + 1} of {iss.pages}"
            )
            z.writestr(f"page-{n + 1:03d}.png", _png(top, bottom, iss.accent))
    print(f"  wrote {out.relative_to(ROOT)}  ({out.stat().st_size:,} bytes)")


def write_cbz_small(out: Path, series: str, number: int, year: int) -> None:
    """Stress-mode CBZ writer. Tiny pages (150x225, 2 per issue), minimal
    ComicInfo. Optimized for scan throughput, not visual fidelity."""
    out.parent.mkdir(parents=True, exist_ok=True)
    page_w, page_h = 150, 225
    accent = (
        (number * 47) % 255,
        (number * 23) % 255,
        (number * 91) % 255,
    )
    pages = 2
    comicinfo = (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        "<ComicInfo>\n"
        f"  <Series>{series}</Series>\n"
        f"  <Number>{number}</Number>\n"
        "  <Volume>1</Volume>\n"
        f"  <Year>{year}</Year>\n"
        f"  <PageCount>{pages}</PageCount>\n"
        "  <LanguageISO>en</LanguageISO>\n"
        "  <Publisher>Stress Press</Publisher>\n"
        "</ComicInfo>\n"
    ).encode()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=1) as z:
        z.writestr("ComicInfo.xml", comicinfo)
        for n in range(pages):
            img = Image.new("RGB", (page_w, page_h), color=accent)
            buf = io.BytesIO()
            img.save(buf, format="PNG", optimize=False)
            z.writestr(f"page-{n + 1:03d}.png", buf.getvalue())


def build_stress() -> None:
    """Generate ~1000 stress CBZs across ~50 series.

    Layout:
      fixtures/library-stress/
        Series 001 (2010)/
          Series 001 #001 (Stress).cbz
          ...
          series.json     (only for series 001..010)

    Series with sidecars get total_issues + status + description so the
    `series.json` precedence path runs against real data.
    """
    if STRESS_LIBRARY.exists():
        # Idempotent: blow away any prior run so we don't accumulate stale
        # files when the series count changes between invocations.
        import shutil

        shutil.rmtree(STRESS_LIBRARY)
    STRESS_LIBRARY.mkdir(parents=True)

    series_count = 50
    issues_per_series = 20
    sidecar_count = 10
    rng = random.Random(0xCAFE)
    print(
        f"Writing {series_count} series × {issues_per_series} issues = "
        f"{series_count * issues_per_series} stress CBZs to "
        f"{STRESS_LIBRARY.relative_to(ROOT.parent)}/ …"
    )
    for s in range(1, series_count + 1):
        series = f"Series {s:03d}"
        year = 2000 + (s % 25)
        folder = STRESS_LIBRARY / f"{series} ({year})"
        folder.mkdir(parents=True, exist_ok=True)
        for n in range(1, issues_per_series + 1):
            out = folder / f"{series} #{n:03d} (Stress).cbz"
            write_cbz_small(out, series, n, year)
        if s <= sidecar_count:
            sidecar = {
                "metadata": {
                    "name": series,
                    "publisher": "Stress Press",
                    "year_began": year,
                    "total_issues": issues_per_series,
                    "status": rng.choice(["Ended", "Continuing", "Cancelled"]),
                    "description_text": (
                        f"Synthetic stress series {series}. "
                        "Generated for scanner perf profiling — see "
                        "docs/dev/scanner-perf.md."
                    ),
                    "comicid": 100000 + s,
                }
            }
            (folder / "series.json").write_text(json.dumps(sidecar, indent=2))
        if s % 10 == 0:
            print(f"  … {s}/{series_count} series done")
    print(f"done. ({STRESS_LIBRARY})")


def build_dev() -> None:
    fixtures = [
        # Saga: writeup + summary, full ComicInfo
        (
            LIBRARY / "Saga" / "Saga (2012) #001 (Image).cbz",
            Issue(
                series="Saga",
                number="1",
                year=2012,
                pages=4,
                accent=(180, 90, 60),
                publisher="Image Comics",
                writer="Brian K. Vaughan",
                penciller="Fiona Staples",
                age_rating="Mature 17+",
                title="The Boy from Mars",
                summary="An interplanetary love story.",
            ),
        ),
        (
            LIBRARY / "Saga" / "Saga (2012) #002 (Image).cbz",
            Issue(
                series="Saga",
                number="2",
                year=2012,
                pages=3,
                accent=(180, 90, 60),
                publisher="Image Comics",
                writer="Brian K. Vaughan",
                penciller="Fiona Staples",
                age_rating="Mature 17+",
            ),
        ),
        # No-ComicInfo issue: exercises filename inference
        (
            LIBRARY / "Saga" / "Saga (2012) #003 (Image).cbz",
            Issue(
                series="Saga",
                number="3",
                year=2012,
                pages=2,
                accent=(180, 90, 60),
                publisher="Image Comics",
                writer="—",
                penciller="—",
                age_rating="—",
                include_comicinfo=False,
            ),
        ),
        # A second series, different accent: exercises grid-with-multiple-series
        (
            LIBRARY / "Daredevil" / "Daredevil (2019) #001 (Marvel).cbz",
            Issue(
                series="Daredevil",
                number="1",
                year=2019,
                pages=3,
                accent=(180, 30, 30),
                publisher="Marvel",
                writer="Chip Zdarsky",
                penciller="Marco Checchetto",
                age_rating="Teen+",
                title="Know Fear",
                summary="The new direction.",
            ),
        ),
        # A manga sample (RTL): tests the Manga flag & filename inference for vNNN
        (
            LIBRARY / "Berserk" / "Berserk v01 (Dark Horse).cbz",
            Issue(
                series="Berserk",
                number="1",
                year=2003,
                pages=3,
                accent=(60, 60, 60),
                publisher="Dark Horse",
                writer="Kentaro Miura",
                penciller="Kentaro Miura",
                age_rating="Mature 17+",
                manga="YesAndRightToLeft",
                title="The Black Swordsman",
            ),
        ),
    ]

    print(f"Writing {len(fixtures)} fixtures under {LIBRARY.relative_to(ROOT.parent)}/")
    for path, iss in fixtures:
        write_cbz(path, iss)
    print("done.")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--scale",
        choices=["dev", "stress"],
        default="dev",
        help="dev = small curated set; stress = ~1000 CBZs for perf profiling",
    )
    args = parser.parse_args()
    if args.scale == "stress":
        build_stress()
    else:
        build_dev()


if __name__ == "__main__":
    main()
