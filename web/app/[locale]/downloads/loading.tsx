import { Skeleton } from "@/components/ui/skeleton";

/**
 * `/downloads` fallback, shaped like `OfflineLibrary`: the header, the
 * storage summary and a card of download rows (cover thumb, title,
 * progress), instead of the full-screen spinner. Static markup with no
 * client modules (not even the shared `LoadingShapes`, which pulls in the
 * issue card): the route must stay light and work offline.
 */
export default function DownloadsLoading() {
  return (
    <main
      aria-busy="true"
      className="mx-auto w-full max-w-3xl space-y-6 px-4 py-8 pb-[max(2rem,var(--safe-bottom))]"
    >
      <span className="sr-only" role="status">
        Loading downloads…
      </span>
      <div aria-hidden className="space-y-6">
        {/* Header: the description is long enough that the "Open
            library" button wraps below it, as on the real page. */}
        <div className="border-border flex flex-wrap items-end justify-between gap-4 border-b pb-4">
          <div className="w-full min-w-0">
            <div className="flex h-8 items-center">
              <Skeleton className="h-6 w-32" />
            </div>
            <div className="mt-1 flex h-5 items-center">
              <Skeleton className="h-3.5 w-[40rem] max-w-full" />
            </div>
          </div>
          <Skeleton className="h-9 w-24 rounded-md" />
        </div>
        <div className="space-y-2">
          <div className="flex h-5 items-center">
            <Skeleton className="h-3.5 w-36" />
          </div>
          <Skeleton className="h-1.5 w-full rounded-full" />
          <div className="flex h-4 items-center">
            <Skeleton className="h-2.5 w-[34rem] max-w-full" />
          </div>
        </div>
        <div className="border-border bg-card rounded-lg border">
          <div className="border-border space-y-2 border-b p-3">
            <Skeleton className="h-4 w-52" />
            <Skeleton className="h-3 w-32" />
          </div>
          <ul className="divide-border divide-y px-3">
            {Array.from({ length: 5 }, (_, i) => (
              <li key={i} className="flex items-center gap-3 py-3">
                <Skeleton className="h-16 w-11 shrink-0 rounded" />
                <div className="min-w-0 flex-1 space-y-2">
                  <Skeleton className={i % 2 ? "h-4 w-1/2" : "h-4 w-2/3"} />
                  <Skeleton className="h-1.5 w-full rounded-full" />
                </div>
                <Skeleton className="size-8 shrink-0 rounded-md" />
              </li>
            ))}
          </ul>
        </div>
      </div>
    </main>
  );
}
