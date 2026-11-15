"use client";

/**
 * `/admin/upcoming`: what the stored Mylar snapshot says Mylar is doing
 * (upcoming-releases 1.0): the issues it wants, its download queue, what
 * it fetched lately, and when its own jobs run.
 *
 * Read-only by construction. Every row is something Mylar's host pushed
 * to Folio; there is no button here because Folio has no way to reach
 * Mylar. Admin-only: these titles span the whole collection.
 */
import { Loader2 } from "lucide-react";

import { ScrollArea } from "@/components/folio/scroll-area";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/folio/tabs";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { useAdminMylarActivity } from "@/lib/api/queries";
import type {
  MylarActivityView,
  MylarHistoryRow,
  MylarJobRow,
  MylarQueueItem,
  MylarWantedRow,
} from "@/lib/api/types";
import { formatRelativeDate } from "@/lib/format";
import { dayLabel } from "@/lib/log/overview";
import { type StatusTone, statusTone } from "@/lib/ui/status-tone";

/** Mylar's own status words, toned. Anything unknown stays neutral: the
 *  word is shown as Mylar wrote it. */
export function mylarStatusTone(status: string): StatusTone {
  switch (status.toLowerCase()) {
    case "failed":
      return "error";
    case "wanted":
      return "warning";
    case "snatched":
    case "downloading":
    case "queued":
      return "info";
    case "downloaded":
    case "post-processed":
    case "completed":
    case "archived":
      return "success";
    default:
      return "neutral";
  }
}

function StatusBadge({ status }: { status: string }) {
  return (
    <Badge variant="outline" className={statusTone(mylarStatusTone(status))}>
      {status}
    </Badge>
  );
}

function when(iso: string | null | undefined): string {
  return formatRelativeDate(iso) ?? "";
}

export function MylarActivityCard() {
  const q = useAdminMylarActivity();
  return (
    <Card className="lg:col-span-2">
      <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-2 pb-2">
        <CardTitle className="text-sm font-medium">
          What Mylar is doing
        </CardTitle>
        {q.data?.received_at ? (
          <span className="text-muted-foreground text-xs">
            As of {when(q.data.received_at)}
          </span>
        ) : null}
      </CardHeader>
      <CardContent className="space-y-3 text-sm">
        <p className="text-muted-foreground text-xs">
          Read from the last snapshot Mylar&rsquo;s host sent. Folio shows it
          and cannot change anything in Mylar.
        </p>
        {q.isLoading ? (
          <div className="text-muted-foreground flex items-center gap-2 py-4 text-sm">
            <Loader2 className="size-4 animate-spin" /> Loading…
          </div>
        ) : !q.data ? (
          <p className="text-destructive text-sm" role="alert">
            Couldn&rsquo;t load Mylar&rsquo;s activity.
          </p>
        ) : (
          <Activity data={q.data} />
        )}
      </CardContent>
    </Card>
  );
}

function Activity({ data }: { data: MylarActivityView }) {
  const queued = Object.values(data.queue.counts ?? {}).reduce(
    (sum, n) => sum + n,
    0,
  );
  return (
    <Tabs defaultValue="wanted">
      <TabsList>
        <TabsTrigger value="wanted">
          Wanted <Count n={data.wanted.length} />
        </TabsTrigger>
        <TabsTrigger value="queue">
          Download queue <Count n={queued} />
        </TabsTrigger>
        <TabsTrigger value="history">
          Recent history <Count n={data.history.length} />
        </TabsTrigger>
        <TabsTrigger value="jobs">Mylar jobs</TabsTrigger>
      </TabsList>
      <TabsContent value="wanted">
        <WantedTable rows={data.wanted} />
      </TabsContent>
      <TabsContent value="queue" className="space-y-3">
        <QueueCounts counts={data.queue.counts ?? {}} />
        <QueueTable rows={data.queue.items ?? []} total={queued} />
      </TabsContent>
      <TabsContent value="history">
        <HistoryTable rows={data.history} />
      </TabsContent>
      <TabsContent value="jobs">
        <JobsTable rows={data.jobs} />
      </TabsContent>
    </Tabs>
  );
}

function Count({ n }: { n: number }) {
  return (
    <span className="text-muted-foreground ml-1 tabular-nums">
      {n.toLocaleString()}
    </span>
  );
}

/** A table that scrolls inside the card once it is long, reachable by
 *  keyboard (the viewport takes focus). */
function Scroller({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <ScrollArea
      viewportClassName="max-h-96"
      viewportProps={{ tabIndex: 0, role: "region", "aria-label": label }}
    >
      {children}
    </ScrollArea>
  );
}

function Nothing({ children }: { children: React.ReactNode }) {
  return <p className="text-muted-foreground py-3 text-sm">{children}</p>;
}

