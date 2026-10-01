#!/usr/bin/env python3
"""ComicTagger parity fixture (roadmap WP-6.4).

Builds a small synthetic CBZ whose pages are real (tiny) PNG / JPEG
images, then has **ComicTagger itself** write the ComicInfo.xml into it,
offline, from explicit `-m` metadata overrides. The result is committed as
`ct-1.5.5-tagged.cbz` and read by
`crates/server/tests/sidecar_parity.rs`, which scans it into Folio, runs
the writeback rewrite and diffs Folio's ComicInfo.xml against
ComicTagger's field by field. ComicTagger is never needed at test time.

Run (ComicTagger 1.5.5 is the latest stable release on PyPI; 1.6.x is
still beta):

    uv run --no-project --with comictagger==1.5.5 \\
        python fixtures/comictagger/make-fixture.py build

    # Reverse direction: does ComicTagger read Folio's rewrite without
    # loss? Reads the golden Folio XML the parity test checks in
    # (`folio-rewrite.ComicInfo.xml`) with ComicTagger's own parser and
    # compares the result against ComicTagger's reading of its own file.
    uv run --no-project --with comictagger==1.5.5 \\
        python fixtures/comictagger/make-fixture.py verify

No online lookups: no `-o`, no API key, and ComicTagger runs against a
throwaway config dir (`--config`) and HOME so a developer's
~/.ComicTagger / ~/.config/ComicTagger settings are never read.
"""

from __future__ import annotations

import io
import os
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
CT_VERSION = "1.5.5"
FIXTURE = HERE / f"ct-{CT_VERSION}-tagged.cbz"
FOLIO_GOLDEN = HERE / "folio-rewrite.ComicInfo.xml"

# Deterministic zip timestamps for the *base* archive (ComicTagger stamps
# its own ComicInfo.xml entry with the wall clock when it appends it).
ZIP_DATE = (2026, 1, 1, 0, 0, 0)

# Every ComicInfo field ComicTagger 1.5.5's CIX writer emits
# (`comicapi/comicinfoxml.py::convert_metadata_to_xml`). Keys are
# ComicTagger `GenericMetadata` attribute names; `credit=Role:Person`
# adds a credit. Commas / equals signs inside a value are escaped with
# `^` (ComicTagger's own escape character) by `metadata_arg`. Values
# deliberately include XML specials, quotes, non-ASCII text and a
# ComicVine web link.
METADATA: list[tuple[str, str]] = [
    ("series", "The Parity Patrol"),
    ("issue", "3"),
    ("title", "Who's Afraid of <Diffs> & Drift?"),
    ("volume", "2021"),
    ("issue_count", "12"),
    ("publisher", "Folio Test Comics"),
    ("imprint", "Fixture Press"),
    ("year", "2021"),
    ("month", "7"),
    ("day", "14"),
    ("alternate_series", "Patrol Team-Up"),
    ("alternate_number", "1"),
    ("alternate_count", "4"),
    ("story_arc", "The Round Trip"),
    ("series_group", "Patrol Family"),
    (
        "comments",
        'The patrol rides again. "Quoted" text, an ampersand & angle <brackets>;'
        " café, naïve, 日本語.",
    ),
    ("notes", "Fixture tagged offline by ComicTagger for Folio WP-6.4."),
    ("genre", "Superhero, Comedy"),
    ("web_link", "https://comicvine.gamespot.com/the-parity-patrol-3/4000-123456/"),
    ("language", "en"),
    ("format", "Series"),
    ("maturity_rating", "Teen"),
    ("critical_rating", "4.5"),
    ("black_and_white", "True"),
    ("manga", "No"),
    ("characters", "Captain Parity, Diff Kid, The Rewriter"),
    ("teams", "Parity Patrol, Legion of Lint"),
    ("locations", "Fixture City, The Archive"),
    ("scan_info", "Synthetic-Folio"),
    ("credit", "Writer:Wanda Writer"),
    ("credit", "Writer:Second Scribe"),
    ("credit", "Penciller:Pat Penciller"),
    ("credit", "Inker:Ian Inker"),
    ("credit", "Colorist:Cora Colorist"),
    ("credit", "Letterer:Lee Letterer"),
    ("credit", "Cover:Cass Cover"),
    ("credit", "Editor:Eddie Editor"),
]


def metadata_arg() -> str:
    def esc(v: str) -> str:
        return v.replace("=", "^=").replace(",", "^,")

    return ", ".join(f"{k}={esc(v)}" for k, v in METADATA)


def page_images() -> list[tuple[str, bytes]]:
    """Four tiny pages with real image bytes: a PNG cover, two JPEG
    interiors and a PNG double-page spread (twice as wide), so
    ComicTagger's page sniffing records real sizes."""
    from PIL import Image

    def render(fmt: str, size: tuple[int, int], color: tuple[int, int, int]) -> bytes:
        im = Image.new("RGB", size, color)
        buf = io.BytesIO()
        if fmt == "PNG":
            im.save(buf, "PNG", optimize=True)
        else:
            im.save(buf, "JPEG", quality=60)
        return buf.getvalue()

    return [
        ("page-001.png", render("PNG", (40, 60), (200, 40, 40))),
        ("page-002.jpg", render("JPEG", (40, 60), (40, 160, 40))),
        ("page-003.png", render("PNG", (80, 60), (40, 40, 200))),
        ("page-004.jpg", render("JPEG", (40, 60), (220, 200, 40))),
    ]


