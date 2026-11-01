"use client";

import { useQueryClient } from "@tanstack/react-query";
import { Download, Trash2 } from "lucide-react";
import { useId, useState } from "react";

import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { collectServerErrors } from "@/lib/api/form-errors";
import { useDeleteAccount } from "@/lib/api/mutations";
import { ApiMutationError } from "@/lib/api/mutations/_core";
import type { MeView } from "@/lib/api/types";

import { EXPORT_HREF } from "./DataExportCard";
import { SettingsSection } from "./SettingsSection";

/** Where a successful deletion lands: the sign-in page's "account deleted"
 *  notice. */
export const DELETED_HREF = "/sign-in?deleted=1";

/** What an identity-provider account types to confirm: its email, or its
 *  display name when the issuer gave no email. Mirrors the server's
 *  `confirmation_matches` (email compared case-insensitively). */
function confirmationTarget(me: Pick<MeView, "email" | "display_name">) {
  const email = me.email?.trim();
  return email
    ? { kind: "email" as const, value: email }
    : {
        kind: "name" as const,
        value: me.display_name.trim(),
      };
}

function matchesTarget(
  typed: string,
  target: ReturnType<typeof confirmationTarget>,
): boolean {
  const t = typed.trim();
  if (!t) return false;
  return target.kind === "email"
    ? t.toLowerCase() === target.value.toLowerCase()
    : t === target.value;
}

/** Settings → Account danger zone (audit 2026-11 F5): `DELETE /me` behind a
 *  confirm dialog that points at the export first. Local accounts re-enter
 *  their password; identity-provider accounts type their email. */
export function DeleteAccountCard({
  me,
}: {
  me: Pick<MeView, "email" | "display_name" | "password_editable">;
}) {
  const queryClient = useQueryClient();
  const del = useDeleteAccount();
  const [open, setOpen] = useState(false);
  const [value, setValue] = useState("");
  const [fieldError, setFieldError] = useState<string | null>(null);
  // Set once the 204 lands: the account is gone and the device cleanup is
  // running, so the form stays locked until the navigation away (a second
  // submit would 401 without the cleared cookies).
  const [leaving, setLeaving] = useState(false);
  const inputId = useId();
  const errorId = useId();

  const busy = del.isPending || leaving;
  const usesPassword = me.password_editable === true;
  const target = confirmationTarget(me);
  const ready = usesPassword ? value.length > 0 : matchesTarget(value, target);

  function onOpenChange(next: boolean) {
    if (busy) return;
    setOpen(next);
    if (!next) {
      setValue("");
      setFieldError(null);
    }
  }

  function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!ready || busy) return;
    setFieldError(null);
    del.mutate(usesPassword ? { password: value } : { confirm: value }, {
      onSuccess: () => {
        setLeaving(true);
        void import("@/lib/pwa/forget-device").then(
          ({ forgetAccountOnDevice }) =>
            forgetAccountOnDevice(queryClient, DELETED_HREF),
        );
      },
      onError: (err) => {
        const fields = collectServerErrors(err, ["password", "confirm"]);
        const msg =
          fields.password ??
          fields.confirm ??
          fields.root ??
          (err instanceof ApiMutationError && err.status === 403
            ? err.message
            : null);
        setFieldError(msg);
      },
    });
  }

  return (
    <SettingsSection
      title="Delete account"
      description="Permanently remove this account and everything it owns."
    >
      <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
        <p className="text-muted-foreground text-sm">
          Reading progress, notes and bookmarks, collections, saved views,
          ratings, reading log, app passwords and every signed-in session are
          deleted at once and can&apos;t be recovered. Your comics and the
          library stay as they are.
        </p>
        <Button
          type="button"
          variant="destructive"
          size="sm"
          className="shrink-0"
          onClick={() => setOpen(true)}
        >
          <Trash2 aria-hidden="true" className="mr-1 size-3.5" />
          Delete account
        </Button>
      </div>

      <AlertDialog open={open} onOpenChange={onOpenChange}>
        <AlertDialogContent>
          <form onSubmit={onSubmit} className="grid gap-4">
            <AlertDialogHeader>
              <AlertDialogTitle>Delete your account?</AlertDialogTitle>
              <AlertDialogDescription>
                This can&apos;t be undone. Download your data first if you might
                want it later — the export is a single JSON file with your
                progress, notes, collections and preferences.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <Button variant="outline" size="sm" asChild className="w-fit">
              <a href={EXPORT_HREF} download>
                <Download aria-hidden="true" className="mr-1 size-3.5" />
                Export my data
              </a>
            </Button>
            <div className="grid gap-2">
              <Label htmlFor={inputId}>
                {usesPassword
                  ? "Your password"
                  : `Type ${target.value} to confirm`}
              </Label>
              <Input
                id={inputId}
                type={usesPassword ? "password" : "text"}
                autoComplete={usesPassword ? "current-password" : "off"}
                value={value}
                onChange={(e) => {
                  setValue(e.target.value);
                  setFieldError(null);
                }}
                aria-invalid={fieldError ? true : undefined}
                aria-describedby={fieldError ? errorId : undefined}
                disabled={busy}
              />
              {usesPassword ? null : (
                <p className="text-muted-foreground text-xs">
                  Your account signs in through an identity provider, so Folio
                  confirms with your{" "}
                  {target.kind === "email" ? "email address" : "display name"}{" "}
                  instead of a password.
                </p>
              )}
              {fieldError ? (
                <p id={errorId} className="text-destructive text-sm">
                  {fieldError}
                </p>
              ) : null}
            </div>
            <AlertDialogFooter>
              <AlertDialogCancel type="button" disabled={busy}>
                Cancel
              </AlertDialogCancel>
              <Button
                type="submit"
                variant="destructive"
                disabled={!ready || busy}
              >
                {busy ? "Deleting…" : "Delete account"}
              </Button>
            </AlertDialogFooter>
          </form>
        </AlertDialogContent>
      </AlertDialog>
    </SettingsSection>
  );
}
