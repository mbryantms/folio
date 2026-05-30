import { Loader2 } from "lucide-react";

import { LoadingWatchdog } from "@/components/LoadingWatchdog";

/**
 * Reader-route Suspense fallback. The reader's server component fans out
 * to several API calls (issue detail → progress → prefs) before it can
 * mount, and the reader lives *outside* the `(library)` group, so without
 * this file the route inherits `[locale]/loading.tsx`. Previously that
 * meant the legacy `Chrome` shell + a near-white cover grid — which
 * flashed white on light-theme iPads and looked nothing like the reader.
 *
 * Instead, mirror the reader shell: the same `--reader-bg` surface, a
 * faux top-chrome silhouette (matching `ReaderChrome`'s safe-area
 * padding), and the same centered `Loader2` the per-page `PageImage`
 * shows — so the hand-off into the first page is continuous.
 *
 * Server component — pure markup.
 */
export default function ReaderLoading() {
  return (
    <div
      className="bg-reader-bg min-h-screen text-neutral-200"
      role="status"
      aria-live="polite"
    >
      <LoadingWatchdog />
      {/* Faux chrome bar — same height/padding contract as ReaderChrome
          so the real bar slides in over the same footprint. */}
      <div className="bg-reader-chrome/85 fixed inset-x-0 top-0 z-30 flex items-center gap-3 border-b border-neutral-800/80 px-[max(0.75rem,var(--safe-left))] pt-[max(0.5rem,var(--safe-top))] pb-2 backdrop-blur">
        <div className="h-6 w-6 animate-pulse rounded bg-neutral-800" />
        <div className="h-4 w-40 animate-pulse rounded bg-neutral-800" />
        <div className="ml-auto h-6 w-6 animate-pulse rounded bg-neutral-800" />
      </div>
      <div className="grid min-h-screen place-items-center">
        <Loader2
          aria-hidden
          className="size-8 animate-spin text-neutral-600 motion-reduce:hidden"
        />
      </div>
      <span className="sr-only">Loading reader…</span>
    </div>
  );
}
