"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";

import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
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
import { useSessions } from "@/lib/api/queries";
import { useRevokeAllSessions, useRevokeSession } from "@/lib/api/mutations";
import { prettyUserAgent, timeAgo } from "@/lib/sessions";
import type { SessionView } from "@/lib/api/types";

import { SettingsSection } from "./SettingsSection";

export function SessionsCard() {
  const sessions = useSessions();
  const router = useRouter();
  const revokeOne = useRevokeSession();
  const revokeAll = useRevokeAllSessions();
  const [revokeAllOpen, setRevokeAllOpen] = useState(false);

  if (sessions.isLoading) {
    return (
      <SettingsSection
        title="Active sessions"
        description="Every browser or device that's currently signed in to your account."
      >
        <Skeleton className="h-24 w-full" />
      </SettingsSection>
    );
  }

  if (sessions.error || !sessions.data) {
    return (
      <SettingsSection
        title="Active sessions"
        description="Every browser or device that's currently signed in to your account."
      >
        <p className="text-destructive text-sm">Failed to load sessions.</p>
      </SettingsSection>
    );
  }

  const rows = sessions.data.sessions;
  const hasOthers = rows.some((r) => !r.current);

  return (
    <SettingsSection
      title="Active sessions"
      description="Every browser or device that's currently signed in to your account. Revoke any session you don't recognize."
    >
      <div className="space-y-3">
        {rows.length === 0 ? (
          <p className="text-muted-foreground text-sm">No active sessions.</p>
        ) : (
          <ul className="space-y-2">
            {rows.map((s) => (
              <SessionRow
                key={s.id}
                s={s}
                onRevoke={() => {
                  revokeOne.mutate(s.id, {
                    onSuccess: () => {
                      if (s.current) router.push("/sign-in");
                    },
                  });
                }}
                disabled={revokeOne.isPending}
              />
            ))}
          </ul>
        )}
        <div className="border-border flex items-center justify-end gap-3 border-t pt-3">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={!hasOthers || revokeAll.isPending}
            onClick={() => setRevokeAllOpen(true)}
          >
            Sign out everywhere
          </Button>
        </div>
      </div>

      <AlertDialog open={revokeAllOpen} onOpenChange={setRevokeAllOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Sign out of every session?</AlertDialogTitle>
            <AlertDialogDescription>
              This signs you out of every browser and device, including this
              one. Existing access tokens stop working immediately. You&apos;ll
              need to sign in again to continue.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={revokeAll.isPending}>
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              disabled={revokeAll.isPending}
              onClick={() => {
                revokeAll.mutate(undefined, {
                  onSuccess: () => router.push("/sign-in"),
                });
              }}
            >
              {revokeAll.isPending ? "Signing out…" : "Sign out everywhere"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </SettingsSection>
  );
}

function SessionRow({
  s,
  onRevoke,
  disabled,
}: {
  s: SessionView;
  onRevoke: () => void;
  disabled?: boolean;
}) {
  const { device, raw } = prettyUserAgent(s.user_agent);
  return (
    <li className="border-border bg-background flex items-start justify-between gap-4 rounded-md border p-3">
      <div className="min-w-0 flex-1 space-y-1">
        <div className="flex items-center gap-2">
          <span
            className="text-foreground truncate text-sm font-medium"
            title={raw}
          >
            {device}
          </span>
          {s.current ? (
            <span className="bg-primary/10 text-primary rounded px-1.5 py-0.5 text-[10px] font-medium tracking-wide uppercase">
              This session
            </span>
          ) : null}
        </div>
        <p className="text-muted-foreground text-xs">
          {s.ip ?? "Unknown IP"} · Active {timeAgo(s.last_used_at)} · Signed in{" "}
          {timeAgo(s.created_at)}
        </p>
      </div>
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={onRevoke}
        disabled={disabled}
      >
        {s.current ? "Sign out" : "Revoke"}
      </Button>
    </li>
  );
}
