"use client";

import type * as React from "react";
import { usePathname } from "next/navigation";

import {
  ActivityFeedSkeleton,
  AdminHeaderSkeleton,
  AuthConfigSkeleton,
  BackgroundWorkSkeleton,
  DashboardSkeleton,
  DuplicatesOverviewSkeleton,
  EmailSkeleton,
  FindingsSkeleton,
  LibraryDetailSkeleton,
  LibraryListSkeleton,
  LogsSkeleton,
  MetadataTabsSkeleton,
  QueuePageSkeleton,
  RelationshipsSkeleton,
  ScanDashboardSkeleton,
  SeriesIdentitySkeleton,
  ServerPageSkeleton,
  StarterListsSkeleton,
  StatsTabsSkeleton,
  UserDetailSkeleton,
  UserTableSkeleton,
} from "@/components/skeletons/AdminShapes";
import {
  LoadingRegion,
  RowsSkeleton,
} from "@/components/skeletons/LoadingShapes";
import { Skeleton } from "@/components/ui/skeleton";

/** Header geometry (title / description length in characters) and body
 *  for each admin route, matching `app/[locale]/(admin)/admin/**`'s
 *  server-rendered `PageHeader` and the client body's loading shape. */
type Shape = {
  header?: { title: number; description: number; actions?: React.ReactNode };
  body: React.ReactNode;
};

const LIBRARIES_ACTIONS = (
  <>
    <Skeleton className="h-9 w-24 rounded-md" />
    <Skeleton className="h-9 w-28 rounded-md" />
  </>
);

/** The admin sub-path (`""`, `"/users"`, `"/libraries/x/health"`) of a
 *  pathname, tolerating a locale prefix. */
export function adminSubpath(pathname: string): string {
  const m = /(?:^|\/)admin(\/.*)?$/.exec(pathname.replace(/\/+$/, ""));
  return m?.[1] ?? "";
}

export function adminRouteShape(sub: string): Shape {
  const parts = sub.split("/").filter(Boolean);
  const [first, second, third] = parts;
  switch (first) {
    case undefined:
      return {
        header: { title: 9, description: 70 },
        body: <DashboardSkeleton label="" />,
      };
    case "activity":
    case "audit":
      return {
        header: { title: 15, description: 163 },
        body: <ActivityFeedSkeleton />,
      };
    case "auth":
      return {
        header: { title: 11, description: 139 },
        body: <AuthConfigSkeleton label="" />,
      };
    case "background-work":
      return {
        header: { title: 15, description: 166 },
        body: <BackgroundWorkSkeleton label="" />,
      };
    case "duplicates":
      return {
        header: { title: 10, description: 165 },
        body: <DuplicatesOverviewSkeleton />,
      };
    case "email":
      return {
        header: { title: 5, description: 122 },
        body: <EmailSkeleton label="" />,
      };
    case "findings":
      return {
        header: { title: 14, description: 170 },
        body: <FindingsSkeleton />,
      };
    case "libraries":
      if (second) return { body: <LibraryDetailSkeleton tab={third ?? ""} /> };
      return {
        header: { title: 9, description: 93, actions: LIBRARIES_ACTIONS },
        body: <LibraryListSkeleton label="" />,
      };
    case "logs":
      return {
        header: { title: 10, description: 201 },
        body: <LogsSkeleton />,
      };
    case "metadata":
      return {
        header: { title: 8, description: 129 },
        body: <MetadataTabsSkeleton />,
      };
    case "queue":
      return {
        header: { title: 5, description: 166 },
        body: <QueuePageSkeleton />,
      };
    case "relationships":
      return {
        header: { title: 13, description: 238 },
        body: <RelationshipsSkeleton />,
      };
    case "scan-dashboard":
      return {
        header: { title: 14, description: 172 },
        body: <ScanDashboardSkeleton />,
      };
    case "series":
      return { body: <SeriesIdentitySkeleton label="" /> };
    case "server":
      // The header sits inside the page's own `space-y-6` stack.
      return { body: <ServerPageSkeleton /> };
    case "starter-lists":
      return {
        header: { title: 13, description: 145 },
        body: <StarterListsSkeleton />,
      };
    case "stats":
      return {
        header: { title: 5, description: 97 },
        body: <StatsTabsSkeleton />,
      };
    case "users":
      if (second) return { body: <UserDetailSkeleton label="" /> };
      return {
        header: { title: 5, description: 61 },
        body: <UserTableSkeleton />,
      };
    case "api-docs":
      return {
        header: { title: 13, description: 118 },
        body: <Skeleton className="h-[70vh] w-full rounded-lg" />,
      };
    default:
      return {
        header: { title: 12, description: 120 },
        body: <RowsSkeleton label="" />,
      };
  }
}

/** `(admin)` route fallback body: the entered page's header and the same
 *  body shape its client component shows while loading. */
export function AdminRouteSkeleton() {
  const shape = adminRouteShape(adminSubpath(usePathname() ?? ""));
  return (
    <LoadingRegion label="Loading…">
      {shape.header ? (
        <AdminHeaderSkeleton
          title={shape.header.title}
          description={shape.header.description}
          actions={shape.header.actions}
        />
      ) : null}
      {shape.body}
    </LoadingRegion>
  );
}
