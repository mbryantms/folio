#!/usr/bin/env node
/* eslint-disable no-console -- CLI script: console output is the user-facing surface. */
/**
 * Reader bundle budget gate (§18.1).
 *
 * Reader route: `/[locale]/read/[seriesSlug]/[issueSlug]` First Load JS.
 *
 * The §18.1 spec target is 150 KB gzip. As of 2026-05-20 the current
 * measured size is ~158 KB under Turbopack — no accidental large
 * imports, chunks are uniformly distributed; the growth is real
 * feature weight from marker mode, multi-page rails, and saved
 * views that landed after the spec.
 *
 * The gate fires on regressions, not on the current state. The intent
 * is to **ratchet this back down to the spec target** once we've
 * lazy-loaded the marker editor / saved-view picker — both of which
 * are only used on a subset of reader sessions yet ship in the
 * initial bundle today (chunk 2.5 of the frontend-audit plan).
 *
 * React Compiler bump (2026-06-13, chunk 1.0b): enabling the compiler
 * (`reactCompiler: true` in next.config.ts) adds ~25 KB of per-component
 * memoization-cache code to this route, taking the measured size from
 * ~168 KB to ~193 KB. The compiler is a net win — every component is
 * auto-memoized, so render hygiene stops regressing per-site — so the
 * ceiling was raised 170 → 195 KB to absorb it. This is deliberately a
 * one-way concession on the ceiling, NOT the target: BUDGET_TARGET_KB
 * stays at 150 and chunk 2.5's reader diet (webtoon windowing,
 * dynamic() MarkerEditor, lazy OCR) is expected to claw the compiler's
 * 25 KB back and let the ceiling ratchet down again.
 *
 * Bundler note: production builds use Turbopack (Next 16 default).
 * The PWA service worker is compiled in a separate post-`next build`
 * step via `@serwist/cli` (see `web/serwist.config.js`) because
 * `@serwist/next` requires Webpack and Webpack's chunk topology
 * roughly doubles the reader-bundle measurement (224 KB / 38 chunks)
 * for the same source code, blowing this ceiling cosmetically.
 *
 * Excluded libraries (must NOT appear in the chunk graph for this route):
 *   - framer-motion
 *   - @tiptap/*
 *   - @dnd-kit/*
 *
 * Approach: parse `.next/server/app/{route}/page_client-reference-manifest.js`
 * to enumerate the static chunks the route actually pulls in (its First Load
 * JS), then gzip-measure them. Next 16's webpack/Turbopack production output
 * dropped per-route sizes from the build summary, so we read the manifest the
 * builder writes anyway.
 */
import { execSync } from "node:child_process";
import { existsSync, readFileSync, statSync } from "node:fs";
import { gzipSync } from "node:zlib";
import { resolve, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const WEB_DIR = resolve(__dirname, "..");
const ROUTE_DIR = "[locale]/read/[seriesSlug]/[issueSlug]";
const ROUTE_LABEL = "/[locale]/read/[seriesSlug]/[issueSlug]";
/** Gate ceiling — regressions above this fail CI. Raised 170 → 195 to
 *  absorb the React Compiler's ~25 KB memoization-cache overhead
 *  (chunk 1.0b; see header). Track BUDGET_TARGET_KB for the spec
 *  number; chunk 2.5's reader diet ratchets this back down. */
const BUDGET_KB = 195;
/** §18.1 spec target. Documented separately so the spec number stays
 *  visible in build logs even while BUDGET_KB is temporarily relaxed. */
const BUDGET_TARGET_KB = 150;
const FORBIDDEN = ["framer-motion", "@tiptap", "@dnd-kit"];

function fail(msg) {
  console.error(`::error::${msg}`);
  process.exit(1);
}

// 1) Forbidden-import scan (ast-free, just text grep against the source).
function scanForbiddenImports() {
  const dirs = ["app/[locale]/read", "lib/reader", "workers"];
  for (const lib of FORBIDDEN) {
    const safe = lib.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const pattern = `from .${safe}`;
    const args = [
      "grep",
      "-REIn",
      "--include=*.ts",
      "--include=*.tsx",
      pattern,
      ...dirs,
    ];
    let hits = "";
    try {
      hits = execSync(
        args.map((a) => `'${a.replace(/'/g, "'\\''")}'`).join(" "),
        {
          cwd: WEB_DIR,
          encoding: "utf8",
        },
      ).trim();
    } catch (e) {
      // grep returns 1 when no matches — that's the success case here.
      if (e.status !== 1) throw e;
    }
    if (hits) {
      fail(`Reader bundle imports forbidden library "${lib}":\n${hits}`);
    }
  }
}

// 2) Build the project (if not already built) and measure the route's
//    First Load JS by summing gzipped chunk sizes.
function checkBuildSize() {
  const manifestPath = resolve(
    WEB_DIR,
    `.next/server/app/${ROUTE_DIR}/page_client-reference-manifest.js`,
  );
  if (!existsSync(manifestPath)) {
    console.log("Running `next build`…");
    execSync("npx next build", { cwd: WEB_DIR, stdio: "inherit" });
  }
  if (!existsSync(manifestPath)) {
    fail(`Manifest not found after build: ${manifestPath}`);
  }
  const manifest = readFileSync(manifestPath, "utf8");
  // Pull every "static/chunks/...js" path the route's manifest references.
  const chunkPaths = new Set(
    [...manifest.matchAll(/static\/chunks\/[^"]*?\.js/g)].map((m) => m[0]),
  );
  if (chunkPaths.size === 0) {
    fail(`No chunk paths found in manifest: ${manifestPath}`);
  }
  let totalGzipBytes = 0;
  const missing = [];
  for (const rel of chunkPaths) {
    const decoded = decodeURIComponent(rel);
    const abs = join(WEB_DIR, ".next", decoded);
    if (!existsSync(abs)) {
      missing.push(decoded);
      continue;
    }
    if (statSync(abs).isDirectory()) continue;
    const buf = readFileSync(abs);
    totalGzipBytes += gzipSync(buf).length;
  }
  if (missing.length > 0 && missing.length === chunkPaths.size) {
    fail(
      `All chunks missing from .next/. Was the build run? Missing:\n${missing.slice(0, 5).join("\n")}`,
    );
  }
  const kb = totalGzipBytes / 1024;
  console.log(
    `${ROUTE_LABEL} First Load JS: ${kb.toFixed(2)} KB gzip (${chunkPaths.size} chunks, ceiling ${BUDGET_KB} KB, §18.1 target ${BUDGET_TARGET_KB} KB)`,
  );
  if (kb > BUDGET_KB) {
    fail(
      `Bundle budget exceeded: ${kb.toFixed(2)} KB > ${BUDGET_KB} KB ceiling. The §18.1 target is ${BUDGET_TARGET_KB} KB; the ceiling is currently relaxed during ratchet-down — see the header comment in this file.`,
    );
  }
  if (kb > BUDGET_TARGET_KB) {
    // Non-fatal: the spec number isn't enforced yet, but we surface
    // the gap on every build so it stays in the contributor's sight
    // line. Ratchet BUDGET_KB down toward BUDGET_TARGET_KB in 1.0.x.
    console.log(
      `::warning::Bundle is ${(kb - BUDGET_TARGET_KB).toFixed(2)} KB above the §18.1 target (${BUDGET_TARGET_KB} KB). Ratchet planned in 1.0.x.`,
    );
  }
}

if (!existsSync(resolve(WEB_DIR, "package.json"))) {
  fail("must run from web/ root");
}
scanForbiddenImports();
checkBuildSize();
console.log("Bundle budget OK ✓");
