#!/usr/bin/env python3
"""Regenerate `web/lib/api/types.ts` as a hybrid alias shim.

This file used to be a 2392-line hand-curated mirror of the Rust API surface.
After audit-remediation M1c (2026-05-23), most types are codegen-aliased
over `components["schemas"]["X"]` from `types.generated.ts`; types that
don't have a codegen equivalent (typed enums, frontend-only constructs,
WS payloads, Rust types not yet derived for `ToSchema`) stay inline.

When to run: when a type is added to or renamed in the Rust API, run
`just openapi` (regenerates `openapi.json` + `types.generated.ts`), then
optionally run this script to refresh the alias roster. The script is
deliberately conservative — it only rewrites the file if the alias/inline
partition would change."""
from __future__ import annotations

import re
from pathlib import Path

REPO = Path("/home/matthew/Documents/folio")
HAND = REPO / "web/lib/api/types.ts"
GEN = REPO / "web/lib/api/types.generated.ts"
OUT = HAND

# Hand-written name → codegen schema name when they differ.
NAMING_MISMATCH = {
    "MeView": "MeResp",
    "FsDirEntry": "DirEntry",
    "FsListResp": "ListResp",
    "AdminOverviewView": "OverviewView",
    "SettingResolvedEntry": "ResolvedEntry",
    "SettingRegistryEntry": "RegistryEntry",
    "PatchLogWidgetReq": "UpdateWidgetReq",
    "AddLogWidgetReq": "AddWidgetReq",
    "CreateRailDismissalReq": "CreateDismissalReq",
    "ReadingLogEventIssue": "EventIssue",
    "ReadingLogEventSeries": "EventSeries",
    "ReadingLogPayload": "EventPayload",
    "ReorderLogWidgetsReq": "ReorderWidgetsReq",
    "ReadingDayBucket": "DayBucket",
    "ReadingSessionUpsertReq": "UpsertReadingSessionReq",
}


def codegen_schema_names() -> set[str]:
    text = GEN.read_text()
    schemas_start = text.find("    schemas: {")
    if schemas_start < 0:
        raise RuntimeError("couldn't find `schemas:` block in codegen output")
    names: set[str] = set()
    depth = 0
    started = False
    for line in text[schemas_start:].splitlines():
        if line == "    schemas: {":
            started = True
            depth = 1
            continue
        if started:
            stripped = line.lstrip()
            depth += stripped.count("{") - stripped.count("}")
            m = re.match(r"^        ([A-Z][A-Za-z0-9_]+):", line)
            if m and depth >= 1:
                names.add(m.group(1))
            if depth <= 0:
                break
    return names


def parse_handwritten_decls(text: str) -> list[tuple[str, str]]:
    """Return list of (name, full_raw_decl_text). Splits the file at every
    line starting with `^export (type|interface) NAME`; the declaration's
    body is everything from that line up to the NEXT `^export ...` (or EOF).
    """
    lines = text.split("\n")
    anchors: list[tuple[int, str]] = []
    pat = re.compile(r"^export (?:type|interface) (\w+)")
    for i, line in enumerate(lines):
        m = pat.match(line)
        if m:
            anchors.append((i, m.group(1)))

    decls: list[tuple[str, str]] = []
    for idx, (line_no, name) in enumerate(anchors):
        end_line = anchors[idx + 1][0] if idx + 1 < len(anchors) else len(lines)
        # Trim trailing blank lines AND any leading doc-comment lines that
        # actually belong to the next anchor. Walk backwards from end_line.
        decl_lines = lines[line_no:end_line]
        # Strip trailing blank lines.
        while decl_lines and decl_lines[-1].strip() == "":
            decl_lines.pop()
        # Walk backwards to remove a JSDoc / line-comment block that
        # immediately precedes the next decl (it's the next decl's prelude,
        # not part of this one).
        cut = len(decl_lines)
        while cut > 0:
            line = decl_lines[cut - 1].rstrip()
            stripped = line.lstrip()
            # If we hit a closing-brace / semicolon-terminated line, that's
            # the real end of this declaration; stop trimming.
            if (
                line.endswith(";")
                or line.endswith("}")
                or line.endswith("]")
                or line.endswith(",")
                or (stripped and not stripped.startswith("//") and not stripped.startswith("/*") and not stripped.startswith("*"))
            ):
                break
            cut -= 1
        body = "\n".join(decl_lines[:cut])
        decls.append((name, body))
    return decls


