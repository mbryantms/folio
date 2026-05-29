import { Skeleton } from "@/components/ui/skeleton";

/**
 * `(settings)` group fallback. Renders inside `AdminShell` (settings nav).
 * Settings pages are `PageHeader` + form/section cards, not card grids —
 * mirror a header plus a couple of form cards with label/control rows.
 */
export default function SettingsLoading() {
  return (
    <div aria-hidden>
      {/* PageHeader */}
      <div className="border-border mb-6 flex flex-wrap items-end justify-between gap-4 border-b pb-4">
        <div className="min-w-0 space-y-2">
          <Skeleton className="h-8 w-48" />
          <Skeleton className="h-4 w-64" />
        </div>
      </div>

      <div className="max-w-2xl space-y-6">
        {Array.from({ length: 2 }, (_, card) => (
          <div
            key={card}
            className="border-border space-y-4 rounded-lg border p-5"
          >
            <Skeleton className="h-5 w-40" />
            {Array.from({ length: 3 }, (_, row) => (
              <div key={row} className="space-y-2">
                <Skeleton className="h-3 w-28" />
                <Skeleton className="h-9 w-full rounded-md" />
              </div>
            ))}
          </div>
        ))}
      </div>
      <span className="sr-only">Loading…</span>
    </div>
  );
}
