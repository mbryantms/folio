"use client";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { StatCard } from "@/components/admin/StatCard";
import { useAdminQuality } from "@/lib/api/queries";

/**
 * Stats v2: data-quality diagnostics. Orphan / long / dangling sessions
 * plus metadata coverage shortcuts.
 */
export function QualityTab() {
  const q = useAdminQuality();
  if (q.isLoading) return <Skeleton className="h-64 w-full" />;
  if (q.error || !q.data) {
    return (
      <p className="text-destructive text-sm">Failed to load quality stats.</p>
    );
  }
  const data = q.data;

  return (
    <div className="space-y-4">
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
        <StatCard
          label="Orphan sessions"
          value={data.orphan_sessions}
          hint="reading_sessions whose issue row is missing"
        />
        <StatCard
          label="Long sessions"
          value={data.long_sessions}
          hint="active_ms > 6h or span > 12h"
        />
        <StatCard
          label="Dangling sessions"
          value={data.dangling_sessions}
          hint="open > 1h with no heartbeat"
        />
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
            Metadata coverage
          </CardTitle>
          <p className="text-muted-foreground text-xs">
            How many active, on-disk issues are missing each common field.
          </p>
        </CardHeader>
        <CardContent>
          <table className="w-full text-sm">
            <thead className="text-muted-foreground text-xs uppercase">
              <tr>
                <th className="px-1 py-1 text-left font-semibold">Field</th>
                <th className="px-1 py-1 text-right font-semibold">Missing</th>
                <th className="px-1 py-1 text-right font-semibold">
                  % of {data.metadata.total_issues}
                </th>
              </tr>
            </thead>
            <tbody className="divide-border divide-y">
              {[
                { label: "Writer", missing: data.metadata.missing_writer },
                {
                  label: "Cover artist",
                  missing: data.metadata.missing_cover_artist,
                },
                {
                  label: "Page count",
                  missing: data.metadata.missing_page_count,
                },
                { label: "Genre", missing: data.metadata.missing_genre },
                {
                  label: "Publisher",
                  missing: data.metadata.missing_publisher,
                },
              ].map((row) => {
                const pct =
                  data.metadata.total_issues > 0
                    ? Math.round(
                        (row.missing / data.metadata.total_issues) * 100,
                      )
                    : 0;
                return (
                  <tr key={row.label}>
                    <td className="px-1 py-1.5 font-medium">{row.label}</td>
                    <td className="px-1 py-1.5 text-right tabular-nums">
                      {row.missing.toLocaleString()}
                    </td>
                    <td className="text-muted-foreground px-1 py-1.5 text-right tabular-nums">
                      {pct}%
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </CardContent>
      </Card>
    </div>
  );
}