function WantedTable({ rows }: { rows: MylarWantedRow[] }) {
  if (rows.length === 0) {
    return <Nothing>Mylar is not waiting on any issue.</Nothing>;
  }
  return (
    <Scroller label="Wanted issues">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Issue</TableHead>
            <TableHead className="w-32">In stores</TableHead>
            <TableHead className="w-28">Status</TableHead>
            <TableHead className="w-28 max-sm:hidden">Added</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((row, i) => (
            <TableRow
              key={`${row.cv_issue_id ?? row.series}-${row.issue}-${i}`}
            >
              <TableCell className="font-medium whitespace-normal">
                {row.series} #{row.issue}
              </TableCell>
              <TableCell className="text-muted-foreground whitespace-nowrap tabular-nums">
                {row.store_date ? dayLabel(row.store_date, THIS_YEAR) : "—"}
              </TableCell>
              <TableCell>
                <StatusBadge status={row.status} />
              </TableCell>
              <TableCell className="text-muted-foreground whitespace-nowrap max-sm:hidden">
                {when(row.added_at)}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </Scroller>
  );
}

/** A year no date carries, so `dayLabel` always prints the year: these
 *  lists reach back months and the admin page has no "today" to lean on. */
const THIS_YEAR = "0000";

function QueueCounts({ counts }: { counts: Record<string, number> }) {
  const entries = Object.entries(counts).sort((a, b) => b[1] - a[1]);
  if (entries.length === 0) return null;
  return (
    <ul className="flex flex-wrap gap-1.5" aria-label="Queue by status">
      {entries.map(([status, n]) => (
        <li key={status}>
          <Badge
            variant="outline"
            className={statusTone(mylarStatusTone(status))}
          >
            {status} {n.toLocaleString()}
          </Badge>
        </li>
      ))}
    </ul>
  );
}

function QueueTable({
  rows,
  total,
}: {
  rows: MylarQueueItem[];
  total: number;
}) {
  if (rows.length === 0) {
    return (
      <Nothing>Nothing is waiting in Mylar&rsquo;s download queue.</Nothing>
    );
  }
  return (
    <>
      {total > rows.length ? (
        <p className="text-muted-foreground text-xs">
          The {rows.length.toLocaleString()} most recently updated entries that
          are not completed. The counts above cover the whole queue.
        </p>
      ) : null}
      <Scroller label="Download queue">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Title</TableHead>
              <TableHead className="w-28">Status</TableHead>
              <TableHead className="w-24 max-sm:hidden">Size</TableHead>
              <TableHead className="w-28">Updated</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((row, i) => (
              <TableRow key={`${row.title}-${i}`}>
                <TableCell className="font-medium whitespace-normal">
                  {row.title}
                  {row.year ? (
                    <span className="text-muted-foreground"> ({row.year})</span>
                  ) : null}
                </TableCell>
                <TableCell>
                  <StatusBadge status={row.status} />
                </TableCell>
                <TableCell className="text-muted-foreground whitespace-nowrap max-sm:hidden">
                  {row.size ?? ""}
                </TableCell>
                <TableCell className="text-muted-foreground whitespace-nowrap">
                  {when(row.updated_at)}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </Scroller>
    </>
  );
}

function HistoryTable({ rows }: { rows: MylarHistoryRow[] }) {
  if (rows.length === 0) {
    return <Nothing>Mylar fetched nothing in the last two weeks.</Nothing>;
  }
  return (
    <Scroller label="Recent history">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Issue</TableHead>
            <TableHead className="w-36">Status</TableHead>
            <TableHead className="w-28">When</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((row, i) => (
            <TableRow
              key={`${row.series}-${row.issue ?? ""}-${row.at ?? i}-${i}`}
            >
              <TableCell className="font-medium whitespace-normal">
                {row.series}
                {row.issue ? ` #${row.issue}` : ""}
              </TableCell>
              <TableCell>
                <StatusBadge status={row.status} />
              </TableCell>
              <TableCell className="text-muted-foreground whitespace-nowrap">
                {when(row.at)}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </Scroller>
  );
}

function JobsTable({ rows }: { rows: MylarJobRow[] }) {
  if (rows.length === 0) {
    return <Nothing>The snapshot lists no Mylar jobs.</Nothing>;
  }
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Job</TableHead>
          <TableHead className="w-28">Status</TableHead>
          <TableHead className="w-32">Last run</TableHead>
          <TableHead className="w-44">Next run</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((row) => (
          <TableRow key={row.name}>
            <TableCell className="font-medium whitespace-normal">
              {row.name}
            </TableCell>
            <TableCell className="text-muted-foreground">
              {row.status ?? ""}
            </TableCell>
            <TableCell className="text-muted-foreground whitespace-nowrap">
              {when(row.prev_run)}
            </TableCell>
            <TableCell className="text-muted-foreground whitespace-nowrap">
              {row.next_run ? new Date(row.next_run).toLocaleString() : ""}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
