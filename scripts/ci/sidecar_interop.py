#!/usr/bin/env python3
"""Sidecar interop check (audit 2026-11 WP A5).

Reads the CBZ that `crates/server/tests/sidecar_interop_emit.rs` wrote with
the two tools other people open Folio's archives with — ComicTagger
(`comicapi`, the ComicInfo.xml reader) and Metron-Tagger (`darkseid`, the
MetronInfo.xml *and* ComicInfo.xml reader) — and diffs what each tool reads
against `expected.json`, the values Folio meant to write. Three tool
readings are compared: comicapi on ComicInfo.xml, darkseid on ComicInfo.xml
and darkseid on MetronInfo.xml; a fourth, stdlib view of the raw
ComicInfo.xml checks the elements neither tool reads (`<GTIN>`,
`<StoryArcNumber>`). darkseid also validates MetronInfo.xml against
its bundled MetronInfo v1.1 / v1.2 schemas with an XSD 1.1 validator (the
`<xs:assert>`s xmllint cannot check) and must report v1.2.

Every place a tool's reading legitimately differs from Folio's intent is a
*named* normalisation (`TOLERATED`, applied by `tolerate()`), printed when it
fires so the job log shows exactly what the comparison forgave; the same
list is documented under "Interop check" in
docs/dev/metadata-sidecar-writeback.md. Anything else is a mismatch and the
exit status is 1.

Usage (stdlib + the pinned tools from sidecar-interop-requirements.txt):

    python3 -I scripts/ci/sidecar_interop.py --dir "$FOLIO_INTEROP_OUT"

`--dir` holds `issue.cbz` and `expected.json` (the producer also drops
`ComicInfo.xml` / `MetronInfo.xml` beside them for the CI artifact). The
script refuses to run against tool versions other than the pinned ones, so
a resolver drift can't silently move the goalposts.
"""

from __future__ import annotations

import argparse
import importlib.metadata
import json
import re
import sys
from collections.abc import Callable
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
REQUIREMENTS = HERE / "sidecar-interop-requirements.txt"

# Every known way a tool's reading is allowed to differ from Folio's intent.
# Keys are the names printed in the log and listed in the doc.
TOLERATED: dict[str, str] = {
    "cover_artist_role_is_cover": (
        "both tools read ComicInfo <CoverArtist> (and the MetronInfo role) as the "
        "role 'Cover'; compared as Folio's 'CoverArtist'"
    ),
    "comicinfo_csv_comma_name": (
        "a name containing a comma makes Folio join the ComicInfo CSV field with "
        "'; '; comicapi splits CSV fields on ',' only and darkseid splits "
        "credits on ';' and ',' (resources on ','), so such a name arrives as "
        "fragments; compared against the tool's own split of Folio's join "
        "(known gap: ComicInfo has no lossless spelling for 'Name, Jr.')"
    ),
    "issue_number_leading_zeros": (
        "darkseid passes <Number> through IssueString, which drops leading "
        "zeros ('003' -> '3'); compared after the same normalisation"
    ),
    "gtin_as_int": (
        "darkseid parses <GTIN><UPC>/<ISBN> as integers, so a leading zero is "
        "lost; compared as integers"
    ),
    "ids_numeric_as_int": (
        "darkseid parses an all-digit <ID> / <Series id> as an integer, so a "
        "leading zero is lost; compared after the same integer round-trip"
    ),
}


# ───────── pins ─────────


def pinned_versions() -> dict[str, str]:
    pins: dict[str, str] = {}
    for raw in REQUIREMENTS.read_text(encoding="utf-8").splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        name, _, version = line.partition("==")
        if not version:
            sys.exit(f"{REQUIREMENTS.name}: expected name==version, got {raw!r}")
        pins[name.strip().lower()] = version.strip()
    return pins


def check_versions() -> dict[str, str]:
    pins = pinned_versions()
    found: dict[str, str] = {}
    for name, want in pins.items():
        try:
            got = importlib.metadata.version(name)
        except importlib.metadata.PackageNotFoundError:
            sys.exit(f"{name} is not installed; pip install -r {REQUIREMENTS}")
        if got != want:
            sys.exit(f"{name}=={got} installed, {REQUIREMENTS.name} pins {want}")
        found[name] = got
    return found


