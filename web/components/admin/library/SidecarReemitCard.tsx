"use client";

import Link from "next/link";
import { useState } from "react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { BACKGROUND_WORK_HREF, pct } from "@/lib/admin/background-work";
import { useStartSidecarReemit } from "@/lib/api/mutations";
import { useLibrary, useSidecarReemit } from "@/lib/api/queries";
import type { SidecarReemitRunView } from "@/lib/api/types";

const n = (v: number) => v.toLocaleString("en-US");

function isLive(run: SidecarReemitRunView | null | undefined): boolean {
  return run?.status === "running" || run?.status === "draining";
}

/** Live and still advancing. A stalled run (no tick for 15 minutes) can be
 *  superseded by starting a new one. */
function isActive(run: SidecarReemitRunView | null | undefined): boolean {
  return isLive(run) && !run?.stalled;
}

/**
 * "Rewrite metadata files" on the library Settings tab (audit 2026-11 A7):
 * re-emits the ComicInfo / MetronInfo sidecars of every issue from the
 * database with the current composer, so archives written by an older
 * build catch up. Only available when the library has both archive
 * writeback and metadata writeback on; the server enforces the same gate.
 */
export function SidecarReemitCard({ slug }: { slug: string }) {
  const { data: library } = useLibrary(slug);
  const writeback =
    !!library?.allow_archive_writeback && !!library?.metadata_writeback_enabled;
  const { data: status } = useSidecarReemit(slug, { enabled: writeback });
  const start = useStartSidecarReemit(slug);
  const [open, setOpen] = useState(false);
  const run = status?.run ?? null;
  const live = isActive(run);

  return (
    <Card>
      <CardContent className="space-y-3 p-5">
        <div>
          <h3 className="text-foreground text-sm font-semibold tracking-tight">
            Rewrite metadata files
          </h3>
          <p className="text-muted-foreground mt-1 text-xs">
            Writes fresh ComicInfo.xml and MetronInfo.xml into every archive in
            this library from the metadata Folio holds, using the current
            format. Use it after an upgrade changed what Folio writes. Pages are
            copied unchanged; CBR and CB7 archives are skipped unless conversion
            is on.
          </p>
          {!writeback ? (
            <p className="text-muted-foreground mt-2 text-xs">
              Needs both archive writeback and metadata writeback turned on
              above.
            </p>
          ) : null}
        </div>

        {writeback && run ? <RunSummary run={run} /> : null}

        <div className="flex justify-end">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={!writeback || live || start.isPending}
            onClick={() => setOpen(true)}
          >
            {live ? "Rewriting…" : "Rewrite metadata files"}
          </Button>
        </div>
      </CardContent>

      <AlertDialog open={open} onOpenChange={setOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              Rewrite every archive in this library?
            </AlertDialogTitle>
            <AlertDialogDescription>
              This rewrites every archive in {library?.name ?? "this library"}:
              each file gets new ComicInfo.xml and MetronInfo.xml entries built
              from the metadata Folio holds. Backups follow the library&apos;s
              backup setting. It runs in the background and can take hours on a
              large library; progress shows on Background work.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={start.isPending}>
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              disabled={start.isPending}
              onClick={() =>
                start.mutate(undefined, { onSettled: () => setOpen(false) })
              }
            >
              {start.isPending ? "Starting…" : "Rewrite all archives"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Card>
  );
}

function RunSummary({ run }: { run: SidecarReemitRunView }) {
  const done =
    run.rewritten + run.failed + run.skipped_refused + run.skipped_gone;
  const live = isLive(run);
  return (
    <div className="space-y-1.5">
      <div className="flex items-baseline justify-between gap-2 text-xs">
        <span className="text-muted-foreground">
          {run.stalled
            ? "Stalled — start it again to resume"
            : live
              ? "In progress"
              : run.status === "aborted"
                ? "Stopped"
                : "Last run"}
          {run.note ? ` — ${run.note}` : ""}
        </span>
        <span className="tabular-nums">
          {n(done)} / {n(run.total)}
        </span>
      </div>
      {live ? (
        <Progress
          value={pct(done, run.total)}
          aria-label="Metadata file rewrite progress"
        />
      ) : null}
      <p className="text-muted-foreground text-xs tabular-nums">
        {n(run.rewritten)} rewritten · {n(run.skipped_refused)} skipped (format)
        · {n(run.failed)} failed
        {live ? (
          <>
            {" · "}
            <Link href={BACKGROUND_WORK_HREF} className="underline">
              Background work
            </Link>
          </>
        ) : null}
      </p>
    </div>
  );
}
