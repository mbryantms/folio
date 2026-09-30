#!/usr/bin/env node
/* eslint-disable no-console -- CLI script: console output is the user-facing surface. */
/**
 * Reader bundle budget gate (§18.1, WP-4.4).
 *
 * Reader route: `/[locale]/read/[seriesSlug]/[issueSlug]` First Load JS.
 *
 * What is measured: every `static/chunks/*.js` the route's
 * `page_client-reference-manifest.js` references — the root layout's
 * client components, the route's error / loading / not-found boundaries
 * and the page itself — gzipped (Node default level) and summed. The shared
 * framework bootstrap (`build-manifest.json` → `rootMainFiles`: React DOM,
 * the Next router runtime, polyfills) is NOT in that manifest and is not
 * counted; it is identical for every route and outside the reader's
 * control. Lazy `next/dynamic` chunks are not counted either — that is
 * the point of the gate.
 *
 * History:
 *   - 2026-05: ~158 KB under Turbopack, gate 170 KB.
 *   - 2026-06 (chunk 1.0b): React Compiler added ~25 KB → gate 195 KB.
 *   - 2026-09 (WP-4.4): code-split every reader surface not needed to
 *     paint page one (chrome, settings, page strip, marker overlay,
 *     end-of-issue card, webtoon footer, the global shortcuts sheet) and
 *     sharded the marker mutations out of the mutations barrel:
 *     191.2 KB / 20 chunks → 117.5 KB / 13 chunks. Gate re-set to
 *     measured + 10% = 130 KB. The pattern for new reader surfaces lives
 *     in `app/[locale]/read/[seriesSlug]/[issueSlug]/lazy.tsx`.
 *
 * Re-measure after any reader-feature WP lands and move BUDGET_KB to the
 * new measured + 10% only with a justification in
 * `docs/dev/pwa-performance.md` — never raise it silently.
 *
 * Bundler note: production builds use Turbopack (Next 16 default). The
 * PWA service worker is compiled in a separate post-`next build` step via
 * `@serwist/cli` (see `web/serwist.config.js`) because `@serwist/next`
 * requires Webpack, whose chunk topology measures ~50% heavier for the
 * same source.
 *
 * Excluded libraries (must NOT appear in reader sources):
 *   - framer-motion
 *   - @tiptap/*
 *   - @dnd-kit/*
 *
 * Usage:
 *   node scripts/check-bundle-size.mjs            # gate + per-chunk table
 *   node scripts/check-bundle-size.mjs --report <dir>
 *       also writes <dir>/reader-bundle.{json,md}. When
 *       `next experimental-analyze --output` has been run first, each
 *       chunk is annotated with its largest source modules (the CI
 *       artifact; see `.github/workflows/ci.yml`). With
 *       $GITHUB_STEP_SUMMARY set, the markdown is appended to the job
 *       summary as well.
 */
import { execSync } from "node:child_process";
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { gzipSync } from "node:zlib";
import { resolve, dirname, join, basename } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const WEB_DIR = resolve(__dirname, "..");
const ROUTE_DIR = "[locale]/read/[seriesSlug]/[issueSlug]";
const ROUTE_LABEL = "/[locale]/read/[seriesSlug]/[issueSlug]";
/** Gate ceiling — regressions above this fail CI. WP-4.4: measured
 *  117.5 KB + 10% (see header). */
const BUDGET_KB = 130;
/** WP-4.4 target. Exceeding it (while still under the ceiling) prints a
 *  warning so creep stays visible in every build log. */
const BUDGET_TARGET_KB = 120;
const FORBIDDEN = ["framer-motion", "@tiptap", "@dnd-kit"];
/** Modules listed per chunk in the report. */
const TOP_MODULES = 8;

function fail(msg) {
  console.error(`::error::${msg}`);
  process.exit(1);
}

