import * as React from "react";
import { AlertCircle } from "lucide-react";

/**
 * Shared building blocks for every error / not-found surface, so they
 * stay visually consistent and theme-aware (replaces the legacy
 * [`Chrome`](./Chrome.tsx) shell those pages used to wrap themselves in).
 *
 * - [`StatusCard`] — the centered content block (optional code/icon,
 *   title, description, digest ref, action row). Rendered on its own
 *   inside the route-group error boundaries, which already sit within
 *   `MainShell` / `AdminShell`.
 * - [`StatusScreen`] — a minimal full-screen brand frame wrapping a
 *   `StatusCard`, for the top-level pages (404, locale error,
 *   global-error) that render *outside* the app shell. No data fetching,
 *   so it's safe in both server and client components.
 */
export function StatusCard({
  code,
  icon,
  title,
  description,
  digest,
  actions,
}: {
  /** Short status code shown above the title, e.g. "404". */
  code?: string;
  icon?: React.ReactNode;
  title: string;
  description: React.ReactNode;
  /** Next.js error digest — correlates to the server-side log row. */
  digest?: string;
  actions: React.ReactNode;
}) {
  return (
    <div className="mx-auto max-w-md py-12 text-center">
      {code ? (
        <p className="text-muted-foreground text-xs font-medium tracking-widest uppercase">
          {code}
        </p>
      ) : null}
      {icon ? <div className="mt-3 flex justify-center">{icon}</div> : null}
      <h1 className="mt-3 text-2xl font-semibold tracking-tight">{title}</h1>
      <div className="text-muted-foreground mt-3 text-sm text-pretty">
        {description}
      </div>
      {digest ? (
        <p className="text-muted-foreground/70 mt-4 font-mono text-[11px]">
          ref: {digest}
        </p>
      ) : null}
      <div className="mt-8 flex flex-wrap items-center justify-center gap-3">
        {actions}
      </div>
    </div>
  );
}

/** Destructive circle icon used by every error (not not-found) surface. */
export function StatusErrorIcon() {
  return (
    <span className="border-destructive/40 bg-destructive/10 text-destructive flex size-12 items-center justify-center rounded-full border">
      <AlertCircle className="size-6" aria-hidden />
    </span>
  );
}

export function StatusScreen(props: React.ComponentProps<typeof StatusCard>) {
  return (
    <div className="bg-background text-foreground flex min-h-screen flex-col">
      {/* Minimal brand header — wordmark matches `MainShell`. A hard
          `<a href>` (not next/link) so it works even from `global-error`
          where the router may be unavailable. */}
      <header className="border-border flex h-14 shrink-0 items-center border-b px-[max(1rem,var(--safe-left))] pt-(--safe-top) md:px-6">
        <a
          href="/"
          className="hover:text-foreground/80 font-semibold tracking-tight transition-colors"
        >
          Folio
        </a>
      </header>
      <main className="grid flex-1 place-items-center px-4">
        <StatusCard {...props} />
      </main>
    </div>
  );
}