# ───────── report ─────────


class Report:
    def __init__(self) -> None:
        self.mismatches: list[str] = []
        self.tolerated: list[str] = []
        self.compared = 0

    def check(self, view: str, field: str, expected: Any, got: Any) -> None:
        self.compared += 1
        if expected != got:
            self.mismatches.append(
                f"{view}: {field}\n    expected: {expected!r}\n    read:     {got!r}"
            )

    def tolerate(self, name: str, view: str, field: str, detail: str) -> None:
        assert name in TOLERATED, name
        self.tolerated.append(f"{name} ({view}: {field}): {detail}")


# ───────── shared normalisers ─────────

# ComicInfo role labels as Folio writes them (`<CoverArtist>`) from the
# label both tools report ('Cover').
ROLE_FROM_TOOL = {"Cover": "CoverArtist"}

# MetronInfo `informationSource` names -> Folio `external_ids.source` keys.
ID_SOURCE_KEYS = {
    "Comic Vine": "comicvine",
    "Metron": "metron",
    "Grand Comics Database": "gcd",
    "Marvel": "marvel",
    "League of Comic Geeks": "locg",
    "MyAnimeList": "mal",
    "AniList": "anilist",
    "MangaUpdates": "mangaupdates",
}


def folio_csv_join(names: list[str]) -> str:
    """`sidecar_compose::join_unambiguous_csv`: '; ' once any name has a comma."""
    sep = "; " if any("," in n for n in names) else ", "
    return sep.join(names)


def strip_leading_zeros(number: str) -> str:
    return number.lstrip("0") or "0"


def credits_from_roles(credits: list[tuple[str, str]]) -> dict[str, list[str]]:
    """(role, person) pairs -> {folio role: sorted names}."""
    out: dict[str, list[str]] = {}
    for role, person in credits:
        out.setdefault(ROLE_FROM_TOOL.get(role, role), []).append(person)
    return {role: sorted(names) for role, names in out.items()}


def names_as_tool_reads(
    report: Report,
    view: str,
    field: str,
    names: list[str],
    tool_split: Callable[[str], list[str]],
) -> list[str]:
    """The names a ComicInfo CSV reader gets back for Folio's list. When a
    name carries a comma the tool cannot read Folio's join back as the same
    names; the expectation becomes the tool's own split of Folio's join
    (named normalisation). Otherwise the list is returned unchanged."""
    if not any("," in n for n in names):
        return names
    joined = folio_csv_join(names)
    as_tool_reads = tool_split(joined)
    report.tolerate(
        "comicinfo_csv_comma_name",
        view,
        field,
        f"Folio wrote {joined!r}; the tool reads {as_tool_reads!r}",
    )
    return as_tool_reads


def expect_csv_list(
    report: Report,
    view: str,
    field: str,
    expected_names: list[str],
    read_names: list[str],
    tool_split: Callable[[str], list[str]],
) -> None:
    want = names_as_tool_reads(report, view, field, expected_names, tool_split)
    report.check(view, field, sorted(want), sorted(read_names))


def expect_credits(
    report: Report,
    view: str,
    expected: dict[str, list[str]],
    read: dict[str, list[str]],
    tool_split: Callable[[str], list[str]] | None,
) -> None:
    """`tool_split` is the reader's ComicInfo CSV rule; `None` for a MetronInfo
    reading, where every `<Creator>` arrives whole."""
    if "CoverArtist" in expected:
        report.tolerate(
            "cover_artist_role_is_cover", view, "credits", "role 'Cover' compared as 'CoverArtist'"
        )
    want: dict[str, list[str]] = {}
    for role, names in expected.items():
        if tool_split is not None:
            names = names_as_tool_reads(report, view, f"credits[{role}]", names, tool_split)
        want[role] = sorted(names)
    report.check(view, "credits", want, read)


