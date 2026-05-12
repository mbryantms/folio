"use client";

import * as React from "react";
import type { ColumnDef } from "@tanstack/react-table";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable } from "@/components/ui/data-table";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { useAuditLog, type AuditFilters } from "@/lib/api/queries";
import type { AuditEntryView } from "@/lib/api/types";

function actionVariant(
  action: string,
): "default" | "secondary" | "destructive" {
  if (action.endsWith(".disable") || action.endsWith(".delete"))
    return "destructive";
  if (action.startsWith("admin.")) return "default";
  return "secondary";
}

function actorDisplay(entry: AuditEntryView): {
  primary: string;
  secondary: string | null;
} {
  if (entry.actor_label) {
    return { primary: entry.actor_label, secondary: entry.actor_type };
  }
  return {
    primary: `${entry.actor_type}:${entry.actor_id.slice(0, 8)}…`,
    secondary: null,
  };
}

function targetDisplay(entry: AuditEntryView): {
  primary: string;
  secondary: string | null;
} {
  if (!entry.target_type) return { primary: "—", secondary: null };
  if (entry.target_label) {
    return { primary: entry.target_label, secondary: entry.target_type };
  }
  if (entry.target_id) {
    return {
      primary: `${entry.target_type}:${entry.target_id.slice(0, 8)}…`,
      secondary: null,
    };
  }
  return { primary: entry.target_type, secondary: null };
}

export interface AuditTableProps {
  /**
   * When provided, the table locks the actor filter to this user id and hides
   * the corresponding control. Used by the per-user "Activity" tab.
   */
  pinnedActorId?: string;
}

export function AuditTable({ pinnedActorId }: AuditTableProps) {
  const [actionInput, setActionInput] = React.useState("");
  const [actorInput, setActorInput] = React.useState("");
  const [sinceInput, setSinceInput] = React.useState("");
  const [debounced, setDebounced] = React.useState({
    action: "",
    actor: "",
    since: "",
  });
  const [cursor, setCursor] = React.useState<string | undefined>(undefined);
  const [history, setHistory] = React.useState<(string | undefined)[]>([]);

  React.useEffect(() => {
    const t = setTimeout(() => {
      setDebounced({
        action: actionInput.trim(),
        actor: actorInput.trim(),
        since: sinceInput.trim(),
      });
      setCursor(undefined);
      setHistory([]);
    }, 250);
    return () => clearTimeout(t);
  }, [actionInput, actorInput, sinceInput]);

  const filters: AuditFilters = React.useMemo(
    () => ({
      actor_id: pinnedActorId ?? (debounced.actor || undefined),
      action: debounced.action || undefined,
      // <input type="datetime-local"> emits `YYYY-MM-DDTHH:mm` (no timezone);
      // append a `:00` and treat it as local time before serialising.
      since: debounced.since
        ? new Date(`${debounced.since}:00`).toISOString()
        : undefined,
      cursor,
      limit: 100,
    }),
    [pinnedActorId, debounced, cursor],
  );

  const { data, isLoading, error, isFetching } = useAuditLog(filters);

  const columns = React.useMemo<ColumnDef<AuditEntryView>[]>(
    () => [
      {
        accessorKey: "created_at",
        header: "When",
        cell: ({ row }) => (
          <span className="text-muted-foreground text-xs">
            {new Date(row.original.created_at).toLocaleString()}
          </span>
        ),
      },
      {
        accessorKey: "action",
        header: "Action",
        cell: ({ row }) => (
          <Badge
            variant={actionVariant(row.original.action)}
            className="font-mono text-[10px] tracking-tight uppercase"
          >
            {row.original.action}
          </Badge>
        ),
      },
      {
        id: "actor",
        header: "Actor",
        cell: ({ row }) => {
          const d = actorDisplay(row.original);
          return (
            <div className="flex flex-col">
              <span className="text-sm">{d.primary}</span>
              {d.secondary ? (
                <span className="text-muted-foreground/70 text-[10px] tracking-wider uppercase">
                  {d.secondary}
                </span>
              ) : null}
            </div>
          );
        },
      },
      {
        id: "target",
        header: "Target",
        cell: ({ row }) => {
          const d = targetDisplay(row.original);
          return (
            <div className="flex flex-col">
              <span className="text-sm">{d.primary}</span>
              {d.secondary ? (
                <span className="text-muted-foreground/70 text-[10px] tracking-wider uppercase">
                  {d.secondary}
                </span>
              ) : null}
            </div>
          );
        },
      },
      {
        id: "ip",
        header: "IP",
        cell: ({ row }) => (
          <span className="text-muted-foreground font-mono text-[11px]">
            {row.original.ip ?? "—"}
          </span>
        ),
      },
    ],
    [],
  );

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-2">
        <Input
          value={actionInput}
          onChange={(e) => setActionInput(e.target.value)}
          placeholder="Action (e.g. admin.user.* )"
          className="h-8 w-64"
        />
        {pinnedActorId ? null : (
          <Input
            value={actorInput}
            onChange={(e) => setActorInput(e.target.value)}
            placeholder="Actor UUID"
            className="h-8 w-72 font-mono text-xs"
          />
        )}
        <Input
          type="datetime-local"
          value={sinceInput}
          onChange={(e) => setSinceInput(e.target.value)}
          className="h-8 w-56"
        />
        {(actionInput || actorInput || sinceInput) && (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              setActionInput("");
              setActorInput("");
              setSinceInput("");
            }}
          >
            Clear
          </Button>
        )}
      </div>

      {isLoading ? (
        <Skeleton className="h-64 w-full" />
      ) : error ? (
        <p className="text-destructive text-sm">{error.message}</p>
      ) : (
        <DataTable
          columns={columns}
          data={data?.items ?? []}
          emptyMessage="No audit entries match these filters."
          renderExpanded={(row) => (
            <pre className="bg-background/60 overflow-x-auto rounded p-3 font-mono text-[11px] leading-relaxed">
              {JSON.stringify(row.original.payload ?? {}, null, 2)}
            </pre>
          )}
        />
      )}

      <div className="text-muted-foreground flex items-center justify-end gap-2 text-xs">
        <Button
          size="sm"
          variant="ghost"
          disabled={history.length === 0 || isFetching}
          onClick={() => {
            setHistory((prev) => {
              const next = [...prev];
              const back = next.pop();
              setCursor(back);
              return next;
            });
          }}
        >
          ← Previous
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={!data?.next_cursor || isFetching}
          onClick={() => {
            setHistory((prev) => [...prev, cursor]);
            setCursor(data?.next_cursor ?? undefined);
          }}
        >
          Next →
        </Button>
      </div>
    </div>
  );
}
