"""Tests for folio-mylar-push.py. Run: `python3 -m unittest -v` in this directory.

They build a small Mylar-shaped SQLite file, so no Mylar is needed.
"""

from __future__ import annotations

import datetime as dt
import http.server
import importlib.util
import io
import json
import os
import pathlib
import sqlite3
import tempfile
import threading
import unittest
from contextlib import redirect_stderr, redirect_stdout

HERE = pathlib.Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("folio_mylar_push", HERE / "folio-mylar-push.py")
push = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(push)

NOW = dt.datetime(2026, 11, 14, 18, 0, tzinfo=dt.timezone.utc)


def make_db(path: str, *, drop_column: str | None = None) -> None:
    """A Mylar-shaped database with one row of everything, plus the
    sensitive columns Mylar really has, so a leak would show up."""
    db = sqlite3.connect(path)
    weekly_cols = (
        "SHIPDATE TEXT, PUBLISHER TEXT, ISSUE TEXT, COMIC TEXT, STATUS TEXT, ComicID TEXT,"
        " IssueID TEXT, seriesyear TEXT, weeknumber TEXT, year TEXT"
    )
    if drop_column == "weekly.seriesyear":
        weekly_cols = weekly_cols.replace(" seriesyear TEXT,", "")
    db.executescript(
        f"""
        CREATE TABLE comics (ComicID TEXT, ComicName TEXT, Status TEXT, ComicPublished TEXT,
            ForceContinuing INTEGER, ComicLocation TEXT);
        CREATE TABLE issues (IssueID TEXT, ComicID TEXT, Issue_Number TEXT, ReleaseDate TEXT,
            Status TEXT, DateAdded TEXT, Location TEXT);
        CREATE TABLE annuals (IssueID TEXT, ComicID TEXT, ReleaseComicID TEXT,
            ReleaseComicName TEXT, Issue_Number TEXT, ReleaseDate TEXT, Status TEXT,
            DateAdded TEXT, Deleted INT DEFAULT 0, Location TEXT);
        CREATE TABLE weekly ({weekly_cols});
        CREATE TABLE snatched (IssueID TEXT, ComicID TEXT, ComicName TEXT, Issue_Number TEXT,
            Status TEXT, DateAdded TEXT, Provider TEXT, Hash TEXT, FolderName TEXT);
        CREATE TABLE ddl_info (ID TEXT, series TEXT, year TEXT, size TEXT, status TEXT,
            updated_date TEXT, link TEXT, mainlink TEXT, filename TEXT, site TEXT);
        CREATE TABLE jobhistory (JobName TEXT, status TEXT, prev_run_datetime TEXT,
            next_run_datetime TEXT);
        CREATE TABLE mylar_info (DatabaseVersion INTEGER PRIMARY KEY);
        INSERT INTO mylar_info VALUES (1);

        INSERT INTO comics VALUES ('91078', 'Action Comics', 'Active', '2016 - Present', 0,
            '/comics/DC Comics/Action Comics');
        INSERT INTO comics VALUES ('4050', 'Saga', 'Paused', '2012 - 2024', 0, '/comics/Image/Saga');
        INSERT INTO comics VALUES ('not-a-number', 'Broken', 'Active', NULL, 0, NULL);

        INSERT INTO issues VALUES ('1198752', '91078', '1104', '2026-11-11', 'Downloaded',
            '2026-11-11', '/comics/x.cbz');
        INSERT INTO issues VALUES ('1198999', '91078', '1105', '2026-11-18', 'Wanted',
            '2026-11-12', NULL);
        INSERT INTO annuals VALUES ('77001', '91078', '160001', 'Action Comics Annual', '1',
            '2026-10-01', 'Wanted', '2026-10-02', 0, NULL);

        INSERT INTO weekly ({', '.join(c.split()[0] for c in weekly_cols.split(', '))}) VALUES
            {"('2026-11-11', 'DC Comics', '1104', 'Action Comics', 'Downloaded', '91078', '1198752', '2016', '45', '2026')"
             if drop_column != 'weekly.seriesyear' else
             "('2026-11-11', 'DC Comics', '1104', 'Action Comics', 'Downloaded', '91078', '1198752', '45', '2026')"};
        """
    )
    if drop_column != "weekly.seriesyear":
        db.executemany(
            "INSERT INTO weekly VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            [
                # No ComicVine ids: the pull service could not match it.
                ("2026-11-11", "Mad Cave Studios", "3", "Unmatched Indie", "Skipped", "", None, "2026", "45", "2026"),
                # Long ago and far ahead: outside the window.
                ("2026-08-05", "DC Comics", "1100", "Action Comics", "Downloaded", "91078", "1", "2016", "31", "2026"),
                ("2027-03-03", "DC Comics", "1120", "Action Comics", "Wanted", "91078", "2", "2016", "09", "2027"),
            ],
        )
    db.executemany(
        "INSERT INTO snatched VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        [
            ("1198752", "91078", "Action Comics", "1104", "Post-Processed", "2026-11-12 22:53:31",
             "SECRET-INDEXER", "deadbeef", "/downloads/Action.Comics.1104"),
            ("5", "91078", "Action Comics", "1000", "Snatched", "2026-05-01 10:00:00",
             "SECRET-INDEXER", "cafe", "/downloads/old"),
        ],
    )
    db.executemany(
        "INSERT INTO ddl_info VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        [
            ("a", "X-Factor #2", "2024", "67 MB", "Queued", "2026-11-06 09:31",
             "https://secret.example/get/1", "https://secret.example/main", "x.cbz", "SECRET-SITE"),
            ("b", "Old Thing #1", "2020", "10 MB", "Completed", "2026-01-01 09:00",
             "https://secret.example/get/2", "", "old.cbz", "SECRET-SITE"),
            ("c", "Red Roots #5", "2026", "33 MB", "Failed", "2026-09-16 16:16",
             "https://secret.example/get/3", "", "r.cbz", "SECRET-SITE"),
        ],
    )
    db.execute(
        "INSERT INTO jobhistory VALUES ('Weekly Pullist', 'Waiting', '2026-11-15 02:02:19', '2026-11-15 06:02:19')"
    )
    db.commit()
    db.close()