function parseArgs(argv) {
  const out = { report: null };
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === "--report") {
      out.report = argv[i + 1];
      i += 1;
    }
  }
  return out;
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
function measureChunks() {
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
    [...manifest.matchAll(/static\/chunks\/[^"]*?\.js/g)].map((m) =>
      decodeURIComponent(m[0]),
    ),
  );
  if (chunkPaths.size === 0) {
    fail(`No chunk paths found in manifest: ${manifestPath}`);
  }
  const chunks = [];
  const missing = [];
  for (const rel of chunkPaths) {
    const abs = join(WEB_DIR, ".next", rel);
    if (!existsSync(abs)) {
      missing.push(rel);
      continue;
    }
    if (statSync(abs).isDirectory()) continue;
    const buf = readFileSync(abs);
    chunks.push({ path: rel, raw: buf.length, gzip: gzipSync(buf).length });
  }
  if (missing.length > 0 && missing.length === chunkPaths.size) {
    fail(
      `All chunks missing from .next/. Was the build run? Missing:\n${missing.slice(0, 5).join("\n")}`,
    );
  }
  chunks.sort((a, b) => b.gzip - a.gzip);
  return chunks;
}

// 3) Optional attribution from `next experimental-analyze --output`.
//    The analyzer re-compiles the app, so its chunk file names differ from
//    the build's; chunks are paired by raw byte size. Analyzer output
//    files carry a `\n//# sourceMappingURL=<13-char-hash>.js.map` trailer
//    the production build does not (43 bytes), hence the offset.
//    Module sizes are the analyzer's per-module compressed estimates.
const ANALYZER_TRAILER_BYTES = 43;

function readAnalyzerData() {
  const p = resolve(
    WEB_DIR,
    ".next/diagnostics/analyze/data",
    ROUTE_DIR,
    "analyze.data",
  );
  if (!existsSync(p)) return null;
  const buf = readFileSync(p);
  const len = buf.readUInt32BE(0);
  return JSON.parse(buf.subarray(4, 4 + len).toString("utf8"));
}

function moduleLabel(path) {
  const m =
    /node_modules\/(?:\.pnpm\/[^/]+\/node_modules\/)?((?:@[^/]+\/)?[^/]+)/.exec(
      path,
    );
  if (m) return m[1];
  return path.replace(/^\[project\]\/web\//, "");
}

function attribute(chunks, data) {
  const sources = data.sources;
  const memo = new Map();
  const fullPath = (i) => {
    if (memo.has(i)) return memo.get(i);
    const s = sources[i];
    const r =
      s.parent_source_index == null
        ? s.path
        : fullPath(s.parent_source_index) + s.path;
    memo.set(i, r);
    return r;
  };
  const byFile = new Map();
  for (const cp of data.chunk_parts) {
    const fn = data.output_files[cp.output_file_index].filename;
    if (!fn.includes("/static/chunks/") || !fn.endsWith(".js")) continue;
    if (!byFile.has(fn)) byFile.set(fn, { size: 0, parts: [] });
    const f = byFile.get(fn);
    f.size += cp.size;
    f.parts.push(cp);
  }
  const used = new Set();
  for (const chunk of chunks) {
    let best = null;
    let bestCost = Infinity;
    for (const [fn, f] of byFile) {
      if (used.has(fn)) continue;
      const cost = Math.abs(f.size - ANALYZER_TRAILER_BYTES - chunk.raw);
      if (cost < bestCost) {
        best = fn;
        bestCost = cost;
      }
    }
    if (!best || bestCost > Math.max(256, chunk.raw * 0.02)) continue;
    used.add(best);
    const agg = new Map();
    for (const cp of byFile.get(best).parts) {
      const label = moduleLabel(fullPath(cp.source_index));
      agg.set(label, (agg.get(label) ?? 0) + cp.compressed_size);
    }
    chunk.modules = [...agg.entries()]
      .sort((a, b) => b[1] - a[1])
      .slice(0, TOP_MODULES)
      .map(([name, bytes]) => ({ name, gzipApprox: bytes }));
  }
}

const kb = (bytes) => (bytes / 1024).toFixed(2);

function renderMarkdown(chunks, totalKb) {
  const lines = [
    `### Reader first-load JS — ${totalKb.toFixed(2)} KB gzip`,
    "",
    `Route \`${ROUTE_LABEL}\`, ${chunks.length} chunks, ceiling ${BUDGET_KB} KB, target ${BUDGET_TARGET_KB} KB.`,
    "",
    "| Chunk | gzip KB | raw KB | Largest modules (approx. gzip KB) |",
    "| --- | ---: | ---: | --- |",
  ];
  for (const c of chunks) {
    const mods = (c.modules ?? [])
      .map((m) => `${m.name} ${kb(m.gzipApprox)}`)
      .join(", ");
    lines.push(
      `| ${basename(c.path)} | ${kb(c.gzip)} | ${kb(c.raw)} | ${mods || "—"} |`,
    );
  }
  lines.push("");
  return lines.join("\n");
}

function main() {
  if (!existsSync(resolve(WEB_DIR, "package.json"))) {
    fail("must run from web/ root");
  }
  const args = parseArgs(process.argv.slice(2));
  scanForbiddenImports();
  const chunks = measureChunks();
  const totalKb = chunks.reduce((s, c) => s + c.gzip, 0) / 1024;

  for (const c of chunks) {
    console.log(
      `${kb(c.gzip).padStart(8)} KB gz ${kb(c.raw).padStart(8)} KB raw  ${c.path}`,
    );
  }
  console.log(
    `${ROUTE_LABEL} First Load JS: ${totalKb.toFixed(2)} KB gzip (${chunks.length} chunks, ceiling ${BUDGET_KB} KB, target ${BUDGET_TARGET_KB} KB)`,
  );

  if (args.report) {
    const data = readAnalyzerData();
    if (data) attribute(chunks, data);
    else
      console.log(
        "No analyzer data (.next/diagnostics/analyze) — run `next experimental-analyze --output` for per-module attribution.",
      );
    const dir = resolve(process.cwd(), args.report);
    mkdirSync(dir, { recursive: true });
    const md = renderMarkdown(chunks, totalKb);
    writeFileSync(
      join(dir, "reader-bundle.json"),
      JSON.stringify(
        {
          route: ROUTE_LABEL,
          totalGzipKb: Number(totalKb.toFixed(2)),
          budgetKb: BUDGET_KB,
          targetKb: BUDGET_TARGET_KB,
          chunks,
        },
        null,
        2,
      ),
    );
    writeFileSync(join(dir, "reader-bundle.md"), md);
    if (process.env.GITHUB_STEP_SUMMARY) {
      appendFileSync(process.env.GITHUB_STEP_SUMMARY, `\n${md}\n`);
    }
    console.log(`Report written to ${dir}`);
  }

  if (totalKb > BUDGET_KB) {
    fail(
      `Bundle budget exceeded: ${totalKb.toFixed(2)} KB > ${BUDGET_KB} KB ceiling. Lazy-load the new surface (see app/[locale]/read/[seriesSlug]/[issueSlug]/lazy.tsx) or justify a new ceiling in docs/dev/pwa-performance.md.`,
    );
  }
  if (totalKb > BUDGET_TARGET_KB) {
    // Non-fatal: surface creep above the target on every build.
    console.log(
      `::warning::Reader first-load JS is ${(totalKb - BUDGET_TARGET_KB).toFixed(2)} KB above the ${BUDGET_TARGET_KB} KB target.`,
    );
  }
  console.log("Bundle budget OK ✓");
}

main();