def write_base_cbz(path: Path) -> None:
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_STORED) as zf:
        for name, data in page_images():
            info = zipfile.ZipInfo(name, date_time=ZIP_DATE)
            zf.writestr(info, data)


def run_comictagger(args: list[str], home: Path) -> str:
    cfg = home / "ct-config"
    cfg.mkdir(exist_ok=True)
    env = {**os.environ, "HOME": str(home), "XDG_CONFIG_HOME": str(home / ".config")}
    cmd = [
        sys.executable,
        "-c",
        "import sys; from comictaggerlib.main import ctmain; sys.exit(ctmain())",
        "--config",
        str(cfg),
        *args,
    ]
    res = subprocess.run(cmd, env=env, capture_output=True, text=True, check=False)
    if res.returncode != 0:
        sys.stderr.write(res.stdout + res.stderr)
        raise SystemExit(f"comictagger failed: {cmd}")
    return res.stdout


def edit_page_list(path: Path) -> None:
    """Second ComicTagger pass: the page-list edits its GUI page editor
    makes (`comictaggerlib/pagelisteditor.py`), applied through the same
    `comicapi` objects and saved with ComicTagger's own CIX writer
    (`ComicArchive.write_cix`). The CLI's `-m` can't reach `<Pages>`, and
    the GUI needs Qt, so this is the headless equivalent: it marks the
    80x60 spread `DoublePage` (ComicTagger stores the Python bool, which
    serializes as "True"), sets page types and a bookmark.
    """
    from comicapi.comicarchive import ComicArchive

    ca = ComicArchive(path)
    md = ca.read_cix()
    md.pages[1]["Type"] = "Story"
    md.pages[1]["Bookmark"] = "The Round Trip"
    md.pages[2]["DoublePage"] = True
    md.pages[3]["Type"] = "BackCover"
    if not ca.write_cix(md):
        raise SystemExit("ComicTagger failed to write the page list")


def check_version() -> None:
    from importlib.metadata import version

    got = version("comictagger")
    if got != CT_VERSION:
        raise SystemExit(f"expected comictagger=={CT_VERSION}, found {got}")


def build() -> None:
    check_version()
    with tempfile.TemporaryDirectory() as tmp:
        home = Path(tmp)
        work = home / FIXTURE.name
        write_base_cbz(work)
        run_comictagger(["-s", "-t", "cr", "-m", metadata_arg(), str(work)], home)
        edit_page_list(work)
        shutil.copyfile(work, FIXTURE)
        print(run_comictagger(["-p", "-t", "cr", "--raw", str(FIXTURE)], home))
    print(f"wrote {FIXTURE.relative_to(HERE.parent.parent)} ({FIXTURE.stat().st_size} bytes)")


def ct_read(xml: bytes) -> dict:
    """ComicTagger's own reading of a ComicInfo.xml, as a plain dict."""
    from comicapi.comicinfoxml import ComicInfoXml

    md = ComicInfoXml().metadata_from_string(xml.decode("utf-8"))
    out = {
        k: v
        for k, v in vars(md).items()
        if k not in ("is_empty", "tag_origin", "issue_id") and v not in (None, "", [], set())
    }
    out["credits"] = sorted((c["role"], c["person"]) for c in md.credits)
    # Page attributes as ComicTagger holds them, with `DoublePage=False`
    # folded into "absent" (the ComicInfo default). Folio writes an
    # explicit `DoublePage="false"` on every non-spread page; ComicTagger
    # reads it as False — equal in value. (Its 1.5.5 GUI page editor ticks
    # the checkbox on attribute *presence*, a known cosmetic quirk listed
    # in docs/dev/metadata-sidecar-writeback.md.)
    out["pages"] = [
        {k: str(v) for k, v in p.items() if not (k == "DoublePage" and v is False)}
        for p in md.pages
    ]
    return out


def verify() -> None:
    check_version()
    if not FOLIO_GOLDEN.exists():
        raise SystemExit(
            f"{FOLIO_GOLDEN.name} missing - run the parity test with "
            "FOLIO_PARITY_BLESS=1 first"
        )
    with zipfile.ZipFile(FIXTURE) as zf:
        ct_xml = zf.read("ComicInfo.xml")
    ct = ct_read(ct_xml)
    folio = ct_read(FOLIO_GOLDEN.read_bytes())
    diffs = sorted(k for k in set(ct) | set(folio) if ct.get(k) != folio.get(k))
    for k in diffs:
        print(f"{k}:\n  comictagger: {ct.get(k)!r}\n  folio:       {folio.get(k)!r}")
    print(f"{len(diffs)} field(s) differ when ComicTagger {CT_VERSION} reads Folio's rewrite")


if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else "build"
    {"build": build, "verify": verify}[cmd]()
