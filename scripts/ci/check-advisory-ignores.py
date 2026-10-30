#!/usr/bin/env python3
"""Keep the accepted-advisory ignore lists honest.

Folio records accepted-risk advisories in three places that no tool keeps
in sync for us:

  deny.toml            `[advisories].ignore`     cargo-deny   (Rust)
  .cargo/audit.toml    `[advisories].ignore`     cargo-audit  (Rust; also
                                                 lockfile-only + unsound ids)
  osv-scanner.toml     `[[IgnoredVulns]]`        osv-scanner  (Rust + npm)
  pnpm-workspace.yaml  `auditConfig.ignoreGhsas` pnpm audit   (npm)

osv-scanner reads neither deny.toml nor audit.toml nor the pnpm config, so
every id accepted in those must be restated in osv-scanner.toml or the
daily OSV sweep goes red on a finding that was triaged weeks earlier
(issue #1208: the three `zip` <9 ids and `crypto-bigint`). This script is
the gate that drift used to slip through.

A Rust advisory the RustSec db has no id for yet (GitHub-only, as the
2026-10-29 `zip` batch and `block-buffer` were) cannot go in deny.toml's
or audit.toml's `ignore` array: an id the db does not know only warns as
`advisory-not-detected` and reads as stale. Its GHSA goes on a comment
line `# osv-only: GHSA-…` in deny.toml, under the full rationale, and in
osv-scanner.toml; that line is what backs the osv entry here.

Checks
  mirror   every RUSTSEC id in deny.toml or audit.toml, every GHSA on an
           `osv-only:` line in deny.toml, and every GHSA in
           pnpm-workspace.yaml, appears in osv-scanner.toml; and every id
           in osv-scanner.toml is backed by one of those sources (an
           osv-only id with no such line has no rationale on file and is
           stale by construction).                         exit 1 on drift
  stale    (daily sweep only) an ignored id that no longer matches
           anything: cargo-deny's `advisory-not-detected` diagnostics
           (`--deny-json`, from `cargo deny --format json`), and, for the
           audit.toml-only ids cargo-deny never evaluates, an advisory
           whose crate is no longer in Cargo.lock (`--advisory-db`, the
           RustSec checkout cargo-audit uses). A stale ignore means the
           upstream fix landed or the crate left the graph: delete the
           entry, and check whether the pin that held it can now move.
           Also (same db) an `osv-only:` GHSA that the RustSec db now
           lists as an alias: move it into the `ignore` arrays under its
           RUSTSEC id, or cargo-deny starts failing on it.
                                                           exit 2 if stale

Usage
  scripts/ci/check-advisory-ignores.py                  # mirror only (CI)
  scripts/ci/check-advisory-ignores.py \
      --deny-json deny.jsonl --advisory-db "$RUNNER_TEMP/advisory-db"
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
RUSTSEC = re.compile(r"RUSTSEC-\d{4}-\d{4}")
GHSA = re.compile(r"GHSA-[0-9a-z]{4}-[0-9a-z]{4}-[0-9a-z]{4}")


def strip_comments(text: str, marker: str = "#") -> str:
    """Drop everything after a comment marker on each line. Good enough
    for these files: none of the ids live inside a quoted string that
    also contains the marker."""
    out = []
    for line in text.splitlines():
        i = line.find(marker)
        out.append(line if i < 0 else line[:i])
    return "\n".join(out)


def ids_in_ignore_array(path: pathlib.Path) -> set[str]:
    """RUSTSEC ids inside the `[advisories]` table's `ignore = [...]`."""
    text = strip_comments(path.read_text())
    m = re.search(r"^\s*ignore\s*=\s*\[(.*?)^\s*\]", text, re.M | re.S)
    if not m:
        sys.exit(f"{path}: no `ignore = [...]` array found")
    return set(RUSTSEC.findall(m.group(1)))


def ids_in_osv(path: pathlib.Path) -> set[str]:
    text = strip_comments(path.read_text())
    return set(re.findall(r'^\s*id\s*=\s*"([^"]+)"', text, re.M))


def osv_only_in_deny(path: pathlib.Path) -> set[str]:
    """GHSA ids on `# osv-only: …` comment lines in deny.toml: Rust
    advisories the RustSec db has no id for yet, accepted with their
    rationale in deny.toml but reportable only by osv-scanner."""
    return set(GHSA.findall(" ".join(re.findall(r"^\s*#\s*osv-only:(.*)$", path.read_text(), re.M))))


