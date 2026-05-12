"use client";

import { useState, useSyncExternalStore } from "react";
import { Copy, Check, Trash2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
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
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useAppPasswords } from "@/lib/api/queries";
import {
  useCreateAppPassword,
  useRevokeAppPassword,
} from "@/lib/api/mutations";
import { timeAgo } from "@/lib/sessions";
import type { AppPasswordCreatedView, AppPasswordView } from "@/lib/api/types";

import { SettingsSection } from "./SettingsSection";

export function AppPasswordsCard() {
  const list = useAppPasswords();
  const create = useCreateAppPassword();
  const [label, setLabel] = useState("");
  const [issued, setIssued] = useState<AppPasswordCreatedView | null>(null);

  function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = label.trim();
    if (!trimmed) return;
    create.mutate(
      { label: trimmed },
      {
        onSuccess: (data) => {
          if (data) {
            setIssued(data);
            setLabel("");
          }
        },
      },
    );
  }

  if (list.isLoading) {
    return (
      <SettingsSection
        title="Existing passwords"
        description="Active Bearer credentials issued to your account. Use them as `Authorization: Bearer <token>` from OPDS readers or scripts."
      >
        <Skeleton className="h-24 w-full" />
      </SettingsSection>
    );
  }
  if (list.error || !list.data) {
    return (
      <SettingsSection
        title="Existing passwords"
        description="Active Bearer credentials issued to your account."
      >
        <p className="text-destructive text-sm">Failed to load passwords.</p>
      </SettingsSection>
    );
  }

  const rows = list.data.items;

  return (
    <div className="space-y-6">
      <SettingsSection
        title="Generate a new password"
        description="Issue a long-lived Bearer token bound to your account. The plaintext is shown once and never retrievable again — copy it before closing the dialog."
      >
        <form onSubmit={onSubmit} className="flex items-end gap-3">
          <div className="flex-1 space-y-1.5">
            <Label htmlFor="app-password-label">Label</Label>
            <Input
              id="app-password-label"
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder="Kobo reader, Kavita sync, …"
              maxLength={80}
              disabled={create.isPending}
            />
          </div>
          <Button type="submit" disabled={create.isPending || !label.trim()}>
            {create.isPending ? "Generating…" : "Generate"}
          </Button>
        </form>
      </SettingsSection>

      <SettingsSection
        title="Existing passwords"
        description="Use them as `Authorization: Bearer <token>` from OPDS readers and scripts. Revoking a password disconnects every client using it immediately."
      >
        {rows.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            No app passwords yet. Generate one above.
          </p>
        ) : (
          <ul className="space-y-2">
            {rows.map((p) => (
              <PasswordRow key={p.id} p={p} />
            ))}
          </ul>
        )}
      </SettingsSection>

      <OpdsConnectionInfo />

      <IssuedDialog
        issued={issued}
        onOpenChange={(open) => {
          if (!open) setIssued(null);
        }}
      />
    </div>
  );
}

function OpdsConnectionInfo() {
  // `useSyncExternalStore` reads `window.location.origin` on the client
  // only; SSR / pre-hydration falls back to the placeholder string.
  // Replaces an older `useEffect(setOrigin)` shape that tripped the
  // `react-hooks/set-state-in-effect` lint.
  const origin = useSyncExternalStore(
    subscribeOriginNoop,
    getOriginSnapshot,
    getOriginServerSnapshot,
  );
  const feedUrl = origin ? `${origin}/opds/v1` : "<your folio URL>/opds/v1";

  return (
    <SettingsSection
      title="OPDS readers"
      description="Point OPDS-compatible readers (Chunky, KyBook, Panels, Kavita-mobile, …) at the URL below. Use any non-empty username with an app password as the password — HTTP Basic auth."
    >
      <div className="space-y-3">
        <div className="space-y-1">
          <Label className="text-muted-foreground text-xs uppercase tracking-wide">
            OPDS catalog URL
          </Label>
          <CopyableUrl value={feedUrl} />
        </div>
        <p className="text-muted-foreground text-xs">
          Bearer auth also works:{" "}
          <code className="bg-secondary/40 rounded px-1 py-0.5 text-[11px]">
            Authorization: Bearer app_…
          </code>
          .
        </p>
      </div>
    </SettingsSection>
  );
}

function subscribeOriginNoop(): () => void {
  return () => {};
}
function getOriginSnapshot(): string {
  return typeof window === "undefined" ? "" : window.location.origin;
}
function getOriginServerSnapshot(): string {
  return "";
}

function CopyableUrl({ value }: { value: string }) {
  const [copied, setCopied] = useState(false);
  async function copy() {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable */
    }
  }
  return (
    <div className="border-border bg-background flex items-center gap-2 rounded-md border p-2">
      <code className="text-foreground flex-1 truncate font-mono text-xs">
        {value}
      </code>
      <Button type="button" variant="outline" size="sm" onClick={copy}>
        {copied ? (
          <>
            <Check className="size-3.5" /> Copied
          </>
        ) : (
          <>
            <Copy className="size-3.5" /> Copy
          </>
        )}
      </Button>
    </div>
  );
}

function PasswordRow({ p }: { p: AppPasswordView }) {
  const revoke = useRevokeAppPassword();
  const [open, setOpen] = useState(false);
  return (
    <li className="border-border bg-background flex items-center justify-between gap-4 rounded-md border p-3">
      <div className="min-w-0 flex-1 space-y-1">
        <p className="text-foreground truncate text-sm font-medium">
          {p.label}
        </p>
        <p className="text-muted-foreground text-xs">
          Created {timeAgo(p.created_at)} ·{" "}
          {p.last_used_at
            ? `Last used ${timeAgo(p.last_used_at)}`
            : "Never used"}
        </p>
      </div>
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => setOpen(true)}
        disabled={revoke.isPending}
      >
        <Trash2 className="size-4" />
        Revoke
      </Button>
      <AlertDialog open={open} onOpenChange={setOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Revoke this app password?</AlertDialogTitle>
            <AlertDialogDescription>
              Any client currently authenticated with this token — including
              OPDS readers — will be signed out on its next request. This action
              is irreversible; you cannot recover the plaintext.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={revoke.isPending}>
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              disabled={revoke.isPending}
              onClick={() => revoke.mutate(p.id)}
            >
              {revoke.isPending ? "Revoking…" : "Revoke"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </li>
  );
}

function IssuedDialog({
  issued,
  onOpenChange,
}: {
  issued: AppPasswordCreatedView | null;
  onOpenChange: (open: boolean) => void;
}) {
  const [copied, setCopied] = useState(false);
  async function copy() {
    if (!issued) return;
    try {
      await navigator.clipboard.writeText(issued.plaintext);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable */
    }
  }
  return (
    <Dialog open={!!issued} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>App password generated</DialogTitle>
          <DialogDescription>
            Copy this token now — you won&rsquo;t be able to see it again. Paste
            it into your client as the password (with any non-empty username) or
            as a Bearer header.
          </DialogDescription>
        </DialogHeader>
        <div className="bg-secondary/40 text-foreground my-2 rounded-md border p-3 font-mono text-xs break-all">
          {issued?.plaintext}
        </div>
        <DialogFooter className="gap-2 sm:gap-2">
          <Button type="button" variant="outline" onClick={copy}>
            {copied ? (
              <>
                <Check className="size-4" /> Copied
              </>
            ) : (
              <>
                <Copy className="size-4" /> Copy
              </>
            )}
          </Button>
          <Button type="button" onClick={() => onOpenChange(false)}>
            Done
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
