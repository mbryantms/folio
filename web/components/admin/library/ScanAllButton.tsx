"use client";

import * as React from "react";
import { Loader2, ScanLine } from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { useScanAllLibraries } from "@/lib/api/mutations";
import { useLibraryList } from "@/lib/api/queries";

/**
 * Header action that triggers a scan across every library the admin
 * can administer. Confirms first because it can fan out to dozens of
 * jobs depending on the operator's library layout; the backend
 * coalesces, but the toast is clearer when the user opted in
 * explicitly.
 *
 * Force-content-verify is opt-in inside the dialog rather than a
 * separate button — the slow path is the rarer choice and burying it
 * under a checkbox keeps the common-case flow one click after the
 * confirm.
 */
export function ScanAllButton() {
  const [open, setOpen] = React.useState(false);
  const [force, setForce] = React.useState(false);
  const { data: libraries } = useLibraryList();
  const mutate = useScanAllLibraries();
  const count = libraries?.length ?? 0;
  const disabled = count === 0 || mutate.isPending;

  function handleConfirm() {
    mutate.mutate(
      { force },
      {
        onSuccess: () => {
          setOpen(false);
          setForce(false);
        },
      },
    );
  }

  return (
    <AlertDialog open={open} onOpenChange={setOpen}>
      <AlertDialogTrigger asChild>
        <Button
          type="button"
          variant="outline"
          disabled={count === 0}
          aria-label="Scan all libraries"
        >
          <ScanLine className="mr-2 h-4 w-4" aria-hidden="true" />
          Scan all
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            Scan {count === 1 ? "the library" : `all ${count} libraries`}?
          </AlertDialogTitle>
          <AlertDialogDescription>
            Enqueues a scan for each library. Libraries already scanning are
            joined, not duplicated — clicking twice is safe.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <div className="flex items-start gap-3 rounded-md border p-3">
          <Checkbox
            id="scan-all-force"
            checked={force}
            onCheckedChange={(v) => setForce(v === true)}
          />
          <div className="grid gap-1.5 leading-none">
            <Label htmlFor="scan-all-force" className="cursor-pointer">
              Content-verify scan
            </Label>
            <p className="text-muted-foreground text-xs">
              Bypasses the per-file mtime fast path and re-parses every archive.
              Much slower; only needed when ComicInfo or filename heuristics
              changed and you want every issue row re-evaluated.
            </p>
          </div>
        </div>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={mutate.isPending}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            onClick={(e) => {
              e.preventDefault();
              handleConfirm();
            }}
            disabled={disabled}
          >
            {mutate.isPending ? (
              <>
                <Loader2
                  className="mr-2 h-4 w-4 animate-spin"
                  aria-hidden="true"
                />
                Enqueuing…
              </>
            ) : force ? (
              "Scan all (content-verify)"
            ) : (
              "Scan all"
            )}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
