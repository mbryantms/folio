"use client";

/**
 * `<CoverPagePickerDialog>` (post-audit T4.2) — pick which archive page is
 * the issue's cover. A grid of the issue's page thumbnails; the page in
 * force is outlined, clicking another one applies it. The parent owns the
 * mutation (`useSetIssueCoverPage`) so this stays a pure picker that unit
 * tests can render without a QueryClient.
 */

import { Check, Loader2 } from "lucide-react";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { pageThumbUrl } from "@/lib/urls";
import { cn } from "@/lib/utils";

export function CoverPagePickerDialog({
  open,
  onOpenChange,
  issueId,
  pageCount,
  currentPageIndex,
  onPick,
  isPending = false,
}: {
  open: boolean;
  onOpenChange: (next: boolean) => void;
  issueId: string;
  /** Pages in the archive; the grid shows one tile per page. */
  pageCount: number;
  /** The cover page in force (`issues.cover_page_index`). */
  currentPageIndex: number;
  /** Apply the pick. The parent closes the dialog on success. */
  onPick: (pageIndex: number) => void;
  isPending?: boolean;
}) {
  const pages = Array.from({ length: Math.max(0, pageCount) }, (_, i) => i);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>Choose cover page</DialogTitle>
          <DialogDescription>
            The chosen page becomes this issue&apos;s cover everywhere. It stays
            put on rescans; on a writeback library the archive&apos;s ComicInfo
            is updated too.
          </DialogDescription>
        </DialogHeader>
        {pages.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            This issue has no pages to choose from.
          </p>
        ) : (
          <ul
            role="list"
            aria-label="Pages"
            aria-busy={isPending || undefined}
            className="grid grid-cols-3 gap-3 sm:grid-cols-4 md:grid-cols-5"
          >
            {pages.map((n) => {
              const current = n === currentPageIndex;
              return (
                <li key={n}>
                  <button
                    type="button"
                    onClick={() => onPick(n)}
                    disabled={isPending || current}
                    aria-pressed={current}
                    aria-label={
                      current
                        ? `Page ${n + 1} (current cover)`
                        : `Page ${n + 1}`
                    }
                    className={cn(
                      "group bg-muted/30 focus-visible:ring-ring relative block w-full overflow-hidden rounded-md",
                      "focus-visible:ring-2 focus-visible:outline-none",
                      current
                        ? "ring-primary ring-2"
                        : "ring-border/60 hover:ring-primary/60 ring-1",
                      isPending && !current && "opacity-60",
                    )}
                  >
                    {/* eslint-disable-next-line @next/next/no-img-element -- page thumbs are same-origin API bytes */}
                    <img
                      src={`${pageThumbUrl(issueId, n)}?variant=strip`}
                      alt=""
                      loading="lazy"
                      decoding="async"
                      className="aspect-[2/3] w-full object-cover"
                    />
                    <span className="bg-background/85 absolute bottom-1 left-1 rounded px-1.5 py-0.5 text-xs tabular-nums backdrop-blur">
                      {n + 1}
                    </span>
                    {current ? (
                      <span className="bg-primary text-primary-foreground absolute top-1 right-1 inline-flex h-6 w-6 items-center justify-center rounded-full">
                        <Check className="h-3.5 w-3.5" aria-hidden="true" />
                      </span>
                    ) : null}
                  </button>
                </li>
              );
            })}
          </ul>
        )}
        {isPending ? (
          <p className="text-muted-foreground flex items-center gap-2 text-xs">
            <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
            Applying…
          </p>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}
