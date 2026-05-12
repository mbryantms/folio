"use client";

import { useRouter } from "next/navigation";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
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
import { useDeleteLibrary } from "@/lib/api/mutations";

/**
 * Destructive-actions card on the library Settings tab. Currently houses
 * the hard-delete button. Confirmation requires typing the library's name
 * to unlock the action — we don't want a single misclick to drop hundreds
 * of issues and their thumbnails.
 */
export function LibraryDangerZone({ id, name }: { id: string; name: string }) {
  const router = useRouter();
  const del = useDeleteLibrary(id);
  const [open, setOpen] = useState(false);
  const [confirmText, setConfirmText] = useState("");
  const canDelete = confirmText.trim() === name;

  return (
    <Card className="border-destructive/40">
      <CardContent className="space-y-3 p-5">
        <div>
          <h3 className="text-destructive text-sm font-semibold tracking-tight">
            Danger zone
          </h3>
          <p className="text-muted-foreground mt-1 text-xs">
            Deleting a library permanently removes its series, issues, scan
            history, health log, and on-disk thumbnails. The original comic
            files on the source filesystem are not touched.
          </p>
        </div>
        <div className="flex justify-end">
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => {
              setConfirmText("");
              setOpen(true);
            }}
            disabled={del.isPending}
            className="text-destructive hover:bg-destructive/10 hover:text-destructive"
          >
            Delete library
          </Button>
        </div>
      </CardContent>

      <AlertDialog open={open} onOpenChange={setOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete this library?</AlertDialogTitle>
            <AlertDialogDescription>
              All series, issues, thumbnails, scan history, and health records
              for this library will be permanently removed. Source files on disk
              are untouched and can be re-imported later.
              <br />
              <br />
              Type the library name <strong>{name}</strong> to confirm.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <div className="space-y-2">
            <Label htmlFor="confirm-name">Library name</Label>
            <Input
              id="confirm-name"
              autoFocus
              value={confirmText}
              onChange={(e) => setConfirmText(e.target.value)}
              placeholder={name}
              disabled={del.isPending}
            />
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={del.isPending}>
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              disabled={!canDelete || del.isPending}
              onClick={() => {
                del.mutate(undefined, {
                  onSuccess: () => {
                    setOpen(false);
                    router.push(`/admin/libraries`);
                  },
                });
              }}
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              {del.isPending ? "Deleting…" : "Delete library"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Card>
  );
}
