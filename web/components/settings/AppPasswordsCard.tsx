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
import { useAppPasswords, useMe } from "@/lib/api/queries";
import {
  useCreateAppPassword,
  useRevokeAppPassword,
} from "@/lib/api/mutations";
import { timeAgo } from "@/lib/sessions";
import type {
  AppPasswordCreatedView,
  AppPasswordScope,
  AppPasswordView,
} from "@/lib/api/types";

import { SettingsSection } from "./SettingsSection";

export function AppPasswordsCard() {
  const list = useAppPasswords();
  const me = useMe();
  const create = useCreateAppPassword();
  const [label, setLabel] = useState("");
  const [scope, setScope] = useState<AppPasswordScope>("read");
  const [issued, setIssued] = useState<AppPasswordCreatedView | null>(null);

  function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = label.trim();
    if (!trimmed) return;
    create.mutate(
      { label: trimmed, scope },
      {
        onSuccess: (data) => {
          if (data) {
            setIssued(data);
            setLabel("");
            setScope("read");
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
        <form onSubmit={onSubmit} className="space-y-3">
          <div className="flex items-end gap-3">
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
          </div>
          <fieldset className="space-y-1.5">
            <Label className="text-sm">Scope</Label>
            <div className="flex flex-col gap-1.5">
              <label className="flex items-start gap-2 text-sm">
                <input
                  type="radio"
                  name="app-password-scope"
                  value="read"
                  checked={scope === "read"}
                  onChange={() => setScope("read")}
                  disabled={create.isPending}
                  className="mt-1"
                />
                <span>
                  <span className="font-medium">Read-only</span>
                  <span className="text-muted-foreground">
                    {" "}
                    — browse + download only. Choose this for one-off downloads,
                    scripts, or clients you don&rsquo;t want writing progress
                    back to Folio.
                  </span>
                </span>
              </label>
              <label className="flex items-start gap-2 text-sm">
                <input
                  type="radio"
                  name="app-password-scope"
                  value="read+progress"
                  checked={scope === "read+progress"}
                  onChange={() => setScope("read+progress")}
                  disabled={create.isPending}
                  className="mt-1"
                />
                <span>
                  <span className="font-medium">Read + write progress</span>
                  <span className="text-muted-foreground">
                    {" "}
                    — also lets the client sync your reading position back to
                    Folio. Recommended for active reader apps:{" "}
                    <span className="text-foreground">Panels (iOS)</span>,{" "}
                    <span className="text-foreground">KOReader</span>,{" "}
                    <span className="text-foreground">Chunky</span>,{" "}
                    <span className="text-foreground">KyBook 3</span>, and
                    Tachiyomi-class manga readers.
                  </span>
                </span>
              </label>
            </div>
          </fieldset>
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
        email={me.data?.email ?? null}
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
  const base = origin || "<your folio URL>";
  const feedUrl = `${base}/opds/v1`;
  // KOReader's KOSync plugin appends `/syncs/progress/{document_hash}`
  // to whatever "custom sync server" URL is configured. Pointing it at
  // `<folio>/opds/v1` lands on our shim path. The full PUT URL is
  // visible on this page for verification, but the user only pastes
  // the base into KOReader.
  const koreaderBase = feedUrl;

  return (
    <>
      <SettingsSection
        title="OPDS readers"
        description="Point OPDS-compatible readers (Chunky, KyBook, Panels, Kavita-mobile, …) at the URL below. Use any non-empty username with an app password as the password — HTTP Basic auth."
      >
        <div className="space-y-3">
          <div className="space-y-1">
            <Label className="text-muted-foreground text-xs tracking-wide uppercase">
              OPDS catalog URL
            </Label>
            <CopyableUrl value={feedUrl} />
          </div>
          <p className="text-muted-foreground text-xs">
            Bearer auth also works:{" "}
            <code className="bg-secondary/40 rounded px-1 py-0.5 text-[11px]">
              Authorization: Bearer app_…
            </code>
            . OPDS 2.0 (JSON-LD) is supported too — clients that prefer it
            negotiate automatically via the <code>Accept</code> header against
            the URL above, no separate setup needed.
          </p>
        </div>
      </SettingsSection>

      <SettingsSection
        title="KOReader sync"
        description="KOReader can sync your reading position back to Folio via its built-in KOSync plugin. Configure a custom sync server, then sign in with an app password scoped Read + write progress."
      >
        <div className="space-y-3">
          <div className="space-y-1">
            <Label className="text-muted-foreground text-xs tracking-wide uppercase">
              KOSync custom server URL
            </Label>
            <CopyableUrl value={koreaderBase} />
            <p className="text-muted-foreground text-xs">
              Paste this into{" "}
              <span className="font-medium">Settings → Progress sync</span> →{" "}
              <span className="font-medium">Custom sync server</span> in
              KOReader. Use any username; the password is an app password issued
              above with the{" "}
              <span className="font-mono">read + write progress</span> scope.
              KOReader appends{" "}
              <code className="bg-secondary/40 rounded px-1 py-0.5 text-[11px]">
                /syncs/progress/&lt;document-hash&gt;
              </code>{" "}
              when it writes; matching positions show up under your reading
              activity in Folio.
            </p>
          </div>
        </div>
      </SettingsSection>

      <SettingsSection
        title="Supported features"
        description="What every OPDS surface above exposes. Each client picks up the ones it understands; nothing here needs extra configuration."
      >
        <ul className="space-y-2 text-sm">
          <FeatureRow
            title="Page streaming (PSE)"
            description="Compatible clients (Chunky, KyBook 3, KOReader) can stream individual pages over signed URLs without downloading the whole archive — handy for large CBZ/CBR files on mobile."
          />
          <FeatureRow
            title="Range requests (resumable downloads)"
            description="Bytes ranges with 206 Partial Content. Interrupted downloads pick up where they left off."
          />
          <FeatureRow
            title="Progress sync"
            description={
              <>
                Reading position written back to Folio via{" "}
                <code className="bg-secondary/40 rounded px-1 py-0.5 text-[11px]">
                  PUT /opds/v1/issues/&#123;id&#125;/progress
                </code>{" "}
                and the KOReader shim. Requires an app password scoped{" "}
                <span className="font-mono">read + write progress</span>.
              </>
            }
          />
          <FeatureRow
            title="Personal feeds"
            description="Want to Read, CBL reading lists, your collections, and pinned filter views all surface as browsable subsections in the catalog."
          />
        </ul>
      </SettingsSection>
    </>
  );
}

function FeatureRow({
  title,
  description,
}: {
  title: string;
  description: React.ReactNode;
}) {
  return (
    <li className="border-border bg-background/40 rounded-md border p-3">
      <p className="text-foreground text-sm font-medium">{title}</p>
      <p className="text-muted-foreground mt-1 text-xs">{description}</p>
    </li>
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
        <div className="flex items-center gap-2">
          <p className="text-foreground truncate text-sm font-medium">
            {p.label}
          </p>
          <span
            className={
              "border-border rounded-full border px-2 py-0.5 font-mono text-[10px] tracking-wide uppercase " +
              (p.scope === "read+progress"
                ? "bg-primary/10 text-primary"
                : "text-muted-foreground")
            }
            title={
              p.scope === "read+progress"
                ? "Can read AND write reading progress"
                : "Read-only access (browse + download)"
            }
          >
            {p.scope === "read+progress" ? "read + write progress" : "read"}
          </span>
        </div>
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
  email,
  onOpenChange,
}: {
  issued: AppPasswordCreatedView | null;
  /**
   * Caller's email. When present, we surface a pre-computed
   * `Basic …` Authorization header value so mobile users don't have
   * to base64-encode `email:token` on a separate device. Falls back
   * to just the raw token if email isn't loaded yet (rare — useMe
   * is fetched at layout time).
   */
  email: string | null;
  onOpenChange: (open: boolean) => void;
}) {
  // Pre-compute the Basic header value for clients (Panels iOS in
  // custom-header mode, Tachiyomi-class) whose UI demands the
  // already-encoded `Authorization` value instead of accepting raw
  // username + password fields. `btoa` is browser-native and won't
  // throw on ASCII; emails with non-ASCII characters fall back to
  // hiding the row rather than crashing — the user can still use the
  // raw token in any client that takes username + password.
  const basicHeader =
    email && issued ? computeBasicHeader(email, issued.plaintext) : null;

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

        <div className="space-y-3">
          <div className="space-y-1">
            <Label className="text-muted-foreground text-xs tracking-wide uppercase">
              Token
            </Label>
            <CopyableValue value={issued?.plaintext ?? ""} />
          </div>

          {basicHeader && (
            <div className="space-y-1">
              <Label className="text-muted-foreground text-xs tracking-wide uppercase">
                Basic authorization header
              </Label>
              <CopyableValue value={basicHeader} />
              <p className="text-muted-foreground text-xs">
                Paste this into a client&rsquo;s custom-header field when the
                client&rsquo;s username/password inputs don&rsquo;t propagate to
                Komga-style REST writers. Required for Panels iOS to sync
                reading progress; harmless for clients that use the token
                directly.
              </p>
            </div>
          )}
        </div>

        <DialogFooter>
          <Button type="button" onClick={() => onOpenChange(false)}>
            Done
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/**
 * Encode `email:token` as base64 and prepend `Basic `. Returns `null`
 * if `btoa` rejects (non-ASCII email). The token itself is always
 * base32-lowercase ASCII, so the encoding can only fail on the email
 * side.
 */
function computeBasicHeader(email: string, token: string): string | null {
  if (typeof window === "undefined") return null;
  try {
    return `Basic ${window.btoa(`${email}:${token}`)}`;
  } catch {
    return null;
  }
}

function CopyableValue({ value }: { value: string }) {
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
    <div className="border-border bg-background flex items-start gap-2 rounded-md border p-2">
      <code className="text-foreground flex-1 font-mono text-xs break-all">
        {value}
      </code>
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={copy}
        className="shrink-0"
      >
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
