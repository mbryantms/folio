"use client";

import * as React from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  useDisableUser,
  useEnableUser,
  useUpdateUser,
} from "@/lib/api/mutations";
import { useMe } from "@/lib/api/queries";
import type { AdminUserDetailView } from "@/lib/api/types";
import { cn } from "@/lib/utils";

function stateVariant(state: string): "default" | "secondary" | "destructive" {
  if (state === "disabled") return "destructive";
  if (state === "pending_verification") return "secondary";
  return "default";
}

export function UserProfileForm({ user }: { user: AdminUserDetailView }) {
  const { data: me } = useMe();
  const isSelf = me?.id === user.id;

  const [displayName, setDisplayName] = React.useState(user.display_name);
  const [role, setRole] = React.useState<"admin" | "user">(
    user.role === "admin" ? "admin" : "user",
  );
  // Re-sync local form state when the upstream user object changes (after a
  // mutation invalidates the query and we re-fetch). Comparing the prev value
  // during render is the React 19 idiom; useEffect would re-render twice.
  const [prevUser, setPrevUser] = React.useState(user);
  if (user !== prevUser) {
    setPrevUser(user);
    setDisplayName(user.display_name);
    setRole(user.role === "admin" ? "admin" : "user");
  }

  const update = useUpdateUser(user.id);
  const disable = useDisableUser(user.id);
  const enable = useEnableUser(user.id);

  const dirty = displayName.trim() !== user.display_name || role !== user.role;
  const trimmed = displayName.trim();
  const valid = trimmed.length > 0;

  return (
    <div className="space-y-6">
      <section className="space-y-3">
        <div className="grid gap-4 md:grid-cols-2">
          <div className="space-y-1">
            <Label htmlFor="display-name">Display name</Label>
            <Input
              id="display-name"
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
              placeholder="Display name"
            />
          </div>
          <div className="space-y-1">
            <Label htmlFor="email">Email</Label>
            <Input
              id="email"
              value={user.email ?? ""}
              readOnly
              className="bg-muted/40 font-mono text-xs"
            />
            {!user.email_verified && user.email ? (
              <p className="text-muted-foreground text-[11px]">
                Email is unverified.
              </p>
            ) : null}
          </div>
        </div>

        <div>
          <Label className="block pb-2">Role</Label>
          <div className="flex gap-2">
            {(["admin", "user"] as const).map((r) => (
              <button
                key={r}
                type="button"
                disabled={isSelf && r === "user"}
                onClick={() => setRole(r)}
                className={cn(
                  "rounded-full border px-3 py-1 text-xs font-medium tracking-wider uppercase transition-colors",
                  role === r
                    ? "border-primary bg-primary/10 text-primary"
                    : "border-border text-muted-foreground hover:text-foreground",
                  isSelf && r === "user" && "cursor-not-allowed opacity-50",
                )}
              >
                {r}
              </button>
            ))}
            {isSelf ? (
              <span className="text-muted-foreground self-center text-[11px]">
                You can&rsquo;t demote yourself.
              </span>
            ) : null}
          </div>
        </div>

        <div className="flex items-center gap-2 pt-2">
          <Button
            size="sm"
            disabled={!dirty || !valid || update.isPending}
            onClick={() =>
              update.mutate({
                display_name:
                  trimmed !== user.display_name ? trimmed : undefined,
                role: role !== user.role ? role : undefined,
              })
            }
          >
            {update.isPending ? "Saving…" : "Save profile"}
          </Button>
          {dirty ? (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                setDisplayName(user.display_name);
                setRole(user.role === "admin" ? "admin" : "user");
              }}
            >
              Reset
            </Button>
          ) : null}
        </div>
      </section>

      <section className="border-border bg-card space-y-2 rounded-md border p-4">
        <div className="flex items-start justify-between gap-4">
          <div>
            <p className="text-sm font-medium">Account state</p>
            <div className="flex items-center gap-2 pt-1">
              <Badge variant={stateVariant(user.state)} className="uppercase">
                {user.state.replace("_", " ")}
              </Badge>
              <span className="text-muted-foreground text-xs">
                Created {new Date(user.created_at).toLocaleDateString()}
                {user.last_login_at
                  ? ` · last login ${new Date(user.last_login_at).toLocaleString()}`
                  : " · never logged in"}
              </span>
            </div>
          </div>
          <div className="flex gap-2">
            {user.state === "disabled" ? (
              <Button
                size="sm"
                disabled={enable.isPending}
                onClick={() => enable.mutate()}
              >
                {enable.isPending ? "Enabling…" : "Enable"}
              </Button>
            ) : (
              <Button
                size="sm"
                variant="destructive"
                disabled={isSelf || disable.isPending}
                onClick={() => disable.mutate()}
                title={isSelf ? "You can't disable yourself" : undefined}
              >
                {disable.isPending ? "Disabling…" : "Disable"}
              </Button>
            )}
          </div>
        </div>
        <p className="text-muted-foreground text-[11px]">
          Disabling revokes active sessions and prevents future sign-ins.
        </p>
      </section>
    </div>
  );
}