def expect_number(report: Report, view: str, expected: str, read: Any) -> None:
    """darkseid passes `<Number>` through IssueString (leading zeros dropped)."""
    want = strip_leading_zeros(expected)
    if want != expected:
        report.tolerate(
            "issue_number_leading_zeros", view, "number", f"{expected!r} compared as {want!r}"
        )
    report.check(view, "number", want, read)


def numeric_id_as_tool_reads(report: Report, view: str, field: str, value: str) -> str:
    """darkseid turns an all-digit id into an int, which `str()` hands back
    without leading zeros; normalise Folio's side the same way."""
    if not value.isdigit():
        return value
    want = str(int(value))
    report.tolerate(
        "ids_numeric_as_int",
        view,
        field,
        f"{value!r} compared as {want!r}" if want != value else "compared as a string",
    )
    return want


# ───────── reader 1: ComicTagger's comicapi on ComicInfo.xml ─────────


def read_comicapi(cbz: Path, expected: dict[str, Any], report: Report) -> None:
    from comicapi import utils
    from comicapi.comicarchive import ComicArchive

    view = "comicapi/ComicInfo"
    ca = ComicArchive(cbz)
    assert "cr" in ca.get_supported_tags(), ca.get_supported_tags()
    md = ca.read_tags("cr")
    if md.is_empty:
        report.mismatches.append(f"{view}: ComicTagger read no ComicInfo.xml")
        return

    def split(text: str) -> list[str]:
        # comicapi's own CSV splitter: ',' only, stripped.
        return [s.strip() for s in utils.split(text, ",")]

    report.check(view, "series", expected["series"], md.series)
    report.check(view, "number", expected["number"], md.issue)
    report.check(view, "title", expected["title"], md.title)
    report.check(view, "series_issue_count", expected["series_issue_count"], md.issue_count)
    report.check(view, "publisher", expected["publisher"], md.publisher)
    report.check(view, "imprint", expected["imprint"], md.imprint)
    cover = (
        f"{md.year:04d}-{md.month:02d}-{md.day:02d}"
        if md.year and md.month and md.day
        else (md.year, md.month, md.day)
    )
    report.check(view, "cover_date", expected["cover_date"], cover)
    report.check(view, "language", expected["language"], md.language)
    report.check(view, "summary", expected["summary"], md.description)
    report.check(view, "notes", expected["notes"], md.notes)
    report.check(view, "page_count", expected["page_count"], md.page_count)
    report.check(view, "age_rating", expected["age_rating"], md.maturity_rating)
    # comicapi parses <Web> into Url objects; compare the string form.
    report.check(view, "web", expected["web"], md.web_links[0].url if md.web_links else None)
    for field in ("characters", "teams", "locations", "genres"):
        expect_csv_list(report, view, field, expected[field], sorted(getattr(md, field)), split)
    # ComicInfo <StoryArc> is names only (comicapi does not read StoryArcNumber).
    expect_csv_list(
        report,
        view,
        "story_arcs",
        [a["name"] for a in expected["story_arcs"]],
        md.story_arcs,
        split,
    )
    expect_credits(
        report,
        view,
        expected["credits"],
        credits_from_roles([(c.role, c.person) for c in md.credits]),
        split,
    )
    # Not read by comicapi 1.6 from ComicInfo.xml: Tags, GTIN, the
    # <ComicVineID>/<MetronID> extension elements, StoryArcNumber,
    # Translator, universes, reprints. GTIN and StoryArcNumber are checked
    # on the raw XML instead (`read_comicinfo_xml`).


# ───────── reader 4: the raw ComicInfo.xml (stdlib) ─────────


def folio_arc_numbers(arcs: list[dict[str, Any]]) -> str | None:
    """`sidecar_compose::compose_comicinfo_arcs`: the numbers parallel to
    `<StoryArc>`, same separator, an unnumbered arc's slot left empty,
    trailing empty slots dropped, no element when no arc has a number."""
    slots = ["" if a["number"] is None else str(a["number"]) for a in arcs]
    while slots and not slots[-1]:
        slots.pop()
    if not slots:
        return None
    sep = "; " if any("," in a["name"] for a in arcs) else ", "
    return sep.join(slots)


