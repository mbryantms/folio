"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useTransition } from "react";
import {
  ChevronUp,
  HelpCircle,
  LogOut,
  Settings,
  Shield,
  User as UserIcon,
} from "lucide-react";
import { toast } from "sonner";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

/**
 * Sidebar footer: user identity + dropdown of account actions. Replaces the
 * topbar user controls (UserNav, header sign-out, "Admin" badge) on every
 * shell. Items are gated on role — "Admin" link only renders when
 * `user.role === "admin"`.
 */
export function UserFooter({
  user,
  collapsed = false,
}: {
  user: { display_name: string; email: string | null; role: string };
  /** When true, the footer renders just the avatar (matches the
   *  collapsed sidebar's icon-only look). The dropdown menu is unchanged. */
  collapsed?: boolean;
}) {
  const router = useRouter();
  const [pending, start] = useTransition();
  const isAdmin = user.role === "admin";
  const initials = computeInitials(user.display_name, user.email);

  async function signOut() {
    const csrf = readCsrfCookie();
    await fetch("/api/auth/logout", {
      method: "POST",
      credentials: "include",
      headers: csrf ? { "X-CSRF-Token": csrf } : undefined,
    }).catch(() => undefined);
    start(() => router.refresh());
  }

  return (
    <div className="border-border/60 border-t px-2 py-2">
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          {collapsed ? (
            <button
              type="button"
              disabled={pending}
              aria-label={user.display_name || user.email || "Account"}
              className="group hover:bg-secondary/60 focus-visible:ring-ring focus-visible:ring-offset-background data-[state=open]:bg-secondary/70 mx-auto flex size-9 items-center justify-center rounded-md transition-colors focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:outline-none disabled:opacity-50"
            >
              <span className="bg-secondary text-foreground flex size-7 items-center justify-center rounded-full text-xs font-semibold">
                {initials || <UserIcon className="size-4" aria-hidden="true" />}
              </span>
            </button>
          ) : (
            <button
              type="button"
              disabled={pending}
              className="group hover:bg-secondary/60 focus-visible:ring-ring focus-visible:ring-offset-background data-[state=open]:bg-secondary/70 flex w-full items-center gap-3 rounded-md px-2 py-2 text-left text-sm transition-colors focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:outline-none disabled:opacity-50"
            >
              <span className="bg-secondary text-foreground flex size-8 shrink-0 items-center justify-center rounded-full text-xs font-semibold">
                {initials || <UserIcon className="size-4" aria-hidden="true" />}
              </span>
              <span className="min-w-0 flex-1">
                <span className="flex items-center gap-1.5">
                  <span className="text-foreground truncate font-medium">
                    {user.display_name || user.email || "Account"}
                  </span>
                  {isAdmin ? (
                    <span className="border-border/60 bg-background/40 text-primary rounded border px-1 py-px text-[9px] font-semibold tracking-widest uppercase">
                      Admin
                    </span>
                  ) : null}
                </span>
                {user.display_name && user.email ? (
                  <span className="text-muted-foreground block truncate text-xs">
                    {user.email}
                  </span>
                ) : null}
              </span>
              <ChevronUp
                className="text-muted-foreground size-4 shrink-0 transition-transform group-data-[state=open]:rotate-180"
                aria-hidden="true"
              />
            </button>
          )}
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" side="top" className="w-56">
          <DropdownMenuLabel className="tracking-normal normal-case">
            <span className="text-muted-foreground block text-[10px] tracking-widest uppercase">
              Signed in as
            </span>
            <span className="text-foreground block truncate text-sm font-medium">
              {user.email ?? user.display_name}
            </span>
          </DropdownMenuLabel>
          <DropdownMenuSeparator />
          <DropdownMenuItem asChild>
            <Link href={`/settings/account`}>
              <UserIcon /> Profile
            </Link>
          </DropdownMenuItem>
          <DropdownMenuItem asChild>
            <Link href={`/settings`}>
              <Settings /> Settings
            </Link>
          </DropdownMenuItem>
          {isAdmin ? (
            <DropdownMenuItem asChild>
              <Link href={`/admin`}>
                <Shield /> Admin
              </Link>
            </DropdownMenuItem>
          ) : null}
          <DropdownMenuSeparator />
          <DropdownMenuItem
            onSelect={() =>
              toast("Help coming soon", {
                description: "User-facing docs land in a follow-up.",
              })
            }
          >
            <HelpCircle /> Help
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem
            onSelect={(e) => {
              e.preventDefault();
              void signOut();
            }}
            disabled={pending}
            className="text-destructive focus:bg-destructive/10 focus:text-destructive"
          >
            <LogOut /> Sign out
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

function computeInitials(displayName: string, email: string | null): string {
  const source = displayName.trim() || email?.trim() || "";
  if (!source) return "";
  // Names: take first letter of up to two words ("Jane Doe" → "JD").
  // Email-only: first letter + first letter after a separator ("matthew@example" → "M.").
  const parts = source.split(/[\s_.-]+|@/).filter(Boolean);
  return parts
    .slice(0, 2)
    .map((p) => p[0]?.toUpperCase() ?? "")
    .join("");
}

function readCsrfCookie(): string | null {
  if (typeof document === "undefined") return null;
  const m = document.cookie.match(/(?:^|;\s*)__Host-comic_csrf=([^;]+)/);
  return m ? decodeURIComponent(m[1]!) : null;
}
