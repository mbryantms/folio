import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

/**
 * Server route files may *render* components exported from a `"use client"`
 * module, but must not call or read anything else from one: across the
 * boundary every export is a client reference, so `parseX()` throws
 * "Attempted to call parseX() from the server" at request time and the
 * route lands on its error boundary. Unit tests render the client
 * component directly and never see this, which is how `/browse` shipped
 * broken (its page called `parseBrowseTab` from `BrowseTabs.tsx`).
 *
 * Rule enforced here: a server route file (`page` / `layout` / `template`
 * / `default` / `not-found` without `"use client"`) may only import
 * PascalCase component names (or `type`-only names) from a `"use client"`
 * module. Server-safe helpers belong in a module without the directive
 * (e.g. `library-grid-filters.ts`, `browse-tabs.ts`).
 */

const WEB = path.resolve(import.meta.dirname, "..");
const APP = path.join(WEB, "app");
const ROUTE_FILE = /^(page|layout|template|default|not-found)\.tsx?$/;

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = path.join(dir, name);
    if (statSync(p).isDirectory()) walk(p, out);
    else if (ROUTE_FILE.test(name)) out.push(p);
  }
  return out;
}

function isClientModule(src: string): boolean {
  // The directive must be the first statement; skip leading comments.
  const body = src
    .replace(/^\s*(\/\/[^\n]*\n|\/\*[\s\S]*?\*\/)\s*/g, "")
    .trimStart();
  return /^["']use client["']/.test(body);
}

function resolveImport(from: string, spec: string): string | null {
  let base: string;
  if (spec.startsWith("@/")) base = path.join(WEB, spec.slice(2));
  else if (spec.startsWith(".")) base = path.resolve(path.dirname(from), spec);
  else return null; // package import
  for (const cand of [
    base,
    `${base}.tsx`,
    `${base}.ts`,
    path.join(base, "index.tsx"),
    path.join(base, "index.ts"),
  ]) {
    if (existsSync(cand) && statSync(cand).isFile()) return cand;
  }
  return null;
}

const IMPORT_RE = /import\s+(type\s+)?\{([^}]*)\}\s+from\s+["']([^"']+)["']/g;
const COMPONENT_NAME = /^[A-Z][A-Za-z0-9]*$/;
const CONSTANT_NAME = /^[A-Z][A-Z0-9_]+$/;

function violations(file: string): string[] {
  const src = readFileSync(file, "utf8");
  if (isClientModule(src)) return [];
  const found: string[] = [];
  for (const m of src.matchAll(IMPORT_RE)) {
    if (m[1]) continue; // `import type { … }`
    const target = resolveImport(file, m[3]!);
    if (!target || !isClientModule(readFileSync(target, "utf8"))) continue;
    for (const raw of m[2]!.split(",")) {
      const part = raw.trim();
      if (!part || part.startsWith("type ")) continue;
      const local = part.split(/\s+as\s+/)[0]!.trim();
      if (COMPONENT_NAME.test(local) && !CONSTANT_NAME.test(local)) continue;
      found.push(`${path.relative(WEB, file)}: \`${local}\` from ${m[3]}`);
    }
  }
  return found;
}

describe("server/client boundary", () => {
  it("server route files import only components from 'use client' modules", () => {
    const files = walk(APP);
    expect(files.length).toBeGreaterThan(20); // the walk found the routes
    const bad = files.flatMap(violations);
    expect(bad).toEqual([]);
  });
});
