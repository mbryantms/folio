"use client";

import * as React from "react";
import { KeyRound, TriangleAlert, UserPlus } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { CopyButton } from "@/components/ui/copy-button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useCreateUser } from "@/lib/api/mutations";
import type { CreateUserResp } from "@/lib/api/types";

/**
 * Admin create-user (3.8 / audit D9). Provisions a local account with a
 * server-generated one-time temporary password — the gap this fills is
 * that the *only* prior way to add a user was temporarily re-opening
 * public registration.
 *
 * Two phases in one dialog: the create form, then a "created" panel that
 * shows the temp password ONCE (it's hashed at rest and never
 * re-retrievable) with a copy button and a hand-off note.
 */
export function CreateUserDialog() {
  const [open, setOpen] = React.useState(false);
  const [email, setEmail] = React.useState("");
  const [displayName, setDisplayName] = React.useState("");
  const [role, setRole] = React.useState<"user" | "admin">("user");
  const [created, setCreated] = React.useState<CreateUserResp | null>(null);
  const create = useCreateUser();

  function reset() {
    setEmail("");
    setDisplayName("");
    setRole("user");
    setCreated(null);
    create.reset();
  }

  function onOpenChange(next: boolean) {
    setOpen(next);
    // Clear transient state (incl. the shown temp password) on close so it
    // never lingers behind a reopened dialog.
    if (!next) reset();
  }

  function submit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = email.trim();
    if (!trimmed) {
      toast.error("Email is required");
      return;
    }
    create.mutate(
      {
        email: trimmed,
        display_name: displayName.trim() || undefined,
        role,
      },
      { onSuccess: (resp) => resp && setCreated(resp) },
    );
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogTrigger asChild>
        <Button size="sm">
          <UserPlus className="size-3.5" />
          Create user
        </Button>
      </DialogTrigger>
      <DialogContent className="sm:max-w-md">
        {created ? (
          <CreatedPanel resp={created} onDone={() => onOpenChange(false)} />
        ) : (
          <form onSubmit={submit} className="space-y-4">
            <DialogHeader>
              <DialogTitle>Create user</DialogTitle>
              <DialogDescription>
                Provisions an account with a one-time temporary password. Works
                even when public sign-ups are closed.
              </DialogDescription>
            </DialogHeader>
            <div className="space-y-2">
              <Label htmlFor="create-user-email">Email</Label>
              <Input
                id="create-user-email"
                type="email"
                autoComplete="off"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                placeholder="reader@example.com"
                autoFocus
                required
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="create-user-name">Display name (optional)</Label>
              <Input
                id="create-user-name"
                value={displayName}
                onChange={(e) => setDisplayName(e.target.value)}
                placeholder="Defaults to the email name"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="create-user-role">Role</Label>
              <Select
                value={role}
                onValueChange={(v) => setRole(v as "user" | "admin")}
              >
                <SelectTrigger id="create-user-role">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="user">User</SelectItem>
                  <SelectItem value="admin">Admin</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <DialogFooter>
              <Button
                type="button"
                variant="ghost"
                onClick={() => onOpenChange(false)}
              >
                Cancel
              </Button>
              <Button type="submit" disabled={create.isPending}>
                {create.isPending ? "Creating…" : "Create user"}
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}

function CreatedPanel({
  resp,
  onDone,
}: {
  resp: CreateUserResp;
  onDone: () => void;
}) {
  return (
    <div className="space-y-4">
      <DialogHeader>
        <DialogTitle>User created</DialogTitle>
        <DialogDescription>
          <span className="text-foreground font-medium">{resp.email}</span> can
          sign in now with this temporary password.
        </DialogDescription>
      </DialogHeader>

      <div className="space-y-2">
        <Label className="text-muted-foreground flex items-center gap-1.5 text-xs tracking-wide uppercase">
          <KeyRound aria-hidden="true" className="size-3.5" />
          Temporary password
        </Label>
        <div className="flex items-center gap-2">
          <code className="border-border bg-muted flex-1 truncate rounded-md border px-3 py-2 font-mono text-sm">
            {resp.temp_password}
          </code>
          <CopyButton value={resp.temp_password} />
        </div>
      </div>

      <p className="text-muted-foreground flex items-start gap-1.5 text-xs text-balance">
        <TriangleAlert
          aria-hidden="true"
          className="mt-px size-3.5 shrink-0 text-amber-500"
        />
        <span>
          Copy it now — it won&apos;t be shown again. Share it with the user;
          they can change it from their account settings after signing in.
        </span>
      </p>

      <DialogFooter>
        <Button type="button" onClick={onDone}>
          Done
        </Button>
      </DialogFooter>
    </div>
  );
}
