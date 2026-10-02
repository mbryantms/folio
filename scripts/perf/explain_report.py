#!/usr/bin/env python3
"""Split an auto_explain Postgres log into per-endpoint plan files.

Input  (all written by scripts/perf/perf-explain.sh into <out>/):
  postgres.log   `docker logs` of the throwaway Postgres (auto_explain on,
                 log_min_duration=0, ANALYZE + BUFFERS)
  requests.txt   label|http status|wall ms|url, one line per endpoint
  reltuples.txt  relname|reltuples for every public table (post-ANALYZE)

Output:
  plans/<label>.txt  every statement the endpoint ran, with its plan
  summary.md         one row per endpoint + the flagged seq scans

Exit status 1 when any endpoint seq-scans a table holding at least
--min-rows rows (default 5,000: the tables that grow with the issue count —
`issues`, its junctions, `field_provenance`, …). Seq scans of smaller
tables holding at least --info-rows rows (default 1,000; at stress scale
that is `series`, 2,500 rows / ~150 pages) are listed but don't fail: the
planner legitimately prefers them, e.g. as the build side of a hash join
that touches 10 % of the table.

`scripts/perf/expected_seqscans.txt` lists reviewed `label|table|reason`
exceptions: plans whose large-table seq scan is right for the fixture
data (e.g. the similar-series tag arm, where every stress-fixture series
carries all eight tags). They are listed as info, marked `[expected]`.

Labels starting with `job-` are whole-library batch jobs (the M7
`relationship_suggest` run): scanning the library is their job, so their
seq scans are listed but never fail the gate.
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

HEAD = re.compile(r"LOG:\s+duration: ([0-9.]+) ms\s+plan:\s*$")
MARK = re.compile(r"perf-marker:(begin|end):([A-Za-z0-9_.-]+)")
SEQ = re.compile(r"(?:Parallel )?Seq Scan on (\w+)")


@dataclass
class Stmt:
    ms: float
    lines: list[str] = field(default_factory=list)

    @property
    def text(self) -> str:
        return "\n".join(self.lines)

    @property
    def query(self) -> str:
        for ln in self.lines:
            s = ln.strip()
            if s.startswith("Query Text:"):
                return s[len("Query Text:") :].strip()
        return ""


def parse(log: Path) -> list[Stmt]:
    out: list[Stmt] = []
    cur: Stmt | None = None
    for raw in log.read_text(errors="replace").splitlines():
        m = HEAD.search(raw)
        if m:
            cur = Stmt(float(m.group(1)))
            out.append(cur)
            continue
        if cur is not None and (raw.startswith("\t") or raw.startswith("  ")):
            cur.lines.append(raw.rstrip())
            continue
        cur = None
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("out", type=Path)
    ap.add_argument("--min-rows", type=int, default=5000)
    ap.add_argument("--info-rows", type=int, default=1000)
    args = ap.parse_args()
    out: Path = args.out

    stmts = parse(out / "postgres.log")
    tuples: dict[str, int] = {}
    for ln in (out / "reltuples.txt").read_text().splitlines():
        if "|" in ln:
            name, n = ln.split("|", 1)
            tuples[name.strip()] = int(float(n.strip() or 0))
    expected: set[tuple[str, str]] = set()
    allow = Path(__file__).with_name("expected_seqscans.txt")
    if allow.exists():
        for ln in allow.read_text().splitlines():
            if ln.strip() and not ln.lstrip().startswith("#"):
                lbl, tbl, _reason = (x.strip() for x in ln.split("|", 2))
                expected.add((lbl, tbl))
    requests = []
    for ln in (out / "requests.txt").read_text().splitlines():
        label, code, wall, url = ln.split("|", 3)
        requests.append((label, code, int(wall), url))

    # Bucket statements between each endpoint's begin/end marker.
    buckets: dict[str, list[Stmt]] = {}
    active: str | None = None
    for st in stmts:
        mk = MARK.search(st.query)
        if mk:
            kind, label = mk.groups()
            active = label if kind == "begin" else None
            continue
        if active is not None:
            buckets.setdefault(active, []).append(st)

    plans = out / "plans"
    plans.mkdir(exist_ok=True)
    rows = []
    flagged_any = False
    for label, code, wall, url in requests:
        sts = buckets.get(label, [])
        flagged: list[str] = []
        info: list[str] = []
        for st in sts:
            for tbl in SEQ.findall(st.text):
                n = tuples.get(tbl, 0)
                if n >= args.min_rows and (label, tbl) in expected:
                    info.append(f"{tbl} ({n:,} rows) [expected]")
                elif n >= args.min_rows:
                    flagged.append(f"{tbl} ({n:,} rows)")
                elif n >= args.info_rows:
                    info.append(f"{tbl} ({n:,} rows)")
        flagged = sorted(set(flagged))
        info = sorted(set(info))
        if label.startswith("job-") and flagged:
            info = sorted(set(info) | {f"{f} [batch]" for f in flagged})
            flagged = []
        flagged_any |= bool(flagged)
        slowest = max(sts, key=lambda s: s.ms, default=None)
        with (plans / f"{label}.txt").open("w") as fh:
            fh.write(f"# {label}  GET {url}  → HTTP {code}, {wall} ms wall\n\n")
            for i, st in enumerate(sts, 1):
                fh.write(f"── statement {i}: {st.ms:.3f} ms ──\n{st.text}\n\n")
        rows.append(
            (
                label,
                code,
                wall,
                len(sts),
                sum(s.ms for s in sts),
                slowest.ms if slowest else 0.0,
                ", ".join(flagged) or "—",
                ", ".join(info) or "—",
            )
        )

    with (out / "summary.md").open("w") as fh:
        fh.write(
            f"Gate: seq scans on tables with ≥ {args.min_rows:,} rows. "
            f"Info: seq scans on tables with {args.info_rows:,}–{args.min_rows - 1:,} rows.\n\n"
            "| endpoint | HTTP | wall ms | stmts | Σ SQL ms | slowest ms | seq scans (gate) | seq scans (info) |\n"
            "|---|---|---:|---:|---:|---:|---|---|\n"
        )
        for r in rows:
            fh.write(
                f"| {r[0]} | {r[1]} | {r[2]} | {r[3]} | {r[4]:.1f} | {r[5]:.1f} | {r[6]} | {r[7]} |\n"
            )
    print((out / "summary.md").read_text())
    print(f"plans: {plans}/")
    return 1 if flagged_any else 0


if __name__ == "__main__":
    sys.exit(main())
