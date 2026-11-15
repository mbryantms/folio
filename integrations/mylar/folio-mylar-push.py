#!/usr/bin/env python3
"""Push a read-only snapshot of a Mylar3 database to Folio.

Folio shows what Mylar is tracking but never controls it. This script is
the whole link between the two: it runs ON the Mylar host, opens
`mylar.db` read-only, and POSTs a snapshot to Folio. Folio never calls
Mylar, so Mylar needs no open port and Folio holds no Mylar key.

What it does not do, by design:

- It never writes to `mylar.db` (SQLite is opened with `mode=ro`, and
  every statement is a SELECT).
- It never calls Mylar's HTTP API, so it needs no Mylar API key and does
  not read `config.ini`.
- It never sends file paths, download links, indexer or provider names,
  NZB names or any credential. `FORBIDDEN_KEYS` below is asserted by the
  test suite against every snapshot.

Configuration (environment, normally an EnvironmentFile; see README.md):

    FOLIO_URL      Folio's origin, e.g. https://folio.example.com
    FOLIO_TOKEN    The ingest token from Folio's Admin -> Upcoming releases
    MYLAR_DB       Path to mylar.db (default /opt/mylar3-data/mylar.db)
    FOLIO_MYLAR_INCLUDE_ANNUALS   "1" when Mylar's annual integration is on
    FOLIO_MYLAR_HEARTBEAT_MINUTES Push even when nothing changed (default 60)
    STATE_DIRECTORY               Set by systemd (StateDirectory=)

Standard library only. Python 3.9+.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import sqlite3
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

SCHEMA_VERSION = 1
USER_AGENT = f"folio-mylar-push/{SCHEMA_VERSION}"
INGEST_PATH = "/integrations/mylar/snapshot"
DEFAULT_DB = "/opt/mylar3-data/mylar.db"
DEFAULT_HEARTBEAT_MINUTES = 60

# How far around today the pull list is read. Mylar keeps the current
# week (and any later week someone opened in its UI); the previous week
# stays useful for "just arrived".
PULL_DAYS_BACK = 10
PULL_DAYS_AHEAD = 28
HISTORY_DAYS = 14
# Caps mirror Folio's ingest limits; a snapshot over them is refused.
MAX_PULL = 2000
MAX_WATCH = 50000
MAX_WANTED = 2000
MAX_HISTORY = 500
MAX_QUEUE = 200

# Every table and column the snapshot reads. A Mylar upgrade that drops
# one makes the run fail with its name instead of sending a partial
# snapshot.
REQUIRED = {
    "comics": ["ComicID", "ComicName", "Status", "ComicPublished", "ForceContinuing"],
    "issues": [
        "IssueID",
        "ComicID",
        "Issue_Number",
        "ReleaseDate",
        "Status",
        "DateAdded",
    ],
    "weekly": [
        "SHIPDATE",
        "PUBLISHER",
        "ISSUE",
        "COMIC",
        "STATUS",
        "ComicID",
        "IssueID",
        "seriesyear",
    ],
    "snatched": ["IssueID", "ComicID", "ComicName", "Issue_Number", "Status", "DateAdded"],
    "ddl_info": ["series", "year", "size", "status", "updated_date"],
    "jobhistory": ["JobName", "status", "prev_run_datetime", "next_run_datetime"],
}
ANNUALS_REQUIRED = {
    "annuals": [
        "IssueID",
        "ComicID",
        "ReleaseComicID",
        "ReleaseComicName",
        "Issue_Number",
        "ReleaseDate",
        "Status",
        "DateAdded",
        "Deleted",
    ],
}

# Keys that must never appear anywhere in a snapshot.
FORBIDDEN_KEYS = frozenset(
    {
        "link",
        "mainlink",
        "filename",
        "tmp_filename",
        "location",
        "comiclocation",
        "foldername",
        "provider",
        "hash",
        "nzbname",
        "site",
        "apikey",
        "api_key",
        "password",
        "token",
    }
)

OPEN_STATUSES = ("Wanted", "Snatched", "Failed")


class ConfigError(Exception):
    """A setting is missing or unusable."""


class SchemaError(Exception):
    """mylar.db does not have a table or column this script reads."""


def open_db(path: str) -> sqlite3.Connection:
    if not os.path.isfile(path):
        raise ConfigError(f"MYLAR_DB does not exist: {path}")
    uri = "file:" + urllib.parse.quote(os.path.abspath(path)) + "?mode=ro"
    db = sqlite3.connect(uri, uri=True, timeout=5)
    db.row_factory = sqlite3.Row
    return db


def check_schema(db: sqlite3.Connection, include_annuals: bool) -> None:
    wanted = dict(REQUIRED)
    if include_annuals:
        wanted.update(ANNUALS_REQUIRED)
    missing = []
    for table, columns in wanted.items():
        have = {row["name"].lower() for row in db.execute(f'PRAGMA table_info("{table}")')}
        if not have:
            missing.append(f"table {table}")
            continue
        missing.extend(f"{table}.{c}" for c in columns if c.lower() not in have)
    if missing:
        raise SchemaError(
            "mylar.db is missing " + ", ".join(missing) + ". This Mylar version is not supported"
            " by this script; nothing was sent."
        )


def _int(value) -> int | None:
    """A ComicVine id as an integer, or None for blank / non-numeric."""
    if value is None:
        return None
    text = str(value).strip()
    return int(text) if text.isdigit() else None


def _text(value, limit: int = 300) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    return text[:limit] if text else None


def _date(value) -> str | None:
    """`YYYY-MM-DD`, or None for blank / `0000-00-00` / anything else."""
    text = _text(value, 10)
    if not text or text.startswith("0000"):
        return None
    try:
        return dt.date.fromisoformat(text).isoformat()
    except ValueError:
        return None


def _stamp(value, *, utc: bool) -> str | None:
    """A Mylar timestamp as RFC 3339.

    Mylar writes most timestamps as naive local time (`utc=False`); the
    scheduler's `jobhistory` columns are UTC (`utc=True`).
    """
    text = _text(value, 32)
    if not text:
        return None
    for fmt in ("%Y-%m-%d %H:%M:%S.%f", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%d"):
        try:
            parsed = dt.datetime.strptime(text, fmt)
        except ValueError:
            continue
        if utc:
            parsed = parsed.replace(tzinfo=dt.timezone.utc)
        else:
            parsed = parsed.astimezone()  # attach the host's local offset
        return parsed.astimezone(dt.timezone.utc).isoformat(timespec="seconds")
    return None


def build_snapshot(
    db: sqlite3.Connection, *, now: dt.datetime | None = None, include_annuals: bool = False
) -> dict:
    """Read everything Folio shows. `now` is timezone-aware."""
    now = now or dt.datetime.now(dt.timezone.utc)
    today = now.astimezone().date()

    pull_list = [
        {
            "ship_date": _date(r["SHIPDATE"]),
            "publisher": _text(r["PUBLISHER"], 120),
            "series": _text(r["COMIC"]),
            "issue": _text(r["ISSUE"], 40),
            "status": _text(r["STATUS"], 24),
            "cv_series_id": _int(r["ComicID"]),
            "cv_issue_id": _int(r["IssueID"]),
            "series_year": _int(r["seriesyear"]),
        }
        for r in db.execute(
            "SELECT SHIPDATE, PUBLISHER, COMIC, ISSUE, STATUS, ComicID, IssueID, seriesyear"
            " FROM weekly WHERE COMIC IS NOT NULL AND ISSUE IS NOT NULL"
            " AND SHIPDATE >= ? AND SHIPDATE <= ?"
            " ORDER BY SHIPDATE, COMIC, ISSUE LIMIT ?",
            (
                (today - dt.timedelta(days=PULL_DAYS_BACK)).isoformat(),
                (today + dt.timedelta(days=PULL_DAYS_AHEAD)).isoformat(),
                MAX_PULL,
            ),
        )
    ]
    pull_list = [p for p in pull_list if p["ship_date"] and p["series"] and p["issue"]]

    # [ComicVine volume id, status, still publishing] — compact on purpose:
    # a large watchlist is most of the payload.
    watch: dict[int, list] = {}
    for r in db.execute(
        "SELECT ComicID, Status, ComicPublished, ForceContinuing FROM comics ORDER BY ComicID"
    ):
        cv = _int(r["ComicID"])
        if cv is None:
            continue
        continuing = "present" in str(r["ComicPublished"] or "").lower() or r["ForceContinuing"] == 1
        watch[cv] = [cv, _text(r["Status"], 24) or "Active", bool(continuing)]
    if include_annuals:
        # An annual is its own ComicVine volume; it is tracked through its
        # parent series, so it takes the parent's state.
        for r in db.execute(
            "SELECT DISTINCT ReleaseComicID, ComicID FROM annuals WHERE NOT Deleted"
        ):
            annual, parent = _int(r["ReleaseComicID"]), _int(r["ComicID"])
            if annual is None or annual in watch or parent not in watch:
                continue
            watch[annual] = [annual, watch[parent][1], watch[parent][2]]
    watchlist = list(watch.values())[:MAX_WATCH]

    placeholders = ",".join("?" for _ in OPEN_STATUSES)
    wanted = [
        {
            "cv_series_id": _int(r["ComicID"]),
            "cv_issue_id": _int(r["IssueID"]),
            "series": _text(r["ComicName"]),
            "issue": _text(r["Issue_Number"], 40),
            "store_date": _date(r["ReleaseDate"]),
            "status": _text(r["Status"], 24),
            "added_at": _stamp(r["DateAdded"], utc=False),
        }
        for r in db.execute(
            "SELECT i.ComicID, i.IssueID, c.ComicName, i.Issue_Number, i.ReleaseDate, i.Status,"
            f" i.DateAdded FROM issues i JOIN comics c ON c.ComicID = i.ComicID"
            f" WHERE i.Status IN ({placeholders})"
            " ORDER BY i.ReleaseDate DESC, c.ComicName LIMIT ?",
            (*OPEN_STATUSES, MAX_WANTED),
        )
    ]
    if include_annuals:
        wanted.extend(
            {
                "cv_series_id": _int(r["ReleaseComicID"]) or _int(r["ComicID"]),
                "cv_issue_id": _int(r["IssueID"]),
                "series": _text(r["ReleaseComicName"]),
                "issue": _text(r["Issue_Number"], 40),
                "store_date": _date(r["ReleaseDate"]),
                "status": _text(r["Status"], 24),
                "added_at": _stamp(r["DateAdded"], utc=False),
            }
            for r in db.execute(
                "SELECT ReleaseComicID, ComicID, IssueID, ReleaseComicName, Issue_Number,"
                " ReleaseDate, Status, DateAdded FROM annuals"
                f" WHERE NOT Deleted AND Status IN ({placeholders})"
                " ORDER BY ReleaseDate DESC LIMIT ?",
                (*OPEN_STATUSES, MAX_WANTED),
            )
        )
    wanted = [w for w in wanted if w["series"] and w["issue"] and w["status"]][:MAX_WANTED]

    since = (now.astimezone() - dt.timedelta(days=HISTORY_DAYS)).strftime("%Y-%m-%d %H:%M:%S")
    history = [
        {
            "cv_series_id": _int(r["ComicID"]),
            "cv_issue_id": _int(r["IssueID"]),
            "series": _text(r["ComicName"]),
            "issue": _text(r["Issue_Number"], 40),
            "status": _text(r["Status"], 24),
            "at": _stamp(r["DateAdded"], utc=False),
        }
        for r in db.execute(
            "SELECT ComicID, IssueID, ComicName, Issue_Number, Status, DateAdded FROM snatched"
            " WHERE DateAdded >= ? ORDER BY DateAdded DESC LIMIT ?",
            (since, MAX_HISTORY),
        )
    ]
    history = [h for h in history if h["series"] and h["status"]]

    queue_counts = {
        str(r["status"]): int(r["n"])
        for r in db.execute(
            "SELECT status, count(*) AS n FROM ddl_info"
            " WHERE status IS NOT NULL AND status <> 'Completed' GROUP BY status"
        )
    }
    queue_items = [
        {
            "title": _text(r["series"]),
            "year": _text(r["year"], 12),
            "size": _text(r["size"], 24),
            "status": _text(r["status"], 24),
            "updated_at": _stamp(r["updated_date"], utc=False),
        }
        for r in db.execute(
            "SELECT series, year, size, status, updated_date FROM ddl_info"
            " WHERE status IS NOT NULL AND status <> 'Completed'"
            " ORDER BY updated_date DESC LIMIT ?",
            (MAX_QUEUE,),
        )
    ]
    queue_items = [q for q in queue_items if q["title"] and q["status"]]

    jobs = [
        {
            "name": _text(r["JobName"], 80),
            "status": _text(r["status"], 24),
            "prev_run": _stamp(r["prev_run_datetime"], utc=True),
            "next_run": _stamp(r["next_run_datetime"], utc=True),
        }
        for r in db.execute(
            "SELECT JobName, status, prev_run_datetime, next_run_datetime FROM jobhistory"
            " ORDER BY JobName LIMIT 50"
        )
    ]
    jobs = [j for j in jobs if j["name"]]

    db_version = None
    try:
        row = db.execute("SELECT DatabaseVersion FROM mylar_info LIMIT 1").fetchone()
        db_version = _int(row[0]) if row else None
    except sqlite3.Error:
        pass  # older Mylar: no mylar_info table

    return {
        "meta": {
            "schema_version": SCHEMA_VERSION,
            "generated_at": now.astimezone(dt.timezone.utc).isoformat(timespec="seconds"),
            "mylar_db_version": db_version,
            "annuals_included": include_annuals,
        },
        "pull_list": pull_list,
        "watchlist": watchlist,
        "wanted": wanted,
        "history": history,
        "queue": {"counts": queue_counts, "items": queue_items},
        "jobs": jobs,
    }


def fingerprint(snapshot: dict) -> str:
    """Stable hash of everything except when the snapshot was made."""
    body = dict(snapshot)
    body["meta"] = {k: v for k, v in snapshot["meta"].items() if k != "generated_at"}
    canonical = json.dumps(body, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def forbidden_keys_in(value, _path: str = "") -> list[str]:
    """Every forbidden key found anywhere in `value` (for the tests)."""
    found = []
    if isinstance(value, dict):
        for key, inner in value.items():
            here = f"{_path}.{key}" if _path else str(key)
            if str(key).lower() in FORBIDDEN_KEYS:
                found.append(here)
            found.extend(forbidden_keys_in(inner, here))
    elif isinstance(value, list):
        for i, inner in enumerate(value):
            found.extend(forbidden_keys_in(inner, f"{_path}[{i}]"))
    return found


def load_state(state_dir: str | None) -> dict:
    if not state_dir:
        return {}
    try:
        with open(os.path.join(state_dir, "state.json"), encoding="utf-8") as fh:
            state = json.load(fh)
        return state if isinstance(state, dict) else {}
    except (OSError, ValueError):
        return {}


def save_state(state_dir: str | None, digest: str, now_epoch: float) -> None:
    if not state_dir:
        return
    os.makedirs(state_dir, exist_ok=True)
    tmp = os.path.join(state_dir, "state.json.tmp")
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump({"hash": digest, "pushed_at": now_epoch}, fh)
    os.replace(tmp, os.path.join(state_dir, "state.json"))


def should_push(state: dict, digest: str, now_epoch: float, heartbeat_minutes: int) -> str | None:
    """Why to push now, or None to skip."""
    if state.get("hash") != digest:
        return "changed"
    pushed_at = state.get("pushed_at")
    if not isinstance(pushed_at, (int, float)):
        return "no record of a previous push"
    if now_epoch - pushed_at >= heartbeat_minutes * 60:
        return "heartbeat"
    return None


def ingest_url(folio_url: str) -> str:
    parsed = urllib.parse.urlsplit(folio_url.strip())
    if parsed.scheme not in ("http", "https") or not parsed.netloc:
        raise ConfigError("FOLIO_URL must be an http(s) origin, e.g. https://folio.example.com")
    return f"{parsed.scheme}://{parsed.netloc}{parsed.path.rstrip('/')}{INGEST_PATH}"


def push(url: str, token: str, snapshot: dict, timeout: float = 30.0) -> None:
    body = json.dumps(snapshot, separators=(",", ":")).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=body,
        method="POST",
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "User-Agent": USER_AGENT,
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            response.read()
    except urllib.error.HTTPError as err:
        detail = err.read(400).decode("utf-8", "replace").replace(token, "REDACTED")
        raise RuntimeError(f"Folio answered HTTP {err.code}: {detail}") from None
    except (urllib.error.URLError, OSError) as err:
        raise RuntimeError(f"could not reach Folio: {err}") from None


def summary(snapshot: dict) -> str:
    return (
        f"{len(snapshot['pull_list'])} pull-list rows, {len(snapshot['watchlist'])} watched series,"
        f" {len(snapshot['wanted'])} wanted, {len(snapshot['history'])} history,"
        f" {len(snapshot['queue']['items'])} queue items, {len(snapshot['jobs'])} jobs"
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--db", default=os.environ.get("MYLAR_DB", DEFAULT_DB))
    parser.add_argument("--state-dir", default=os.environ.get("STATE_DIRECTORY"))
    parser.add_argument("--dry-run", action="store_true", help="build and summarise; send nothing")
    parser.add_argument("--print", action="store_true", help="write the snapshot JSON to stdout")
    parser.add_argument("--force", action="store_true", help="push even when nothing changed")
    args = parser.parse_args(argv)

    include_annuals = os.environ.get("FOLIO_MYLAR_INCLUDE_ANNUALS", "") == "1"
    try:
        heartbeat = int(os.environ.get("FOLIO_MYLAR_HEARTBEAT_MINUTES", DEFAULT_HEARTBEAT_MINUTES))
    except ValueError:
        heartbeat = DEFAULT_HEARTBEAT_MINUTES

    try:
        db = open_db(args.db)
        try:
            check_schema(db, include_annuals)
            snapshot = build_snapshot(db, include_annuals=include_annuals)
        finally:
            db.close()
        if args.print:
            json.dump(snapshot, sys.stdout, indent=1)
            sys.stdout.write("\n")
            return 0
        if args.dry_run:
            print(f"dry run: {summary(snapshot)}; nothing sent")
            return 0

        folio_url = os.environ.get("FOLIO_URL", "")
        token = os.environ.get("FOLIO_TOKEN", "").strip()
        if not token:
            raise ConfigError("FOLIO_TOKEN is not set")
        url = ingest_url(folio_url)
    except (ConfigError, SchemaError) as err:
        print(f"folio-mylar-push: {err}", file=sys.stderr)
        return 2
    except sqlite3.Error as err:
        # A busy or half-written database: nothing was sent, the next run retries.
        print(f"folio-mylar-push: could not read mylar.db: {err}", file=sys.stderr)
        return 1

    digest = fingerprint(snapshot)
    now_epoch = time.time()
    reason = "forced" if args.force else should_push(
        load_state(args.state_dir), digest, now_epoch, heartbeat
    )
    if reason is None:
        print("unchanged since the last push; nothing sent")
        return 0
    try:
        push(url, token, snapshot)
    except RuntimeError as err:
        print(f"folio-mylar-push: {err}", file=sys.stderr)
        return 1
    save_state(args.state_dir, digest, now_epoch)
    print(f"pushed ({reason}): {summary(snapshot)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
