/**
 * Scan-activity consolidation (2026-11): the merged admin pages have one
 * URL each and the old pages were removed outright (no redirects). The
 * href builders are the canonical way to link to them, and no source file
 * may link to a removed page.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { describe, expect, it } from "vitest";

import {
  JOBS_HREF,
  libraryScanHref,
  scanBatchHref,
  SCANS_HREF,
  SERVER_LOG_HREF,
} from "@/lib/admin/routes";

describe("admin route hrefs", () => {
  it("builds the canonical scan / activity URLs", () => {
    expect(SCANS_HREF).toBe("/admin/scans");
    expect(scanBatchHref("abc")).toBe("/admin/scans?batch=abc");
    expect(libraryScanHref("my lib")).toBe("/admin/libraries/my%20lib/scan");
    expect(SERVER_LOG_HREF).toBe("/admin/activity?view=log");
    expect(JOBS_HREF).toBe("/admin/background-work?view=jobs");
  });
});

const WEB = resolve(__dirname, "../..");
const ROOTS = ["app", "components", "lib"].map((d) => join(WEB, d));

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return sources(path);
    return /\.(ts|tsx)$/.test(name) && !name.endsWith(".generated.ts")
      ? [path]
      : [];
  });
}

// Web page URLs that no longer exist. API paths (`/admin/logs` behind
// `jsonFetch`, `/admin/queue/jobs`, …) are matched only as page links:
// an href / router target / string that is exactly the old page.
const REMOVED = [
  /["'`]\/admin\/scan-dashboard/,
  /href=["'`{]+\/admin\/(logs|queue|audit)\b/,
  /["'`]\/admin\/(logs|queue|audit)(\?[^"'`]*)?["'`]/,
  /\/admin\/libraries\/\$\{[^}]+\}\/(history|thumbnails)["'`]/,
  /\/admin\/findings\?tab=scans/,
];

describe("no links to removed admin pages", () => {
  it.each(ROOTS.flatMap(sources).map((p) => [p.slice(WEB.length + 1), p]))(
    "%s",
    (_rel, path) => {
      const text = readFileSync(path, "utf8");
      // jsonFetch/apiMutate calls hit the JSON API, not a page.
      const pageLinks = text
        .split("\n")
        .filter((l) => !/jsonFetch|apiFetch|path:\s|queryFn/.test(l));
      for (const re of REMOVED) {
        expect(pageLinks.filter((l) => re.test(l))).toEqual([]);
      }
    },
  );
});
