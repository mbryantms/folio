"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useEffect, useState, useTransition } from "react";

import { UserNav } from "./UserNav";

function readCsrfCookie(): string | null {
  if (typeof document === "undefined") return null;
  const m = document.cookie.match(/(?:^|;\s*)__Host-comic_csrf=([^;]+)/);
  return m ? decodeURIComponent(m[1]!) : null;
}

/**
 * Tiny inline sign-out button. Rendered only on the legacy chrome (sign-in,
 * not-found) — the main library shell uses [`UserFooter`](shell/UserFooter.tsx)
 * with a full dropdown. We don't import the M5 SessionsCard hooks here
 * because this button only ever needs to revoke the current cookie pair.
 */
function HeaderSignOut() {
  const [hasSession, setHasSession] = useState<boolean | null>(null);
  const [pending, start] = useTransition();
  const router = useRouter();

  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setHasSession(readCsrfCookie() !== null);
  }, []);

  if (!hasSession) return null;

  async function signOut() {
    const csrf = readCsrfCookie();
    await fetch("/api/auth/logout", {
      method: "POST",
      credentials: "include",
      headers: csrf ? { "X-CSRF-Token": csrf } : undefined,
    }).catch(() => undefined);
    setHasSession(false);
    start(() => router.refresh());
  }

  return (
    <button
      type="button"
      onClick={signOut}
      disabled={pending}
      className="text-xs text-neutral-400 hover:text-neutral-100 disabled:opacity-50"
    >
      Sign out
    </button>
  );
}

export function Chrome({
  children,
  breadcrumbs,
  showSignOut = true,
}: {
  children: React.ReactNode;
  breadcrumbs?: { href?: string; label: string }[];
  showSignOut?: boolean;
}) {
  return (
    <div className="flex min-h-screen flex-col">
      <header className="sticky top-0 z-10 border-b border-neutral-800 bg-neutral-950/80 backdrop-blur">
        <div className="mx-auto flex max-w-6xl items-center gap-4 px-6 py-3">
          <Link
            href="/"
            className="font-semibold tracking-tight text-neutral-100 hover:text-white"
          >
            Comic Reader
          </Link>
          <nav className="flex flex-1 items-center gap-3 text-sm text-neutral-400">
            {breadcrumbs?.map((b, i) => (
              <span key={i} className="flex items-center gap-3">
                <span aria-hidden="true">/</span>
                {b.href ? (
                  <Link href={b.href} className="hover:text-neutral-100">
                    {b.label}
                  </Link>
                ) : (
                  <span className="text-neutral-200">{b.label}</span>
                )}
              </span>
            ))}
          </nav>
          {showSignOut ? (
            <div className="flex items-center gap-4">
              <UserNav />
              <HeaderSignOut />
            </div>
          ) : null}
        </div>
      </header>
      <main className="mx-auto w-full max-w-6xl flex-1 px-6 py-8">
        {children}
      </main>
    </div>
  );
}
