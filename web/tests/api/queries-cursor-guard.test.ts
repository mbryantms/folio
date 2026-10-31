/**
 * Source guard for the CLAUDE.md "List pagination" rule: a `useQuery` on a
 * response that carries `next_cursor` is an infinite query in disguise —
 * it renders page one and silently drops the rest. The 2026-11 audit
 * (N-O7) found three such hooks in `web/lib/api/queries.ts` (`useScanRuns`,
 * `useScanBatches`, `useAdminHealthIssues`); this test keeps the class from
 * returning.
 *
 * How it works: the set of cursor-carrying response types is derived from
 * the generated OpenAPI types (`types.generated.ts`, every schema with a
 * `next_cursor` field) plus the `types.ts` aliases onto them. Every
 * top-level hook in `queries.ts` (`export function useX` or
 * `export const useX =`) that calls `useQuery` / `useQueries` and whose
 * code (comments stripped) names one of those types, or spells
 * `next_cursor` in an inline shape, is an offender unless it is in
 * ALLOWLIST with a reason. The allowlist is checked both ways, so a stale
 * entry fails too.
 *
 * Legitimate `useQuery` uses of a cursor type, for the allowlist:
 *  - a summary consumer that reads only `counts` / the first row (and says
 *    so), or probes `limit=1`;
 *  - caller-driven paging where the UI exposes the cursor itself
 *    (prev/next table with `keepPreviousData`), so nothing is hidden;
 *  - a bounded endpoint whose `next_cursor` is always null by domain.
 */
import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

const WEB = path.resolve(import.meta.dirname, "..", "..");
const read = (rel: string) => readFileSync(path.join(WEB, rel), "utf8");

/** Hooks that fetch a cursor-carrying type with `useQuery` on purpose. */
const ALLOWLIST: Record<string, string> = {
  useHealthIssues:
    "overview / live-scan summary: reads `counts` only; the table is useHealthIssuesInfinite",
  useLatestScanRun:
    "limit=1 probe for the newest run (overview card, live-scan hydrate); the table is useScanRuns",
  useAdminScanRuns:
    "dashboard 'Recent scan failures' card (limit 5, 7-day window); the table is useAdminScanRunsInfinite",
  useRemovedPending: "limit=1 probe for the Removed tab badge",
  useUserList:
    "admin users table: caller-driven cursor paging, keepPreviousData",
  useAuditLog:
    "admin audit table: caller-driven cursor paging, keepPreviousData",
  useSeriesList:
    "series list: caller-driven cursor paging, keepPreviousData (grid walks via its own infinite hook)",
  useMarkers: "markers list: caller-driven cursor paging, keepPreviousData",
  useCollectionEntries:
    "collection entries: caller-driven cursor paging; the reorder surface walks all pages first",
  useSavedViewResults: "saved-view results: caller passes the cursor",
  useSavedViewIssueResults:
    "saved-view issue results: single bounded page by design",
  useCblListIssues: "CBL matched issues: offset-paged by the caller",
  useLogWidgets: "per-user widgets: bounded, next_cursor is always null",
  useMePages: "per-user pages: bounded, next_cursor is always null",
  useAdminUsersStats:
    "one row per registered user; the server returns the whole set (admin_stats.rs), next_cursor is always null",
  useEntitySearch:
    "global-search source: the first page of the entity index by design; the browse index walks via useEntityListInfinite",
};

/** Schema names in the generated OpenAPI types whose object carries
 *  `next_cursor`. */
function cursorSchemas(generated: string): Set<string> {
  const out = new Set<string>();
  const re = /^ {8}(\w+): \{\n([\s\S]*?)^ {8}\};/gm;
  for (const m of generated.matchAll(re)) {
    if (/^\s+next_cursor\??:/m.test(m[2]!)) out.add(m[1]!);
  }
  return out;
}

/** `types.ts` names that alias (or inline) a cursor-carrying shape. */
function cursorAliases(typesTs: string, schemas: Set<string>): Set<string> {
  const out = new Set<string>();
  for (const m of typesTs.matchAll(
    /^export type (\w+) = Schemas\["(\w+)"\];/gm,
  )) {
    if (schemas.has(m[2]!)) out.add(m[1]!);
  }
  // Inline `export type X = { … next_cursor … }` (frontend-only shapes).
  for (const m of typesTs.matchAll(
    /^export (?:type|interface) (\w+)\b[^\n]*\{\n([\s\S]*?)^\};?/gm,
  )) {
    if (/^\s+next_cursor\??:/m.test(m[2]!)) out.add(m[1]!);
  }
  return out;
}

function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/[^\n]*/g, "");
}