class Receiver:
    """A local HTTP server standing in for Folio."""

    def __init__(self, status: int = 204):
        self.requests: list[dict] = []
        outer = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):  # noqa: N802
                length = int(self.headers.get("Content-Length", "0"))
                outer.requests.append(
                    {
                        "path": self.path,
                        "authorization": self.headers.get("Authorization"),
                        "user_agent": self.headers.get("User-Agent"),
                        "body": json.loads(self.rfile.read(length) or b"null"),
                    }
                )
                self.send_response(status)
                self.send_header("Content-Length", "0")
                self.end_headers()

            def log_message(self, *args):
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()


class SnapshotTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.db_path = os.path.join(self.tmp.name, "mylar.db")
        make_db(self.db_path)

    def snapshot(self, **kwargs) -> dict:
        db = push.open_db(self.db_path)
        try:
            push.check_schema(db, kwargs.get("include_annuals", False))
            return push.build_snapshot(db, now=NOW, **kwargs)
        finally:
            db.close()

    def test_pull_list_keeps_the_window_and_rows_without_ids(self):
        rows = self.snapshot()["pull_list"]
        self.assertEqual([r["series"] for r in rows], ["Action Comics", "Unmatched Indie"])
        action, indie = rows
        self.assertEqual(action["cv_series_id"], 91078)
        self.assertEqual(action["cv_issue_id"], 1198752)
        self.assertEqual(action["series_year"], 2016)
        self.assertEqual(action["status"], "Downloaded")
        self.assertIsNone(indie["cv_series_id"])
        self.assertIsNone(indie["cv_issue_id"])

    def test_watchlist_is_compact_and_skips_bad_ids(self):
        self.assertEqual(
            self.snapshot()["watchlist"],
            [[4050, "Paused", False], [91078, "Active", True]],
        )

    def test_annuals_are_only_included_when_asked(self):
        plain = self.snapshot()
        self.assertEqual([w["issue"] for w in plain["wanted"]], ["1105"])
        self.assertFalse(plain["meta"]["annuals_included"])

        with_annuals = self.snapshot(include_annuals=True)
        self.assertIn([160001, "Active", True], with_annuals["watchlist"])
        annual = [w for w in with_annuals["wanted"] if w["series"] == "Action Comics Annual"]
        self.assertEqual(len(annual), 1)
        self.assertEqual(annual[0]["cv_series_id"], 160001)
        self.assertTrue(with_annuals["meta"]["annuals_included"])

    def test_history_queue_and_jobs(self):
        snap = self.snapshot()
        self.assertEqual([h["issue"] for h in snap["history"]], ["1104"], "old history dropped")
        self.assertTrue(snap["history"][0]["at"].endswith("+00:00"))
        self.assertEqual(snap["queue"]["counts"], {"Queued": 1, "Failed": 1})
        self.assertEqual(
            sorted(q["title"] for q in snap["queue"]["items"]), ["Red Roots #5", "X-Factor #2"]
        )
        # jobhistory is UTC already and must not be shifted by the host's zone.
        self.assertEqual(snap["jobs"][0]["next_run"], "2026-11-15T06:02:19+00:00")
        self.assertEqual(snap["meta"]["schema_version"], 1)
        self.assertEqual(snap["meta"]["mylar_db_version"], 1)

    def test_nothing_sensitive_leaves_the_host(self):
        snap = self.snapshot(include_annuals=True)
        self.assertEqual(push.forbidden_keys_in(snap), [])
        text = json.dumps(snap)
        for secret in ("SECRET-INDEXER", "SECRET-SITE", "secret.example", "/downloads/", "/comics/", "deadbeef", ".cbz"):
            self.assertNotIn(secret, text)

    def test_the_forbidden_key_check_finds_nested_keys(self):
        self.assertEqual(
            push.forbidden_keys_in({"queue": {"items": [{"Link": "x"}]}, "token": 1}),
            ["queue.items[0].Link", "token"],
        )

    def test_fingerprint_ignores_only_the_generation_time(self):
        a = self.snapshot()
        b = json.loads(json.dumps(a))
        b["meta"]["generated_at"] = "2030-01-01T00:00:00+00:00"
        self.assertEqual(push.fingerprint(a), push.fingerprint(b))
        b["pull_list"][0]["status"] = "Wanted"
        self.assertNotEqual(push.fingerprint(a), push.fingerprint(b))

    def test_a_missing_column_fails_loudly(self):
        path = os.path.join(self.tmp.name, "old.db")
        make_db(path, drop_column="weekly.seriesyear")
        db = push.open_db(path)
        self.addCleanup(db.close)
        with self.assertRaises(push.SchemaError) as ctx:
            push.check_schema(db, False)
        self.assertIn("weekly.seriesyear", str(ctx.exception))

    def test_the_database_is_opened_read_only(self):
        db = push.open_db(self.db_path)
        self.addCleanup(db.close)
        with self.assertRaises(sqlite3.OperationalError):
            db.execute("UPDATE comics SET Status = 'Paused'")


