import { Chrome } from "@/components/Chrome";

/**
 * App Router loading fallback for `[locale]/`. Renders the page shell
 * (sidebar + topbar via `<Chrome>`) with a skeleton-card grid so the
 * first paint after a route transition isn't a blank canvas.
 * Audit-remediation M7.1 (2026-05-24).
 *
 * Note: this is a server component — no `"use client"` needed; the
 * shell + skeletons are pure markup. Per-section loading.tsx files
 * under `(library)/(admin)/(settings)` can render tighter skeletons.
 */
export default function LocaleLoading() {
  return (
    <Chrome breadcrumbs={[{ label: "Loading…" }]}>
      <div className="py-6">
        <div className="grid grid-cols-2 gap-4 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6">
          {Array.from({ length: 12 }, (_, i) => (
            <div
              key={i}
              className="aspect-[2/3] animate-pulse rounded-md bg-muted/40"
              aria-hidden
            />
          ))}
        </div>
        <span className="sr-only">Loading…</span>
      </div>
    </Chrome>
  );
}
