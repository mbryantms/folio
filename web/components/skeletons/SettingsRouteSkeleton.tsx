"use client";

import { usePathname } from "next/navigation";
import type * as React from "react";

import { LoadingRegion } from "@/components/skeletons/LoadingShapes";
import {
  AccountBodySkeleton,
  ActivityBodySkeleton,
  ApiTokensBodySkeleton,
  DownloadsBodySkeleton,
  HiddenRailItemsSkeleton,
  KeybindsBodySkeleton,
  PagesManagerSkeleton,
  ReadingBodySkeleton,
  SettingsIndexBodySkeleton,
  SidebarLayoutSkeleton,
  ThemeBodySkeleton,
  ViewsTabsSkeleton,
} from "@/components/skeletons/SettingsShapes";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

/** `PageHeader` stand-in with one width per description line (the Views
 *  description wraps to two at desktop widths). Same line boxes, border
 *  and margins as `components/admin/PageHeader`. */
function SettingsHeaderSkeleton({
  title,
  description,
  action,
}: {
  title: string;
  description: readonly string[];
  /** Width class of an outline button in the actions slot. */
  action?: string;
}) {
  return (
    <div className="border-border mb-6 flex flex-wrap items-end justify-between gap-4 border-b pb-4 max-sm:mb-4 max-sm:pb-3">
      <div className="min-w-0 flex-1">
        <div className="flex h-8 items-center max-sm:h-7">
          <Skeleton className={cn("h-6 max-w-full max-sm:h-5", title)} />
        </div>
        <div className="mt-1">
          {description.map((w, i) => (
            <div key={i} className="flex h-5 items-center max-sm:h-4">
              <Skeleton className={cn("h-3.5 max-w-full", w)} />
            </div>
          ))}
        </div>
      </div>
      {action ? (
        <Skeleton className={cn("h-9 shrink-0 rounded-md", action)} />
      ) : null}
    </div>
  );
}

type Page = {
  title: string;
  description: readonly string[];
  action?: string;
  label: string;
  body: React.ReactNode;
  /** Pages that put the header inside their own `space-y-*` stack. */
  stack?: string;
};

const PAGES: Record<string, Page> = {
  "": {
    title: "w-28",
    description: ["w-[29rem]"],
    label: "Loading settings…",
    body: <SettingsIndexBodySkeleton />,
    stack: "space-y-8",
  },
  account: {
    title: "w-28",
    description: ["w-[30rem]"],
    label: "Loading account…",
    body: <AccountBodySkeleton />,
  },
  activity: {
    title: "w-64",
    description: ["w-[40rem]"],
    label: "Loading reading history settings…",
    body: <ActivityBodySkeleton />,
  },
  "api-tokens": {
    title: "w-40",
    description: ["w-[39rem]"],
    label: "Loading app passwords…",
    body: <ApiTokensBodySkeleton />,
  },
  downloads: {
    title: "w-28",
    description: ["w-[54rem]"],
    action: "w-32",
    label: "Loading downloads…",
    body: <DownloadsBodySkeleton />,
  },
  keybinds: {
    title: "w-48",
    description: ["w-[34rem]"],
    label: "Loading keyboard shortcuts…",
    body: <KeybindsBodySkeleton />,
  },
  pages: {
    title: "w-20",
    description: ["w-[61rem]"],
    label: "Loading pages…",
    body: (
      <>
        <PagesManagerSkeleton />
        <div className="mt-10">
          <HiddenRailItemsSkeleton />
        </div>
      </>
    ),
  },
  reading: {
    title: "w-24",
    description: ["w-[48rem]"],
    label: "Loading reading settings…",
    body: <ReadingBodySkeleton />,
  },
  sidebar: {
    title: "w-24",
    description: ["w-[56rem]"],
    label: "Loading sidebar layout…",
    body: <SidebarLayoutSkeleton />,
  },
  theme: {
    title: "w-20",
    description: ["w-[38rem]"],
    label: "Loading theme settings…",
    body: <ThemeBodySkeleton />,
  },
  views: {
    title: "w-20",
    description: ["w-full", "w-56"],
    label: "Loading views…",
    body: <ViewsTabsSkeleton />,
    stack: "space-y-6",
  },
};

/** The settings page a pathname points at (`/settings/<page>`, with or
 *  without a locale prefix); "" for the index. */
export function settingsPageKey(pathname: string | null): string {
  const parts = (pathname ?? "").split("/").filter(Boolean);
  const at = parts.indexOf("settings");
  return at < 0 ? "" : (parts[at + 1] ?? "");
}

/**
 * `(settings)` route fallback: the `PageHeader` plus the body shape of the
 * page being opened, picked from the pathname (the fallback renders for
 * every settings route). Bodies are the same shapes the pages' client
 * components show while their queries load, so the handoff holds still.
 * Unknown pages fall back to a generic stack of section cards.
 */
export function SettingsRouteSkeleton() {
  const key = settingsPageKey(usePathname());
  const page = PAGES[key];
  if (!page) {
    return (
      <LoadingRegion label="Loading…">
        <SettingsHeaderSkeleton title="w-40" description={["w-96"]} />
        <ThemeBodySkeleton />
      </LoadingRegion>
    );
  }
  const header = (
    <SettingsHeaderSkeleton
      title={page.title}
      description={page.description}
      action={page.action}
    />
  );
  return (
    <LoadingRegion label={page.label} className={page.stack}>
      {header}
      {page.body}
    </LoadingRegion>
  );
}
