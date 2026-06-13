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
 * This flags any `text-(emerald|amber|red|green)-(200|300)` that isn't
 * paired with a `dark:` variant on the same line (the presence of
 * `dark:` signals an intentional light-theme fallback was provided).
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
const PATTERN = /text-(?:emerald|amber|red|green)-(?:200|300)\b/;
const READER_PREFIX = join("app", "[locale]", "read") + sep;
const ALLOWLIST = new Set([join("lib", "ui", "status-tone.ts")]);

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