def read_comicinfo_xml(cbz: Path, expected: dict[str, Any], report: Report) -> None:
    """The ComicInfo elements neither tool reads but ComicInfo-only readers
    (ComicRack, Kavita, Komga) do: `<GTIN>` (one barcode, the precedence
    Folio chose) and `<StoryArcNumber>` as a list parallel to `<StoryArc>`.
    Compared as the exact element text, so order and separator count."""
    import xml.etree.ElementTree as ET
    import zipfile

    view = "xml/ComicInfo"
    with zipfile.ZipFile(cbz) as zf:
        root = ET.fromstring(zf.read("ComicInfo.xml"))

    def text(tag: str) -> str | None:
        el = root.find(tag)
        return el.text if el is not None else None

    report.check(view, "gtin", expected["comicinfo_gtin"], text("GTIN"))
    arcs = expected["story_arcs"]
    report.check(
        view, "story_arc", folio_csv_join([a["name"] for a in arcs]) or None, text("StoryArc")
    )
    report.check(view, "story_arc_number", folio_arc_numbers(arcs), text("StoryArcNumber"))


# ───────── reader 2 + 3: Metron-Tagger's darkseid ─────────


def darkseid_view(cbz: Path, fmt: Any) -> Any:
    """A fresh `Comic` per format: darkseid caches one parsed metadata
    object per `Comic`, whichever format was read first."""
    from darkseid.comic import Comic

    return Comic(cbz).read_metadata(fmt)


def basic_names(items: Any) -> list[str]:
    return sorted(b.name for b in (items or []))


def darkseid_credits(credits: Any) -> dict[str, list[str]]:
    pairs = [(role.name, c.person) for c in (credits or []) for role in c.role]
    return credits_from_roles(pairs)


def read_darkseid_comicinfo(cbz: Path, expected: dict[str, Any], report: Report) -> None:
    from darkseid.comic import MetadataFormat

    view = "darkseid/ComicInfo"
    md = darkseid_view(cbz, MetadataFormat.COMIC_INFO)
    if md.is_empty:
        report.mismatches.append(f"{view}: darkseid read no ComicInfo.xml")
        return

    def split_resources(text: str) -> list[str]:
        # darkseid `_string_to_resource`: ',' (and double-quoted runs).
        return [s.strip() for s in re.split(r',|"(.*?)"', text) if s and s.strip()]

    def split_credits(text: str) -> list[str]:
        # darkseid splits ComicInfo credit columns on ';' and ','.
        return [s.strip() for s in re.split(r"[;,]", text) if s.strip()]

    report.check(view, "series", expected["series"], md.series.name)
    expect_number(report, view, expected["number"], md.issue)
    report.check(
        view, "title", expected["title"], basic_names(md.stories)[0] if md.stories else None
    )
    report.check(view, "series_issue_count", expected["series_issue_count"], md.series.issue_count)
    report.check(
        view, "publisher", expected["publisher"], md.publisher.name if md.publisher else None
    )
    report.check(
        view,
        "imprint",
        expected["imprint"],
        md.publisher.imprint.name if md.publisher and md.publisher.imprint else None,
    )
    report.check(
        view,
        "cover_date",
        expected["cover_date"],
        md.cover_date.isoformat() if md.cover_date else None,
    )
    report.check(view, "language", expected["language"], md.series.language)
    report.check(view, "summary", expected["summary"], md.comments)
    report.check(view, "notes", expected["notes"], md.notes.comic_rack if md.notes else None)
    report.check(view, "page_count", expected["page_count"], md.page_count)
    report.check(
        view,
        "age_rating",
        expected["age_rating"],
        md.age_rating.comic_rack if md.age_rating else None,
    )
    report.check(view, "web", expected["web"], md.web_link[0].url if md.web_link else None)
    for field in ("characters", "teams", "locations", "genres"):
        expect_csv_list(
            report, view, field, expected[field], basic_names(getattr(md, field)), split_resources
        )
    expect_csv_list(
        report,
        view,
        "story_arcs",
        [a["name"] for a in expected["story_arcs"]],
        sorted(a.name for a in md.story_arcs),
        split_resources,
    )
    expect_credits(report, view, expected["credits"], darkseid_credits(md.credits), split_credits)
    # Not read by darkseid from ComicInfo.xml: Tags, GTIN, the extension
    # id elements, StoryArcNumber, Translator, universes, reprints (GTIN and
    # StoryArcNumber: see `read_comicinfo_xml`).


