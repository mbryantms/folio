import { Loader2 } from "lucide-react";

import { LoadingWatchdog } from "@/components/LoadingWatchdog";

/**
 * Top-level `[locale]/` Suspense fallback. Deliberately **shell-agnostic**:
 * at this level we don't yet know which area the user is entering
 * (library / admin / settings render `MainShell` / `AdminShell`; the
 * reader is full-screen and chrome-less), so committing to a shell here
 * guarantees a layout shift when the real one arrives.
 *
 * Each route group supplies its own `loading.tsx` that renders a
 * shape-matched skeleton *inside* the correct shell once its layout has
 * resolved. This fallback only covers the brief window before that —
 * a neutral, theme-aware full-screen spinner on `bg-background`.
 *
 * Server component — pure markup, no `"use client"`.
 */
export default function LocaleLoading() {
  return (
    <div
      className="bg-background grid min-h-screen place-items-center"
      role="status"
      aria-live="polite"
    >
      <LoadingWatchdog />
      <Loader2
        aria-hidden
        className="text-muted-foreground/60 size-8 animate-spin motion-reduce:hidden"
      />
      <span className="sr-only">Loading…</span>
    </div>
  );
}