/** Top-level statements of queries.ts, keyed by the declared name ("" for
 *  imports / types / anything unnamed). A block runs to the next column-0
 *  statement, so a hook never absorbs a neighbouring helper or a mid-file
 *  `import type { … }` that happens to name a cursor type. */
function topLevelBlocks(src: string): { name: string; body: string }[] {
  const starts = [...src.matchAll(/^[A-Za-z][^\n]*/gm)];
  const decl =
    /^(?:export\s+)?(?:async\s+)?(?:function\s+(\w+)|(?:const|let)\s+(\w+)\s*=)/;
  return starts.map((m, i) => {
    const d = decl.exec(m[0]);
    return {
      name: d ? (d[1] ?? d[2]!) : "",
      body: src.slice(m.index!, starts[i + 1]?.index ?? src.length),
    };
  });
}

function offenders(
  queries: string,
  cursorTypes: Set<string>,
): Map<string, string> {
  const found = new Map<string, string>();
  const typeRe = new RegExp(`\\b(${[...cursorTypes].join("|")})\\b`);
  for (const { name, body } of topLevelBlocks(queries)) {
    if (!name.startsWith("use")) continue;
    const code = stripComments(body);
    if (!/\buseQuer(?:y|ies)\s*[<(]/.test(code)) continue;
    const hit = /\bnext_cursor\b/.exec(code)?.[0] ?? typeRe.exec(code)?.[1];
    if (hit) found.set(name, hit);
  }
  return found;
}

describe("queries.ts: no useQuery over a next_cursor response", () => {
  const generated = read("lib/api/types.generated.ts");
  const typesTs = read("lib/api/types.ts");
  const queries = read("lib/api/queries.ts");

  const schemas = cursorSchemas(generated);
  const cursorTypes = new Set([...schemas, ...cursorAliases(typesTs, schemas)]);

  it("derives a non-trivial cursor-type list from the generated types", () => {
    // Sanity: the derivation must see the shapes the hooks actually fetch,
    // or the guard would pass vacuously.
    for (const t of [
      "CursorPage_ScanBatchView",
      "CursorPage_ScanRunView",
      "HealthIssuesPage",
      "CblEntryListView",
      "EntityListView",
    ]) {
      expect(cursorTypes.has(t), `missing ${t}`).toBe(true);
    }
    expect(cursorTypes.size).toBeGreaterThan(20);
  });

  it("sees both declaration styles and stops a block at the next declaration", () => {
    const blocks = topLevelBlocks(
      [
        "export function useA() {\n  return useQuery({ queryFn: () => jsonFetch<AListView>('/a') });\n}",
        "function helper(p: AListView) {\n  return p.next_cursor;\n}",
        "export const useB = () =>\n  useQuery({ queryFn: () => fetchB() });",
        'import type { AListView } from "./types";',
        "export function useC() {\n  return useInfiniteQuery({});\n}",
      ].join("\n\n"),
    );
    expect(blocks.map((b) => b.name)).toEqual([
      "useA",
      "helper",
      "useB",
      "",
      "useC",
    ]);
    const found = offenders(
      blocks.map((b) => b.body).join("\n\n"),
      new Set(["AListView"]),
    );
    // useA fetches a cursor type; useB's helper lives elsewhere and is not
    // typed here, so it is invisible to this guard (document that limit);
    // helper() is not a hook; the mid-file import names the type but is
    // its own block, so useB stays clean; useC is already infinite.
    expect([...found.keys()]).toEqual(["useA"]);
  });

  it("every useQuery over a cursor type is allowlisted with a reason", () => {
    const found = offenders(queries, cursorTypes);
    const unexpected = [...found].filter(([name]) => !(name in ALLOWLIST));
    expect(
      unexpected,
      "useQuery on a next_cursor response — make it useInfiniteQuery with a sentinel (CLAUDE.md 'List pagination'), or add it to ALLOWLIST with a reason",
    ).toEqual([]);
  });

  it("the allowlist carries no stale entries", () => {
    const found = offenders(queries, cursorTypes);
    const stale = Object.keys(ALLOWLIST).filter((name) => !found.has(name));
    expect(stale, "no longer a useQuery over a cursor type; drop it").toEqual(
      [],
    );
  });

  it("the three N-O7 hooks are infinite queries", () => {
    const blocks = topLevelBlocks(queries);
    for (const name of [
      "useScanRuns",
      "useScanBatches",
      "useAdminHealthIssues",
    ]) {
      const block = blocks.find((b) => b.name === name);
      expect(block, `${name} missing`).toBeDefined();
      expect(block!.body).toMatch(/\buseInfiniteQuery\(/);
      expect(block!.body).not.toMatch(/\buseQuery\(/);
    }
  });
});