def build_shim() -> str:
    codegen = codegen_schema_names()
    hand_text = HAND.read_text()
    decls = parse_handwritten_decls(hand_text)

    aliased: list[tuple[str, str]] = []  # (fe_name, gen_name)
    inlined: list[tuple[str, str]] = []  # (name, raw)
    for name, raw in decls:
        if name in codegen:
            aliased.append((name, name))
        elif name in NAMING_MISMATCH and NAMING_MISMATCH[name] in codegen:
            aliased.append((name, NAMING_MISMATCH[name]))
        else:
            inlined.append((name, raw))

    header = '''/**
 * **Hybrid alias shim (audit-remediation M1b).** This file used to be a
 * 2392-line hand-curated mirror of the Rust API surface. As of M1b,
 * the codegen output at [./types.generated.ts](./types.generated.ts)
 * is the source of truth for every type that has a `#[derive(ToSchema)]`
 * Rust struct/enum reachable from a `#[utoipa::path]` handler — those
 * are exported here as one-line aliases over `components["schemas"]["X"]`.
 *
 * The remaining hand-written entries below are types that *do not yet*
 * appear in the generated spec:
 *   - **Frontend-only constructs** (computed unions, intersection types,
 *     WS payload shapes the spec doesn't cover): kept inline by design.
 *   - **Rust types not yet derived for `ToSchema`** (marker kind/shape,
 *     log-widget kind, filter/sort enums): kept inline as a TODO — each
 *     migration follows the preferences-enum pattern in
 *     `crates/server/src/auth/preferences.rs`.
 *
 * Drift gate: `just openapi-check` regenerates both `openapi.json` and
 * `types.generated.ts` and fails the build if either disagrees with
 * the checked-in copy. Add new types by writing the Rust `ToSchema`,
 * running `just openapi`, then aliasing here.
 */

import type { components } from "./types.generated";

type Schemas = components["schemas"];

// ────────────── Aliased from codegen ──────────────
// Hand-edits to the *aliased* block below will be silently overridden by
// the next `just openapi` when the Rust source moves. To change one of
// these shapes, change the Rust DTO.
'''

    body_lines = []
    for fe, gen in aliased:
        if fe == gen:
            body_lines.append(f'export type {fe} = Schemas["{gen}"];')
        else:
            body_lines.append(
                f'export type {fe} = Schemas["{gen}"]; // renamed in codegen as {gen}'
            )

    body_lines.append("")
    body_lines.append("// ────────────── Frontend-only / not yet derived for ToSchema ──────────────")
    body_lines.append("// Each entry below is either a frontend-only computed type or a Rust type")
    body_lines.append("// that hasn't been wired into the OpenAPI spec yet. When the Rust source")
    body_lines.append("// derives ToSchema, move the corresponding entry up into the aliased block.")
    body_lines.append("")

    for name, raw in inlined:
        body_lines.append(raw)
        body_lines.append("")

    return header + "\n".join(body_lines) + "\n"


if __name__ == "__main__":
    new = build_shim()
    OUT.write_text(new)
    aliased = new.count('Schemas["')
    inlined = sum(1 for ln in new.splitlines() if ln.startswith("export type") or ln.startswith("export interface"))
    print(f"wrote {OUT}: {len(new)} bytes")
    print(f"  aliased: {aliased}")
    print(f"  total exports (incl. inline): {inlined}")
