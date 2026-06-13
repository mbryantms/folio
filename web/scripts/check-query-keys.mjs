#!/usr/bin/env node
/* eslint-disable no-console -- CLI script: console output is the user-facing surface. */
/**
 * Query-key registry gate (audit H1a).
 *
 * Every TanStack query *definition* — `useQuery` / `useInfiniteQuery` /
 * `useSuspenseQuery` — must take its `queryKey` from the `queryKeys`
 * registry in `lib/api/query-keys.ts`, never an inline tuple literal.
 * Inlining a key lets a hook's cache key drift from the keys mutations
 * invalidate against, which silently breaks refetch-after-write.
 *
 * This scans for the anti-pattern: a `use*Query(` call whose options
 * object assigns `queryKey:` directly to an array literal (`[`). It is
 * scoped to query *definitions* on purpose — `invalidateQueries` /
 * `cancelQueries` / `setQueryData` legitimately pass partial-prefix
 * tuples (e.g. `["series"]`) and are out of scope here (their
 * consolidation is tracked separately).
 *
 * The registry file itself is the one allowed home for key tuples.
 *
 * Run: `pnpm --filter web run check-query-keys`
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import { resolve, dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const WEB_DIR = resolve(__dirname, "..");
const SCAN_DIRS = ["app", "components", "lib"];
const ALLOWLIST = new Set([
  // The registry — the single legitimate home for key tuples.
  "lib/api/query-keys.ts",
]);
const HOOK_RE = /\buse(?:Infinite|Suspense)?Query\s*\(/g;

/** Walk a dir for .ts/.tsx files, skipping node_modules + .next. */
function* walk(dir) {
  for (const name of readdirSync(dir)) {
    if (name === "node_modules" || name === ".next") continue;
    const abs = join(dir, name);
    const st = statSync(abs);
    if (st.isDirectory()) yield* walk(abs);
    else if (/\.tsx?$/.test(name)) yield abs;
  }
}

/**
 * From the index just after a `use*Query(` match, find the `queryKey:`
 * property at the top level of that call's options object and report
 * whether its value is an inline array literal. Returns the 1-based line
 * of the offending `queryKey:` or null when the key isn't inline.
 *
 * Brace/paren/bracket-depth aware so a nested object's `queryKey` (there
 * are none today, but be safe) or a later call doesn't confuse it; stops
 * at the close of the call's argument list.
 */
function inlineKeyLine(src, start) {
  let depth = 0; // net (){}[] depth relative to the call's open paren
  for (let i = start; i < src.length; i++) {
    const c = src[i];
    if (c === "(" || c === "{" || c === "[") depth++;
    else if (c === ")" || c === "}" || c === "]") {
      depth--;
      if (depth < 0) return null; // closed the call args; no inline key
    } else if (depth === 1 && src.startsWith("queryKey", i)) {
      // At the options-object top level (depth 1 = inside the single
      // options object passed to the hook). Read past `queryKey` + ws + `:`.
      let j = i + "queryKey".length;
      while (j < src.length && /\s/.test(src[j])) j++;
      if (src[j] !== ":") continue;
      j++;
      while (j < src.length && /\s/.test(src[j])) j++;
      // Inline tuple → violation. `queryKeys.x(...)` or a variable → ok.
      if (src[j] === "[") {
        return src.slice(0, i).split("\n").length;
      }
      return null;
    }
  }
  return null;
}

const violations = [];
for (const dir of SCAN_DIRS) {
  const root = join(WEB_DIR, dir);
  for (const abs of walk(root)) {
    const rel = relative(WEB_DIR, abs);
    if (ALLOWLIST.has(rel)) continue;
    const src = readFileSync(abs, "utf8");
    for (const m of src.matchAll(HOOK_RE)) {
      const line = inlineKeyLine(src, m.index + m[0].length);
      if (line) violations.push(`${rel}:${line}`);
    }
  }
}

if (violations.length > 0) {
  console.error(
    "::error::Query definitions must use the queryKeys registry, not an inline tuple.\n" +
      "Add a key factory to web/lib/api/query-keys.ts and reference it.\n" +
      "Offending use*Query queryKey literals:\n  " +
      violations.join("\n  "),
  );
  process.exit(1);
}
console.log(
  `Query-key registry gate OK ✓ (no inline queryKey tuples in ${SCAN_DIRS.join("/")} query definitions)`,
);
