#!/usr/bin/env python3
"""Move CHANGELOG.md's Unreleased notes into a dated release section."""

from __future__ import annotations

import argparse
import datetime as dt
import re
from pathlib import Path


VERSION_RE = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.]+)?$")
HEADING_RE = re.compile(r"^## \[(?P<name>[^\]]+)\](?: - (?P<date>[0-9]{4}-[0-9]{2}-[0-9]{2}))?\s*$", re.M)
LINK_RE = re.compile(r"^\[[^\]]+\]: .+$", re.M)
LINK_NAME_RE = re.compile(r"^\[([^\]]+)\]: ")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version", help="Semver version without a leading v, e.g. 0.9.4")
    parser.add_argument(
        "--date",
        default=dt.date.today().isoformat(),
        help="Release date for the changelog heading, default: today",
    )
    parser.add_argument(
        "--previous-tag",
        required=True,
        help="Previous release tag, e.g. v0.9.3",
    )
    parser.add_argument(
        "--path",
        default="CHANGELOG.md",
        help="Path to CHANGELOG.md, default: CHANGELOG.md",
    )
    return parser.parse_args()


def split_links(text: str) -> tuple[str, str]:
    matches = list(LINK_RE.finditer(text))
    if not matches:
        raise SystemExit("CHANGELOG.md has no compare-link block")

    start = matches[0].start()
    body = text[:start].rstrip()
    links = text[start:].strip()
    return body, links


def find_heading(headings: list[re.Match[str]], name: str) -> re.Match[str] | None:
    return next((heading for heading in headings if heading.group("name") == name), None)


def upsert_compare_links(links: str, version: str, previous_tag: str, tag: str) -> str:
    lines = links.splitlines()
    urls = {
        "Unreleased": f"https://github.com/mbryantms/folio/compare/{tag}...HEAD",
        version: f"https://github.com/mbryantms/folio/compare/{previous_tag}...{tag}",
    }

    names: list[str] = []
    for line in lines:
        match = LINK_NAME_RE.match(line)
        if match:
            names.append(match.group(1))

    output: list[str] = []
    inserted_version = False
    for line in lines:
        match = LINK_NAME_RE.match(line)
        if not match:
            output.append(line)
            continue

        name = match.group(1)
        if name in urls:
            output.append(f"[{name}]: {urls[name]}")
        else:
            output.append(line)

        if name == "Unreleased" and version not in names:
            output.append(f"[{version}]: {urls[version]}")
            inserted_version = True

    if version not in names and not inserted_version:
        output.append(f"[{version}]: {urls[version]}")

    return "\n".join(output)


def main() -> None:
    args = parse_args()
    version = args.version.removeprefix("v")
    tag = f"v{version}"
    previous_tag = args.previous_tag

    if not VERSION_RE.match(version):
        raise SystemExit(f"version must look like 1.2.3, got {args.version!r}")
    if not re.match(r"^v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.]+)?$", previous_tag):
        raise SystemExit(f"previous tag must look like v1.2.3, got {previous_tag!r}")
    if tag == previous_tag:
        raise SystemExit("new tag and previous tag are identical")
    try:
        dt.date.fromisoformat(args.date)
    except ValueError as exc:
        raise SystemExit(f"release date must be YYYY-MM-DD, got {args.date!r}") from exc

    path = Path(args.path)
    text = path.read_text(encoding="utf-8")
    body, links = split_links(text)
    headings = list(HEADING_RE.finditer(body))

    unreleased = find_heading(headings, "Unreleased")
    if unreleased is None:
        raise SystemExit("CHANGELOG.md has no '## [Unreleased]' section")

    existing = find_heading(headings, version)
    if existing is not None:
        if existing.group("date") is None:
            line_end = body.find("\n", existing.start())
            body = f"{body[:line_end]} - {args.date}{body[line_end:]}"
        links = upsert_compare_links(links, version, previous_tag, tag)
        path.write_text(f"{body.rstrip()}\n\n{links.strip()}\n", encoding="utf-8")
        return

    next_heading = next((heading for heading in headings if heading.start() > unreleased.start()), None)
    unreleased_content_start = unreleased.end()
    unreleased_content_end = next_heading.start() if next_heading else len(body)
    unreleased_content = body[unreleased_content_start:unreleased_content_end].strip()

    if not unreleased_content:
        raise SystemExit("CHANGELOG.md Unreleased section is empty; add release notes first")

    before_unreleased = body[: unreleased.end()].rstrip()
    after_unreleased = body[unreleased_content_end:].lstrip()
    release_section = f"## [{version}] - {args.date}\n\n{unreleased_content}"
    body = f"{before_unreleased}\n\n{release_section}\n\n{after_unreleased}".rstrip()

    links = upsert_compare_links(links, version, previous_tag, tag)

    path.write_text(f"{body}\n\n{links.strip()}\n", encoding="utf-8")


if __name__ == "__main__":
    main()
