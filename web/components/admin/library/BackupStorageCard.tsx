"use client";

import { Archive, Loader2 } from "lucide-react";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useBackupStorage } from "@/lib/api/queries";

/**
 * `.bak` backup-storage rollup card (archive-rewrite-1.0 M7).
 *
 * Surfaces how much disk the edit/restore safety backups consume for a
 * library, so an operator can decide whether to lower the per-library
 * `archive_backup_retain_count` / `archive_backup_retain_days`. Backups are
 * the `<archive>.bak[.N]` siblings the rewrite path keeps on every edit.
 */
export function BackupStorageCard({ libraryId }: { libraryId: string }) {
  const q = useBackupStorage(libraryId);

  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="flex items-center gap-2 text-sm font-medium">
          <Archive className="h-4 w-4" /> Archive backups (.bak)
        </CardTitle>
      </CardHeader>
      <CardContent>
        {q.isLoading ? (
          <div className="text-muted-foreground flex items-center gap-2 text-sm">
            <Loader2 className="h-4 w-4 animate-spin" /> Scanning…
          </div>
        ) : !q.data ? (
          <p className="text-destructive text-sm">
            Failed to read backup storage.
          </p>
        ) : q.data.file_count === 0 ? (
          <p className="text-muted-foreground text-sm">
            No <code>.bak</code> backups on disk for this library.
          </p>
        ) : (
          <dl className="grid grid-cols-2 gap-x-6 gap-y-2 text-sm sm:grid-cols-4">
            <Stat label="Total size" value={formatBytes(q.data.total_bytes)} />
            <Stat label="Files" value={String(q.data.file_count)} />
            <Stat label="Oldest" value={formatDate(q.data.oldest_modified_at)} />
            <Stat label="Newest" value={formatDate(q.data.newest_modified_at)} />
          </dl>
        )}
      </CardContent>
    </Card>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0">
      <dt className="text-muted-foreground text-xs">{label}</dt>
      <dd className="truncate font-medium tabular-nums">{value}</dd>
    </div>
  );
}

function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(
    units.length - 1,
    Math.floor(Math.log(bytes) / Math.log(1024)),
  );
  const value = bytes / 1024 ** i;
  return `${value.toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

function formatDate(iso: string | null | undefined): string {
  if (!iso) return "—";
  return new Date(iso).toLocaleDateString();
}