def rustsec_aliases(db: pathlib.Path) -> dict[str, str]:
    """GHSA alias -> RUSTSEC id, over every advisory in the RustSec checkout."""
    out: dict[str, str] = {}
    for md in (db / "crates").glob("*/RUSTSEC-*.md"):
        m = re.search(r"^aliases\s*=\s*\[(.*?)\]", md.read_text(), re.M | re.S)
        for ghsa in GHSA.findall(m.group(1)) if m else []:
            out[ghsa] = md.stem
    return out


def ghsas_in_pnpm(path: pathlib.Path) -> set[str]:
    text = strip_comments(path.read_text())
    m = re.search(r"^\s*ignoreGhsas:\s*\n((?:\s*-\s*\S+\s*\n?)*)", text, re.M)
    return set(GHSA.findall(m.group(1))) if m else set()


def lockfile_versions(path: pathlib.Path) -> dict[str, list[str]]:
    """crate name -> every version of it in Cargo.lock."""
    out: dict[str, list[str]] = {}
    for name, version in re.findall(r'^name = "([^"]+)"\nversion = "([^"]+)"', path.read_text(), re.M):
        out.setdefault(name, []).append(version)
    return out


def advisory_front_matter(db: pathlib.Path, advisory_id: str) -> tuple[str, list[str], list[str]] | None:
    """(crate, patched reqs, unaffected reqs) from the RustSec markdown's
    TOML front matter. Layout: crates/<crate>/<RUSTSEC-id>.md."""
    hits = list((db / "crates").glob(f"*/{advisory_id}.md"))
    if not hits:
        return None
    text = hits[0].read_text()
    def reqs(key: str) -> list[str]:
        m = re.search(rf"^{key}\s*=\s*\[(.*?)\]", text, re.M | re.S)
        return re.findall(r'"([^"]+)"', m.group(1)) if m else []
    return hits[0].parent.name, reqs("patched"), reqs("unaffected")


def _ver(v: str) -> tuple[int, ...]:
    core = v.split("+")[0].split("-")[0]
    parts = [int(x) for x in core.split(".")]
    return tuple(parts + [0] * (3 - len(parts)))


def _matches(version: str, req: str) -> bool:
    """A small Cargo version-requirement evaluator: `>=`, `>`, `<`, `<=`,
    `=`, `^`, `~` and bare versions, comma-joined. Enough for RustSec
    `[versions]` ranges; pre-release tags are ignored."""
    v = _ver(version)
    for part in [r.strip() for r in req.split(",") if r.strip()]:
        m = re.match(r"^(>=|<=|>|<|=|\^|~)?\s*(\d+(?:\.\d+){0,2})", part)
        if not m:
            return False
        op, raw = m.group(1) or "^", m.group(2)
        n = raw.count(".") + 1
        r = _ver(raw)
        if op == ">=" and not v >= r: return False
        if op == ">" and not v > r: return False
        if op == "<" and not v < r: return False
        if op == "<=" and not v <= r: return False
        if op == "=" and v[:n] != r[:n]: return False
        if op in ("^", "~"):
            if v < r: return False
            if op == "~":
                hi = (r[0], r[1] + 1, 0) if n >= 2 else (r[0] + 1, 0, 0)
            elif r[0] > 0 or n == 1:
                hi = (r[0] + 1, 0, 0)
            elif r[1] > 0 or n == 2:
                hi = (0, r[1] + 1, 0)
            else:
                hi = (0, 0, r[2] + 1)
            if not v < hi: return False
    return True