def read_darkseid_metroninfo(cbz: Path, expected: dict[str, Any], report: Report) -> None:
    from darkseid.comic import Comic, MetadataFormat

    view = "darkseid/MetronInfo"
    schema = str(Comic(cbz).validate_metadata(MetadataFormat.METRON_INFO))
    report.check(view, "schema (darkseid MetronInfo XSD validation)", "Metron Info V1.2", schema)
    md = darkseid_view(cbz, MetadataFormat.METRON_INFO)
    if md.is_empty:
        report.mismatches.append(f"{view}: darkseid read no MetronInfo.xml")
        return

    report.check(view, "series", expected["series"], md.series.name)
    report.check(view, "series_sort_name", expected["series_sort_name"], md.series.sort_name)
    report.check(view, "series_start_year", expected["series_start_year"], md.series.start_year)
    report.check(view, "series_end_year", expected["series_end_year"], md.series.end_year)
    report.check(view, "series_issue_count", expected["series_issue_count"], md.series.issue_count)
    report.check(view, "series_status_metron", expected["series_status_metron"], md.series.status)
    report.check(view, "series_format_metron", expected["series_format_metron"], md.series.format)
    report.check(view, "language", expected["language"], md.series.language)
    report.check(
        view,
        "series_metron_id",
        numeric_id_as_tool_reads(report, view, "series_metron_id", expected["series_metron_id"]),
        str(md.series.id_) if md.series.id_ is not None else None,
    )
    expect_number(report, view, expected["number"], md.issue)
    report.check(
        view, "title", expected["title"], basic_names(md.stories)[0] if md.stories else None
    )
    report.check(
        view, "publisher", expected["publisher"], md.publisher.name if md.publisher else None
    )
    report.check(
        view,
        "imprint",
        expected["imprint"],
        md.publisher.imprint.name if md.publisher and md.publisher.imprint else None,
    )
    report.check(
        view,
        "cover_date",
        expected["cover_date"],
        md.cover_date.isoformat() if md.cover_date else None,
    )
    report.check(
        view,
        "store_date",
        expected["store_date"],
        md.store_date.isoformat() if md.store_date else None,
    )
    report.check(view, "summary", expected["summary"], md.comments)
    report.check(view, "notes", expected["notes"], md.notes.metron_info if md.notes else None)
    report.check(view, "page_count", expected["page_count"], md.page_count)
    report.check(
        view,
        "age_rating_metron",
        expected["age_rating_metron"],
        md.age_rating.metron_info if md.age_rating else None,
    )
    for field in ("characters", "teams", "locations", "genres", "tags", "reprints"):
        report.check(view, field, sorted(expected[field]), basic_names(getattr(md, field)))
    report.check(
        view, "universes", sorted(expected["universes"]), sorted(u.name for u in md.universes)
    )
    report.check(
        view,
        "story_arcs",
        sorted((a["name"], a["number"]) for a in expected["story_arcs"]),
        sorted((a.name, a.number) for a in md.story_arcs),
    )
    expect_credits(report, view, expected["credits"], darkseid_credits(md.credits), None)
    # <IDS>: every source, its id and which one is primary.
    want_ids = {
        source: numeric_id_as_tool_reads(report, view, f"ids[{source}]", value)
        for source, value in expected["ids"].items()
    }
    ids = {ID_SOURCE_KEYS.get(s.name, s.name): str(s.id_) for s in (md.info_source or [])}
    report.check(view, "ids", want_ids, ids)
    primary = [ID_SOURCE_KEYS.get(s.name, s.name) for s in (md.info_source or []) if s.primary]
    report.check(view, "primary_id_source", [expected["primary_id_source"]], primary)
    # <GTIN>: darkseid hands back integers and drops a value it cannot parse
    # as one (an ISBN-10 check digit `X`, hyphens), so a non-digit expectation
    # is compared as "absent".
    want_gtin: dict[str, int] = {}
    for kind, value in expected["gtin"].items():
        if value.isdigit():
            want_gtin[kind] = int(value)
            if str(int(value)) != value:
                report.tolerate(
                    "gtin_as_int", view, f"gtin[{kind}]", f"{value!r} compared as {int(value)}"
                )
        else:
            report.mismatches.append(
                f"{view}: gtin[{kind}]\n    expected: {value!r}\n"
                "    read:     darkseid only reads all-digit GTINs"
            )
    got_gtin = {"upc": md.gtin.upc, "isbn": md.gtin.isbn} if md.gtin else {}
    report.check(view, "gtin", want_gtin, {k: v for k, v in got_gtin.items() if v is not None})
    # <URLs>: order kept, the primary flagged.
    report.check(view, "urls", expected["urls"], [u.url for u in (md.web_link or [])])
    report.check(view, "web", [expected["web"]], [u.url for u in (md.web_link or []) if u.primary])


