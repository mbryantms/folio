"use client";

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
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { useMe } from "@/lib/api/queries";
import {
  useClearReadingHistory,
  useUpdatePreferences,
} from "@/lib/api/mutations";

/**
 * Privacy block on /settings/activity. Bundles:
 *  - Capture reading sessions (kill switch)
 *  - Exclude my activity from server-wide aggregates
 *  - Delete all reading history (audited destructive)
 */
export function PrivacyControls() {
  const me = useMe();
  const update = useUpdatePreferences({ silent: false });
  const clearMut = useClearReadingHistory();
  const [open, setOpen] = useState(false);

  const trackingEnabled = me.data?.activity_tracking_enabled !== false;
  const excluded = me.data?.exclude_from_aggregates === true;

  return (
    <div className="space-y-4">
      <Row
        title="Capture reading sessions"
        description="When off, the reader stops creating new sessions. Existing data stays put — clear it below if you want a fresh slate."
        right={
          <Switch
            checked={trackingEnabled}
            onCheckedChange={(v) =>
              update.mutate({ activity_tracking_enabled: v })
            }
            disabled={update.isPending || me.isLoading}
            aria-label="Capture reading sessions"
          />
        }
      />
      <Row
        title="Exclude from server-wide aggregates"
        description="When on, admins still see your data on the per-user drill-down (always audited) but server totals, DAU/WAU/MAU, and top-series lists won't count your sessions."
        right={
          <Switch
            checked={excluded}
            onCheckedChange={(v) =>
              update.mutate({ exclude_from_aggregates: v })
            }
            disabled={update.isPending || me.isLoading}
            aria-label="Exclude from server-wide aggregates"
          />
        }
      />
      <Row
        title="Delete all reading history"
        description="Permanently removes every reading session row for your account. Stats reset to zero. The deletion itself is recorded in the audit log."
        right={
          <AlertDialog open={open} onOpenChange={setOpen}>
            <AlertDialogTrigger asChild>
              <Button variant="destructive" size="sm">
                Delete history
              </Button>
            </AlertDialogTrigger>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>Delete all reading history?</AlertDialogTitle>
                <AlertDialogDescription>
                  This can&apos;t be undone. Every `reading_sessions` row for
                  your account will be removed. Audit log keeps a record that
                  you initiated the deletion.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel disabled={clearMut.isPending}>
                  Cancel
                </AlertDialogCancel>
                <AlertDialogAction
                  disabled={clearMut.isPending}
                  onClick={(e) => {
                    e.preventDefault();
                    clearMut.mutate(undefined, {
                      onSuccess: () => setOpen(false),
                    });
                  }}
                  className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                >
                  {clearMut.isPending ? "Deleting…" : "Delete history"}
                </AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        }
      />
    </div>
  );
}

function Row({
  title,
  description,
  right,
}: {
  title: string;
  description: string;
  right: React.ReactNode;
}) {
  return (
    <div className="flex items-start justify-between gap-6">
      <div className="space-y-0.5">
        <p className="text-foreground text-sm font-medium">{title}</p>
        <p className="text-muted-foreground max-w-prose text-sm">
          {description}
        </p>
      </div>
      <div className="shrink-0 pt-1">{right}</div>
    </div>
  );
}
