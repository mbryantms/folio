"use client";

import Link from "next/link";

import { Skeleton } from "@/components/ui/skeleton";
import { formatTotalHours } from "@/lib/activity";
import { useAdminUsersStats } from "@/lib/api/queries";

/** Stats v2: list view of per-user reading aggregates. */
export function UsersTab() {
  const q = useAdminUsersStats();
  if (q.isLoading) return <Skeleton className="h-64 w-full" />;
  if (q.error || !q.data) {
    return <p className="text-destructive text-sm">Failed to load users.</p>;
  }
  if (q.data.users.length === 0) {
    return <p className="text-muted-foreground text-sm">No users yet.</p>;
  }
  return (
    <div className="border-border bg-card overflow-hidden rounded-md border">
      <table className="w-full text-sm">
        <thead className="bg-muted/50 text-muted-foreground text-xs uppercase">
          <tr>
            <th className="px-3 py-2 text-left font-semibold">User</th>
            <th className="px-3 py-2 text-right font-semibold">30d time</th>
            <th className="px-3 py-2 text-right font-semibold">30d sessions</th>
            <th className="px-3 py-2 text-right font-semibold">All time</th>
            <th className="px-3 py-2 text-left font-semibold">Top series</th>
            <th className="px-3 py-2 text-left font-semibold">Last active</th>
            <th className="px-3 py-2 text-right font-semibold">Devices</th>
          </tr>
        </thead>
        <tbody className="divide-border divide-y">
          {q.data.users.map((u) => (
            <tr key={u.user_id} className="hover:bg-muted/30">
              <td className="px-3 py-2">
                <Link
                  href={`/admin/users/${u.user_id}/activity`}
                  className="text-foreground font-medium hover:underline"
                >
                  {u.display_name}
                </Link>
                <div className="text-muted-foreground text-xs">
                  {u.email ?? "—"}
                  {u.excluded_from_aggregates ? (
                    <span className="bg-muted text-muted-foreground ml-2 inline-block rounded px-1 py-0.5 text-[10px] uppercase">
                      excluded
                    </span>
                  ) : null}
                </div>
              </td>
              <td className="px-3 py-2 text-right tabular-nums">
                {formatTotalHours(u.active_ms_30d / 3_600_000)}
              </td>
              <td className="text-muted-foreground px-3 py-2 text-right tabular-nums">
                {u.sessions_30d}
              </td>
              <td className="text-muted-foreground px-3 py-2 text-right tabular-nums">
                {formatTotalHours(u.active_ms_all_time / 3_600_000)} ·{" "}
                {u.sessions_all_time}
              </td>
              <td className="text-muted-foreground max-w-[200px] truncate px-3 py-2">
                {u.top_series_name ?? "—"}
              </td>
              <td className="text-muted-foreground px-3 py-2">
                {u.last_active_at
                  ? new Date(u.last_active_at).toLocaleString()
                  : "never"}
              </td>
              <td className="text-muted-foreground px-3 py-2 text-right text-xs">
                {u.device_breakdown.length > 0
                  ? u.device_breakdown
                      .slice(0, 3)
                      .map((d) => `${d.device} (${d.sessions})`)
                      .join(", ")
                  : "—"}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