# ───────── main ─────────


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument(
        "--dir",
        required=True,
        type=Path,
        help="directory holding issue.cbz + expected.json (FOLIO_INTEROP_OUT of the producer)",
    )
    args = ap.parse_args()
    # Absolute: cargo runs the producer with cwd = crates/server, so a
    # relative path in the hint below would land the fixture elsewhere.
    out_dir = args.dir.resolve()
    cbz = out_dir / "issue.cbz"
    expected_path = out_dir / "expected.json"
    for p in (cbz, expected_path):
        if not p.is_file():
            sys.exit(
                f"{p} missing; run the producer first:\n"
                f"  FOLIO_INTEROP_OUT={out_dir} cargo test -p server --test sidecar_interop_emit"
            )

    versions = check_versions()
    print("readers: " + ", ".join(f"{k}=={v}" for k, v in versions.items()))
    expected = json.loads(expected_path.read_text(encoding="utf-8"))

    report = Report()
    read_comicapi(cbz, expected, report)
    read_darkseid_comicinfo(cbz, expected, report)
    read_darkseid_metroninfo(cbz, expected, report)
    read_comicinfo_xml(cbz, expected, report)

    # darkseid bundles ComicInfo v1/v2 XSDs only; Folio writes the v2.1
    # draft vocabulary (<Tags>, <Translator>, <StoryArcNumber>, <GTIN>) and
    # the de-facto <ComicVineID>/<MetronID> extension elements, so this
    # one is informational (darkseid logs "did not validate" first).
    from darkseid.comic import Comic, MetadataFormat

    print(
        "darkseid ComicInfo schema check (informational: it ships the v1/v2 XSDs, "
        "Folio writes the v2.1 draft vocabulary + extension elements):",
        flush=True,
    )
    print(f"  {Comic(cbz).validate_metadata(MetadataFormat.COMIC_INFO)}")

    print(f"\ncompared {report.compared} field readings across 4 views")
    if report.tolerated:
        print(f"\ntolerated normalisations ({len(report.tolerated)}):")
        for line in sorted(set(report.tolerated)):
            print(f"  - {line}")
    # Everything goes to stdout so a `| tee report.txt` in CI keeps the
    # whole picture in one file, in order.
    if report.mismatches:
        print(f"\nMISMATCHES ({len(report.mismatches)}):")
        for m in report.mismatches:
            print(f"  {m}")
        print(
            "\nA real difference between what Folio wrote and what the tool reads. Fix the "
            "composer/serializer, or add a *named* entry to TOLERATED in this script and the "
            'doc\'s "Interop check" list.'
        )
        return 1
    print("\nOK: every tool reading agrees with expected.json")
    return 0


if __name__ == "__main__":
    sys.exit(main())