class PushDecisionTests(unittest.TestCase):
    def test_push_when_changed_or_heartbeat_is_due(self):
        self.assertEqual(push.should_push({}, "h", 1000.0, 60), "changed")
        state = {"hash": "h", "pushed_at": 1000.0}
        self.assertIsNone(push.should_push(state, "h", 1000.0 + 59 * 60, 60))
        self.assertEqual(push.should_push(state, "h", 1000.0 + 60 * 60, 60), "heartbeat")
        self.assertEqual(push.should_push(state, "other", 1001.0, 60), "changed")
        self.assertEqual(
            push.should_push({"hash": "h"}, "h", 1001.0, 60), "no record of a previous push"
        )

    def test_ingest_url(self):
        self.assertEqual(
            push.ingest_url("https://folio.example.com/"),
            "https://folio.example.com/integrations/mylar/snapshot",
        )
        self.assertEqual(
            push.ingest_url("http://192.168.1.108:8080"),
            "http://192.168.1.108:8080/integrations/mylar/snapshot",
        )
        for bad in ("", "folio.example.com", "ftp://folio.example.com", "file:///etc/passwd"):
            with self.assertRaises(push.ConfigError):
                push.ingest_url(bad)


class MainTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.db_path = os.path.join(self.tmp.name, "mylar.db")
        self.state_dir = os.path.join(self.tmp.name, "state")
        make_db(self.db_path)
        self._env = dict(os.environ)
        self.addCleanup(self._restore_env)
        for key in ("FOLIO_URL", "FOLIO_TOKEN", "MYLAR_DB", "STATE_DIRECTORY",
                    "FOLIO_MYLAR_INCLUDE_ANNUALS", "FOLIO_MYLAR_HEARTBEAT_MINUTES"):
            os.environ.pop(key, None)

    def _restore_env(self):
        os.environ.clear()
        os.environ.update(self._env)

    def run_main(self, *extra: str) -> tuple[int, str, str]:
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = push.main(["--db", self.db_path, "--state-dir", self.state_dir, *extra])
        return code, out.getvalue(), err.getvalue()

    def test_pushes_once_then_skips_until_something_changes(self):
        receiver = Receiver()
        self.addCleanup(receiver.close)
        os.environ.update(FOLIO_URL=receiver.url, FOLIO_TOKEN="fmi_testtoken")

        code, out, _ = self.run_main()
        self.assertEqual(code, 0)
        self.assertIn("pushed (changed)", out)
        self.assertEqual(len(receiver.requests), 1)
        request = receiver.requests[0]
        self.assertEqual(request["path"], "/integrations/mylar/snapshot")
        self.assertEqual(request["authorization"], "Bearer fmi_testtoken")
        self.assertTrue(request["user_agent"].startswith("folio-mylar-push/"))
        self.assertEqual(request["body"]["meta"]["schema_version"], 1)

        code, out, _ = self.run_main()
        self.assertEqual(code, 0)
        self.assertIn("unchanged", out)
        self.assertEqual(len(receiver.requests), 1, "nothing sent when nothing changed")

        db = sqlite3.connect(self.db_path)
        db.execute("UPDATE issues SET Status = 'Snatched' WHERE IssueID = '1198999'")
        db.commit()
        db.close()
        code, out, _ = self.run_main()
        self.assertIn("pushed (changed)", out)
        self.assertEqual(len(receiver.requests), 2)

        code, out, _ = self.run_main("--force")
        self.assertIn("pushed (forced)", out)
        self.assertEqual(len(receiver.requests), 3)

    def test_a_heartbeat_is_sent_when_the_last_push_is_old(self):
        receiver = Receiver()
        self.addCleanup(receiver.close)
        os.environ.update(FOLIO_URL=receiver.url, FOLIO_TOKEN="fmi_testtoken")
        self.assertEqual(self.run_main()[0], 0)
        state_file = os.path.join(self.state_dir, "state.json")
        with open(state_file, encoding="utf-8") as fh:
            state = json.load(fh)
        state["pushed_at"] -= 61 * 60
        with open(state_file, "w", encoding="utf-8") as fh:
            json.dump(state, fh)

        code, out, _ = self.run_main()
        self.assertEqual(code, 0)
        self.assertIn("pushed (heartbeat)", out)
        self.assertEqual(len(receiver.requests), 2)

    def test_a_refused_push_fails_and_is_retried_next_run(self):
        receiver = Receiver(status=401)
        self.addCleanup(receiver.close)
        os.environ.update(FOLIO_URL=receiver.url, FOLIO_TOKEN="fmi_wrong")

        code, _, err = self.run_main()
        self.assertEqual(code, 1)
        self.assertIn("HTTP 401", err)
        self.assertNotIn("fmi_wrong", err)
        self.assertFalse(os.path.exists(os.path.join(self.state_dir, "state.json")))
        self.assertEqual(self.run_main()[0], 1, "still tries: no state was saved")
        self.assertEqual(len(receiver.requests), 2)

    def test_configuration_and_schema_errors_send_nothing(self):
        code, _, err = self.run_main()
        self.assertEqual(code, 2)
        self.assertIn("FOLIO_TOKEN is not set", err)

        os.environ.update(FOLIO_URL="not a url", FOLIO_TOKEN="fmi_testtoken")
        code, _, err = self.run_main()
        self.assertEqual(code, 2)
        self.assertIn("FOLIO_URL", err)

        old = os.path.join(self.tmp.name, "old.db")
        make_db(old, drop_column="weekly.seriesyear")
        out, err_io = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err_io):
            code = push.main(["--db", old, "--state-dir", self.state_dir])
        self.assertEqual(code, 2)
        self.assertIn("weekly.seriesyear", err_io.getvalue())

    def test_dry_run_and_print_send_nothing(self):
        code, out, _ = self.run_main("--dry-run")
        self.assertEqual(code, 0)
        self.assertIn("nothing sent", out)
        code, out, _ = self.run_main("--print")
        self.assertEqual(code, 0)
        self.assertEqual(json.loads(out)["meta"]["schema_version"], 1)


if __name__ == "__main__":
    unittest.main()