def affected(version: str, patched: list[str], unaffected: list[str]) -> bool:
    return not any(_matches(version, r) for r in patched + unaffected)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--deny-json", type=pathlib.Path,
                    help="cargo-deny `--format json` diagnostics (one JSON object per line)")
    ap.add_argument("--advisory-db", type=pathlib.Path,
                    help="RustSec advisory-db checkout (cargo-audit's --db path)")
    args = ap.parse_args()

    deny = ids_in_ignore_array(ROOT / "deny.toml")
    audit = ids_in_ignore_array(ROOT / ".cargo" / "audit.toml")
    osv_only = osv_only_in_deny(ROOT / "deny.toml")
    osv = ids_in_osv(ROOT / "osv-scanner.toml")
    npm = ghsas_in_pnpm(ROOT / "pnpm-workspace.yaml")
    sources = deny | audit | osv_only | npm

    problems: list[str] = []
    for i in sorted((deny | audit) - osv):
        where = " + ".join(n for n, s in (("deny.toml", deny), (".cargo/audit.toml", audit)) if i in s)
        problems.append(f"{i} is accepted in {where} but missing from osv-scanner.toml")
    for i in sorted(osv_only - osv):
        problems.append(f"{i} is on an `osv-only:` line in deny.toml but missing from osv-scanner.toml")
    for i in sorted(npm - osv):
        problems.append(f"{i} is in pnpm-workspace.yaml ignoreGhsas but missing from osv-scanner.toml")
    for i in sorted(osv - sources):
        problems.append(f"{i} is in osv-scanner.toml only — no deny.toml / audit.toml / osv-only / pnpm rationale backs it")

    if problems:
        print("advisory ignore lists have drifted:")
        for p in problems:
            print(f"  - {p}")
        print("\nRule: osv-scanner.toml mirrors deny.toml (ignore array + `osv-only:` lines) + "
              ".cargo/audit.toml + pnpm-workspace.yaml ignoreGhsas exactly "
              "(see docs/dev/dependency-management.md, Accepted advisories).")
        return 1

    print(f"ignore lists in sync: {len(deny)} deny, {len(audit)} audit, "
          f"{len(osv_only)} osv-only, {len(npm)} npm, {len(osv)} osv")

    stale: list[str] = []
    if args.deny_json and args.deny_json.exists():
        for line in args.deny_json.read_text().splitlines():
            line = line.strip()
            if not line.startswith("{"):
                continue
            try:
                d = json.loads(line)
            except json.JSONDecodeError:
                continue
            f = d.get("fields") or {}
            if d.get("type") == "diagnostic" and f.get("code") == "advisory-not-detected":
                for label in f.get("labels") or []:
                    span = label.get("span") or ""
                    if RUSTSEC.fullmatch(span):
                        stale.append(f"{span}: deny.toml ignore no longer matches any crate (cargo-deny advisory-not-detected)")
    if args.advisory_db and args.advisory_db.exists():
        lock = lockfile_versions(ROOT / "Cargo.lock")
        # cargo-deny evaluates deny.toml's ids itself (above); these are
        # the ids only audit.toml carries, judged the way cargo-deny would:
        # stale when no lockfile version of the crate is still affected.
        for i in sorted(audit - deny):
            fm = advisory_front_matter(args.advisory_db, i)
            if fm is None:
                stale.append(f"{i}: audit.toml ignore for an advisory the RustSec db no longer has")
                continue
            crate, patched, unaffected = fm
            versions = lock.get(crate, [])
            if not versions:
                stale.append(f"{i}: audit.toml ignore for `{crate}`, which is no longer in Cargo.lock")
            elif not any(affected(v, patched, unaffected) for v in versions):
                stale.append(f"{i}: audit.toml ignore for `{crate}` {versions}, none of which the advisory still affects")
        # A GitHub-only id the RustSec db has caught up with: cargo-deny now
        # reports it under the RUSTSEC id, which nothing ignores yet.
        aliases = rustsec_aliases(args.advisory_db)
        for i in sorted(osv_only):
            if i in aliases:
                stale.append(f"{i}: `osv-only:` in deny.toml, but the RustSec db now has it as {aliases[i]} — "
                             f"move it into the deny.toml / audit.toml ignore arrays under that id")

    if stale:
        print("\nstale ignores (the fix landed upstream or the crate left the graph):")
        for s in stale:
            print(f"  - {s}")
        print("\nDelete the entry from every file that carries it, and check whether the pin "
              "that held the advisory in place can now move (the `Revisit when…` note on the entry). "
              "An `osv-only:` id the RustSec db caught up with moves instead: into the deny.toml / "
              "audit.toml ignore arrays under its RUSTSEC id, keeping its osv-scanner.toml entry.")
        return 2
    if args.deny_json or args.advisory_db:
        print("no stale ignores")
    return 0


if __name__ == "__main__":
    sys.exit(main())
