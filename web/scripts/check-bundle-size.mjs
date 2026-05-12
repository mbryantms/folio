#!/usr/bin/env node
/* eslint-disable no-console -- CLI script: console output is the user-facing surface. */
/**
 * Reader bundle budget gate (§18.1).
 *
 * Phase 2 budget: `/[locale]/read/[id]` First Load JS ≤ 150 KB gzip.
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
const ROUTE_DIR = "[locale]/read/[id]";
const ROUTE_LABEL = "/[locale]/read/[id]";
const BUDGET_KB = 150;
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
    `${ROUTE_LABEL} First Load JS: ${kb.toFixed(2)} KB gzip (${chunkPaths.size} chunks, budget ${BUDGET_KB} KB)`,
  );
  if (kb > BUDGET_KB) {
    fail(`Bundle budget exceeded: ${kb.toFixed(2)} KB > ${BUDGET_KB} KB`);
  }
}

if (!existsSync(resolve(WEB_DIR, "package.json"))) {
  fail("must run from web/ root");
}
scanForbiddenImports();
checkBuildSize();
console.log("Bundle budget OK ✓");
