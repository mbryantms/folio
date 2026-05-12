"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import type { MeView } from "@/lib/api/types";

export function UserNav() {
  const [me, setMe] = useState<MeView | null>(null);

  useEffect(() => {
    let cancelled = false;
    void fetch("/api/auth/me", { credentials: "include" })
      .then((r) => (r.ok ? (r.json() as Promise<MeView>) : null))
      .then((v) => {
        if (!cancelled) setMe(v);
      })
      .catch(() => {
        /* unauthenticated */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (!me) return null;
  return (
    <nav
      aria-label="User navigation"
      className="flex items-center gap-3 text-xs"
    >
      {me.role === "admin" ? (
        <Link
          href={`/admin`}
          className="text-neutral-400 hover:text-neutral-100"
        >
          Admin
        </Link>
      ) : null}
      <Link
        href={`/settings`}
        className="text-neutral-400 hover:text-neutral-100"
      >
        Settings
      </Link>
    </nav>
  );
}
