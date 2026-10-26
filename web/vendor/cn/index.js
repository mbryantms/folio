// The files under web/components/ui are vendored verbatim from the shadcn
// registry and import `cn` from the bare specifier "cn". The published `cn`
// package (0.5.0) ships a compiled class table that weighs ~10.4 KB gzip
// and landed in two reader chunks, pushing the reader first-load gate from
// 143.6 KB to 152.3 KB (2026-10-26). This workspace package shadows it
// with the same signature on top of clsx + tailwind-merge, so every tool
// (tsc, vitest, Next's webpack dev path and Turbopack prod build) resolves
// the import through node_modules with no alias config.
import { clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs) {
  return twMerge(clsx(inputs));
}
