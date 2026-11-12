#!/usr/bin/env python3
"""Fetch or check the reference pages and candidate models for ocr-bench.

    python3 -I fetch.py pages  DEST [--only TEXT]... [--accept-reencode]
    python3 -I fetch.py models DEST [--only TEXT]...
    python3 -I fetch.py verify pages|models DIR [--only TEXT]...

`pages` downloads the 12 reference pages listed in `pages.json` into
`DEST/<book>/pNNN.jpg`; `models` downloads the model files listed in
`models.json` into `DEST/<org>__<repo>/<file>`. Both are the layouts
`ocr-bench --pages` / `--models` read. `verify` checks a directory that
already holds the files and downloads nothing.

Rules, the same for every file:

- DEST must not exist yet or must be empty; nothing is ever overwritten.
- Only https, on the first request and on every redirect.
- Each file is hashed while it streams into `<name>.part`; it is renamed
  into place only when its size and SHA-256 equal the manifest's, and
  deleted otherwise. A download is cut off once it passes the pinned size.
- Nothing downloaded is executed or imported.

Two pages are served as JPEG 2000 only. For those the download itself is
checked against its own pinned SHA-256, then converted with Pillow
(convert to RGB, JPEG quality 95) and the result is checked against the
page's pinned SHA-256. Another Pillow, OpenJPEG or libjpeg build may
encode different bytes; the page is then refused unless
`--accept-reencode` is given, which keeps it with a warning (its source
was verified, but benchmark figures for that page may differ slightly
from published ones).

`--only TEXT` (repeatable) limits the run to entries whose path contains
TEXT. Exit status: 0 when every selected file is in place and verified,
1 when any is missing or failed, 2 for a usage error or a refused DEST.
"""

import argparse
import hashlib
import io
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
USER_AGENT = "ocr-bench-fetch"
CHUNK = 1 << 20
TIMEOUT_S = 120
# Seconds between requests to the page host.
PAGE_PAUSE_S = 1.0
# The host every URL of a manifest must name.
HOSTS = {"pages": "archive.org", "models": "huggingface.co"}


class Refused(Exception):
    """A file that was not written, with the reason."""


