#!/usr/bin/env node
/* eslint-disable no-console -- CLI script: console output is the user-facing surface. */
/**
 * Status-color contrast gate (audit F1/F2).
 *
 * Bare light-shade status text — `text-emerald-300`, `text-amber-200`,
 * `text-red-300`, `text-green-200` — is tuned for the dark theme and
 * renders near-invisible on the light/amber themes when it has no
 * `dark:`-qualified counterpart. Use the semantic tokens via
 * `statusToneText()` / `statusTone()` (see `lib/ui/status-tone.ts`),
 * which resolve correctly on all three themes.
 *
 * This flags any raw palette class (`text-emerald-300`, `bg-amber-500`,
 * `border-violet-500/40`, …) that isn't paired with a `dark:` variant on
 * the same line (the presence of `dark:` signals an intentional
 * light-theme fallback was provided).
 *
 * Excluded:
 *   - the status-tone helper itself,
 *   - the reader route (`app/[locale]/read/**`) — its chrome is a
 *     deliberate always-dark surface (`--reader-bg` is theme-independent
 *     black) with its own `neutral-*` palette, so dark-tuned accents
 *     there are correct by construction.
 *
 * Run: `pnpm --filter web run check-status-colors`
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import { resolve, dirname, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const WEB_DIR = resolve(__dirname, "..");
const SCAN_DIRS = ["app", "components", "lib"];
// Any raw Tailwind palette class (widened 2026-10-26 from the four
// dark-tuned shades the first sweep targeted). Tokens — `text-success`,
// `bg-warning`, `text-info`, `bg-destructive`, `bg-primary/30` — follow
// every theme; a palette shade follows none of them.
const PATTERN =
  /\b(?:text|bg|border|ring|fill|stroke|from|to|via|outline|decoration|shadow)-(?:red|green|emerald|amber|blue|rose|yellow|orange|sky|zinc|gray|slate|neutral|stone|indigo|violet|purple|pink|lime|teal|cyan|fuchsia)-[0-9]{2,3}\b/;
const READER_PREFIX = join("app", "[locale]", "read") + sep;
const ALLOWLIST = new Set([
  // The tone helper itself.
  join("lib", "ui", "status-tone.ts"),
  // Accent swatches: each dot IS a palette colour by definition.
  join("components", "settings", "ThemePicker.tsx"),
  // Marker colours are a fixed per-marker palette, not status tones.
  join("components", "markers", "MarkersList.tsx"),
  // The offline reader is a reader surface (always dark), like app/read.
  join("app", "[locale]", "downloads", "OfflineReader.tsx"),
]);

function* walk(dir) {
  for (const name of readdirSync(dir)) {
    if (name === "node_modules" || name === ".next") continue;
    const abs = join(dir, name);
    const st = statSync(abs);
    if (st.isDirectory()) yield* walk(abs);
    else if (/\.tsx?$/.test(name)) yield abs;
  }
}

const violations = [];
for (const dir of SCAN_DIRS) {
  for (const abs of walk(join(WEB_DIR, dir))) {
    const rel = relative(WEB_DIR, abs);
    if (ALLOWLIST.has(rel) || rel.startsWith(READER_PREFIX)) continue;
    const lines = readFileSync(abs, "utf8").split("\n");
    lines.forEach((line, i) => {
      if (PATTERN.test(line) && !line.includes("dark:")) {
        violations.push(`${rel}:${i + 1}  ${line.trim().slice(0, 100)}`);
      }
    });
  }
}

if (violations.length > 0) {
  console.error(
    "::error::Bare light-shade status text has no light-theme fallback " +
      "(invisible on light/amber). Use statusToneText()/statusTone() from " +
      "lib/ui/status-tone.ts, or pair a `dark:` variant for categorical hues.\n  " +
      violations.join("\n  "),
  );
  process.exit(1);
}
console.log(
  "Status-color contrast gate OK ✓ (no bare light-shade status text outside the reader chrome)",
);
