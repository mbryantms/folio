"use client";

import {
  CartesianGrid,
  Cell,
  Legend,
  Line,
  LineChart,
  Pie,
  PieChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { formatDurationMs } from "@/lib/activity";
import { useAdminEngagement } from "@/lib/api/queries";

/** Stats v2: DAU/WAU/MAU line + device-30d donut. */
export function EngagementTab() {
  const q = useAdminEngagement();
  if (q.isLoading) return <Skeleton className="h-64 w-full" />;
  if (q.error || !q.data) {
    return (
      <p className="text-destructive text-sm">Failed to load engagement.</p>
    );
  }
  const data = q.data;
  const rows = data.series.map((p) => ({
    ts: new Date(p.date).getTime(),
    dau: p.dau,
    wau: p.wau,
    mau: p.mau,
  }));

  return (
    <div className="space-y-4">
      <Card>
        <CardHeader>
          <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
            Active users — DAU / WAU / MAU
          </CardTitle>
          <p className="text-muted-foreground text-xs">
            Trailing-window distinct user counts across the last 90 days.
            Excluded users are filtered out.
          </p>
        </CardHeader>
        <CardContent>
          <ResponsiveContainer width="100%" height={220}>
            <LineChart
              data={rows}
              margin={{ top: 6, right: 4, left: 0, bottom: 0 }}
            >
              <CartesianGrid
                stroke="var(--color-border)"
                strokeDasharray="2 2"
                vertical={false}
              />
              <XAxis
                dataKey="ts"
                type="number"
                domain={["dataMin", "dataMax"]}
                tickFormatter={tickDate}
                tick={{ fill: "var(--color-muted-foreground)", fontSize: 10 }}
                stroke="var(--color-border)"
                minTickGap={32}
              />
              <YAxis
                tick={{ fill: "var(--color-muted-foreground)", fontSize: 10 }}
                stroke="var(--color-border)"
                allowDecimals={false}
                width={32}
              />
              <Tooltip
                cursor={{ stroke: "var(--color-muted)", strokeWidth: 1 }}
                contentStyle={{
                  background: "var(--color-popover)",
                  border: "1px solid var(--color-border)",
                  borderRadius: 6,
                  fontSize: 12,
                }}
                labelFormatter={(label) =>
                  typeof label === "number"
                    ? new Date(label).toLocaleDateString()
                    : String(label ?? "")
                }
              />
              <Legend wrapperStyle={{ fontSize: 11 }} />
              <Line
                type="monotone"
                dataKey="dau"
                name="DAU"
                stroke="var(--color-primary)"
                strokeWidth={2}
                dot={false}
                isAnimationActive={false}
              />
              <Line
                type="monotone"
                dataKey="wau"
                name="WAU"
                stroke="var(--color-primary)"
                strokeOpacity={0.65}
                strokeWidth={1.6}
                dot={false}
                isAnimationActive={false}
              />
              <Line
                type="monotone"
                dataKey="mau"
                name="MAU"
                stroke="var(--color-primary)"
                strokeOpacity={0.35}
                strokeWidth={1.4}
                dot={false}
                isAnimationActive={false}
              />
            </LineChart>
          </ResponsiveContainer>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-muted-foreground text-sm font-semibold tracking-wide uppercase">
            Devices (last 30 days)
          </CardTitle>
        </CardHeader>
        <CardContent>
          {data.devices_30d.length === 0 ? (
            <p className="text-muted-foreground text-sm">
              No device data recorded in the last 30 days.
            </p>
          ) : (
            <div className="grid grid-cols-1 items-center gap-4 sm:grid-cols-2">
              <ResponsiveContainer width="100%" height={180}>
                <PieChart>
                  <Pie
                    data={data.devices_30d}
                    dataKey="active_ms"
                    nameKey="device"
                    innerRadius={50}
                    outerRadius={80}
                    paddingAngle={2}
                    stroke="var(--color-background)"
                    strokeWidth={2}
                  >
                    {data.devices_30d.map((_, i) => (
                      <Cell
                        key={i}
                        fill="var(--color-primary)"
                        fillOpacity={Math.max(
                          0.3,
                          1 - i * (1 / Math.max(data.devices_30d.length, 1)),
                        )}
                      />
                    ))}
                  </Pie>
                  <Tooltip
                    contentStyle={{
                      background: "var(--color-popover)",
                      border: "1px solid var(--color-border)",
                      borderRadius: 6,
                      fontSize: 12,
                    }}
                    formatter={(value: unknown) => [
                      formatDurationMs(Number(value)),
                      "Time",
                    ]}
                  />
                </PieChart>
              </ResponsiveContainer>
              <ul className="space-y-1">
                {data.devices_30d.map((d) => (
                  <li
                    key={d.device}
                    className="flex items-center justify-between gap-3 text-sm"
                  >
                    <span className="font-medium">{d.device}</span>
                    <span className="text-muted-foreground tabular-nums">
                      {formatDurationMs(d.active_ms)} · {d.sessions} session
                      {d.sessions === 1 ? "" : "s"}
                    </span>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}

function tickDate(ms: number): string {
  return new Date(ms).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}