class HttpsOnlyRedirects(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if urllib.parse.urlsplit(newurl).scheme != "https":
            raise Refused(f"redirect to a non-https address: {newurl}")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def open_url(url):
    """Open `url` for reading. https only, redirects included."""
    opener = urllib.request.build_opener(HttpsOnlyRedirects)
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    return opener.open(request, timeout=TIMEOUT_S)


def load_entries(kind):
    """The manifest as a list of {path, url, bytes, sha256, download?}."""
    if kind == "pages":
        raw = json.loads((HERE / "pages.json").read_text(encoding="utf-8"))
        entries = [
            {
                "path": p["file"],
                "url": p["url"],
                "bytes": p["bytes"],
                "sha256": p["sha256"],
                "download": p.get("download"),
            }
            for p in raw["pages"]
        ]
    else:
        raw = json.loads((HERE / "models.json").read_text(encoding="utf-8"))
        entries = [
            {
                "path": f["path"],
                "url": f["url"],
                "bytes": f["bytes"],
                "sha256": f["sha256"],
                "download": None,
            }
            for f in raw["files"]
        ]
    for e in entries:
        parts = urllib.parse.urlsplit(e["url"])
        if parts.scheme != "https" or parts.hostname != HOSTS[kind]:
            raise SystemExit(f"manifest error: {e['url']} is not https://{HOSTS[kind]}/")
        rel = Path(e["path"])
        if rel.is_absolute() or ".." in rel.parts:
            raise SystemExit(f"manifest error: unsafe path {e['path']}")
    return entries


def select(entries, only):
    if not only:
        return entries
    chosen = [e for e in entries if any(text in e["path"] for text in only)]
    if not chosen:
        raise SystemExit("--only matched no entry of the manifest")
    return chosen


def sha256_file(path):
    digest = hashlib.sha256()
    size = 0
    with open(path, "rb") as fh:
        while True:
            chunk = fh.read(CHUNK)
            if not chunk:
                break
            digest.update(chunk)
            size += len(chunk)
    return digest.hexdigest(), size


def prepare_dest(dest):
    """Create DEST, or refuse when it holds anything."""
    if dest.exists():
        if not dest.is_dir():
            raise SystemExit(f"{dest} exists and is not a directory")
        if any(dest.iterdir()):
            print(f"{dest} is not empty; give a new or empty directory", file=sys.stderr)
            raise SystemExit(2)
    dest.mkdir(parents=True, exist_ok=True)


def download(url, part, want_bytes, want_sha256):
    """Stream `url` into the new file `part`; raise unless size and digest match."""
    digest = hashlib.sha256()
    size = 0
    try:
        with open_url(url) as response, open(part, "xb") as out:
            while True:
                chunk = response.read(CHUNK)
                if not chunk:
                    break
                size += len(chunk)
                if size > want_bytes:
                    raise Refused(f"more than the pinned {want_bytes} bytes")
                digest.update(chunk)
                out.write(chunk)
        if size != want_bytes:
            raise Refused(f"{size} bytes, pinned {want_bytes}")
        if digest.hexdigest() != want_sha256:
            raise Refused(f"sha256 {digest.hexdigest()}, pinned {want_sha256}")
    except Refused:
        part.unlink(missing_ok=True)
        raise
    except (OSError, urllib.error.URLError) as err:
        part.unlink(missing_ok=True)
        raise Refused(f"download failed: {err}") from err


def reencode(part, entry, accept_reencode):
    """JPEG 2000 download -> the JPEG page. Returns (bytes, matched_pin)."""
    try:
        from PIL import Image
    except ImportError as err:
        raise Refused("this page is served as JPEG 2000 and needs Pillow to convert") from err
    try:
        with Image.open(part) as image:
            image.load()
            buffer = io.BytesIO()
            image.convert("RGB").save(buffer, "JPEG", quality=95)
    except Exception as err:  # Pillow raises several unrelated types here
        raise Refused(f"JPEG 2000 conversion failed: {err}") from err
    data = buffer.getvalue()
    matched = (
        len(data) == entry["bytes"] and hashlib.sha256(data).hexdigest() == entry["sha256"]
    )
    if not matched and not accept_reencode:
        raise Refused(
            "the converted JPEG differs from the pinned one (another Pillow, OpenJPEG or "
            "libjpeg build); the download itself was verified. Run again into a new "
            "directory with --accept-reencode to keep such a page"
        )
    return data, matched


def fetch_one(dest, entry, accept_reencode):
    """Download one entry into DEST. Returns True when it equals the pin."""
    final = dest / entry["path"]
    final.parent.mkdir(parents=True, exist_ok=True)
    part = final.with_name(final.name + ".part")
    source = entry["download"]
    if source is None:
        download(entry["url"], part, entry["bytes"], entry["sha256"])
        os.replace(part, final)
        return True
    download(entry["url"], part, source["bytes"], source["sha256"])
    try:
        data, matched = reencode(part, entry, accept_reencode)
    finally:
        part.unlink(missing_ok=True)
    with open(final, "xb") as out:
        out.write(data)
    return matched


def run_fetch(kind, dest, only, accept_reencode):
    entries = select(load_entries(kind), only)
    prepare_dest(dest)
    total = sum((e["download"] or e)["bytes"] for e in entries)
    print(f"{len(entries)} files, {total / 1e6:.1f} MB, from {HOSTS[kind]} into {dest}")
    failed, reencoded = [], []
    for index, entry in enumerate(entries):
        if kind == "pages" and index:
            time.sleep(PAGE_PAUSE_S)
        try:
            matched = fetch_one(dest, entry, accept_reencode)
        except Refused as reason:
            failed.append(entry["path"])
            print(f"FAILED  {entry['path']}: {reason}")
            continue
        if matched:
            print(f"ok      {entry['path']}")
        else:
            reencoded.append(entry["path"])
            print(f"WARNING {entry['path']}: kept a re-encode that differs from the pin")
    if reencoded:
        print(f"{len(reencoded)} page(s) differ from the pinned bytes: {', '.join(reencoded)}")
    if failed:
        print(f"{len(failed)} of {len(entries)} files were not written; {dest} is incomplete")
        return 1
    print("all files verified" if not reencoded else "all other files verified")
    return 0


def run_verify(kind, directory, only):
    if not directory.is_dir():
        raise SystemExit(f"{directory} is not a directory")
    entries = select(load_entries(kind), only)
    bad = 0
    for entry in entries:
        path = directory / entry["path"]
        if not path.is_file():
            bad += 1
            print(f"MISSING  {entry['path']}")
            continue
        digest, size = sha256_file(path)
        if size == entry["bytes"] and digest == entry["sha256"]:
            print(f"ok       {entry['path']}")
        else:
            bad += 1
            print(f"MISMATCH {entry['path']}: {size} bytes, sha256 {digest}")
    listed = {e["path"] for e in load_entries(kind)}
    extra = sorted(
        str(p.relative_to(directory))
        for p in directory.rglob("*")
        if p.is_file() and p.relative_to(directory).as_posix() not in listed
    )
    if extra:
        print(f"{len(extra)} file(s) in {directory} are not in the manifest (not checked)")
    print(f"{len(entries) - bad} of {len(entries)} listed files match")
    return 1 if bad else 0


def main(argv=None):
    parser = argparse.ArgumentParser(
        description="Fetch or check the ocr-bench reference pages and candidate models."
    )
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("pages", "models"):
        p = sub.add_parser(name, help=f"download the {name} into a new or empty directory")
        p.add_argument("dest", type=Path)
        p.add_argument("--only", action="append", default=[], metavar="TEXT")
        if name == "pages":
            p.add_argument("--accept-reencode", action="store_true")
    v = sub.add_parser("verify", help="check a directory against a manifest; no download")
    v.add_argument("kind", choices=("pages", "models"))
    v.add_argument("directory", type=Path)
    v.add_argument("--only", action="append", default=[], metavar="TEXT")
    args = parser.parse_args(argv)
    if args.command == "verify":
        return run_verify(args.kind, args.directory, args.only)
    return run_fetch(
        args.command, args.dest, args.only, getattr(args, "accept_reencode", False)
    )


if __name__ == "__main__":
    sys.exit(main())
