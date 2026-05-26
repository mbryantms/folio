"use client";

/**
 * Runs tab — paginated metadata_run history.
 */

import { ChevronRight, Loader2 } from "lucide-react";
import { useState } from "react";

import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
import {
  useAdminMetadataRun,
  useAdminMetadataRuns,
} from "@/lib/api/queries";
import type { RunRow } from "@/lib/api/types";

export function RunsTab() {
  const [scope, setScope] = useState<string>("");
  const [status, setStatus] = useState<string>("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const runs = useAdminMetadataRuns({
    scope: scope || undefined,
    status: status || undefined,
  });
  const rows = runs.data?.runs ?? [];

  return (
    <div className="grid gap-4 lg:grid-cols-[1fr_28rem]">
      <div className="space-y-3">
        <div className="flex flex-wrap items-center gap-2 text-sm">
          <span className="text-muted-foreground">Scope:</span>
          <FilterChip label="All" active={!scope} onClick={() => setScope("")} />
          <FilterChip label="Series" active={scope === "series"} onClick={() => setScope("series")} />
          <FilterChip label="Issue" active={scope === "issue"} onClick={() => setScope("issue")} />
          <span className="text-muted-foreground ml-3">Status:</span>
          <FilterChip label="All" active={!status} onClick={() => setStatus("")} />
          <FilterChip label="Completed" active={status === "completed"} onClick={() => setStatus("completed")} />
          <FilterChip label="Awaiting" active={status === "awaiting_quota"} onClick={() => setStatus("awaiting_quota")} />
          <FilterChip label="Failed" active={status === "failed"} onClick={() => setStatus("failed")} />
        </div>

        {runs.isLoading ? (
          <div className="text-muted-foreground flex items-center gap-2 py-6 text-sm">
            <Loader2 className="h-4 w-4 animate-spin" /> Loading…
          </div>
        ) : rows.length === 0 ? (
          <Card>
            <CardContent className="text-muted-foreground py-8 text-center text-sm">
              No runs match the current filter.
            </CardContent>
          </Card>
        ) : (
          <ul className="space-y-1.5">
            {rows.map((r) => (
              <RunListRow
                key={r.id}
                run={r}
                active={selectedId === r.id}
                onClick={() => setSelectedId(r.id)}
              />
            ))}
          </ul>
        )}
      </div>

      <div>
        {selectedId ? (
          <RunDetailPanel id={selectedId} />
        ) : (
          <Card>
            <CardContent className="text-muted-foreground py-12 text-center text-sm">
              Pick a run to see its candidates.
            </CardContent>
          </Card>
        )}
      </div>
    </div>
  );
}

function RunListRow({
  run,
  active,
  onClick,
}: {
  run: RunRow;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <li>
      <button
        onClick={onClick}
        className={`border-border bg-card hover:bg-muted w-full rounded border p-2 text-left text-sm transition-colors ${
          active ? "ring-foreground ring-1" : ""
        }`}
      >
        <div className="flex items-center justify-between gap-2">
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <StatusBadge status={run.status} />
              <span className="text-muted-foreground text-xs uppercase">
                {run.scope}
              </span>
              <span className="text-muted-foreground text-xs">
                · {run.trigger_kind}
              </span>
            </div>
            <p className="text-muted-foreground truncate text-xs">
              {new Date(run.started_at).toLocaleString()} ·{" "}
              {run.providers.join(", ")} ·{" "}
              {run.items_total > 0
                ? `${run.items_matched_high}H / ${run.items_matched_medium}M / ${run.items_matched_low}L`
                : "no candidates"}
            </p>
          </div>
          <ChevronRight className="text-muted-foreground h-4 w-4 shrink-0" />
        </div>
      </button>
    </li>
  );
}

function StatusBadge({ status }: { status: string }) {
  const variant =
    status === "completed"
      ? "default"
      : status === "failed"
        ? "destructive"
        : status === "awaiting_quota"
          ? "secondary"
          : "outline";
  return (
    <Badge variant={variant as "default" | "secondary" | "outline" | "destructive"}>
      {status}
    </Badge>
  );
}

function FilterChip({
  label,
  active,
  onClick,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={`rounded border px-2 py-0.5 text-xs ${
        active
          ? "border-foreground bg-foreground text-background"
          : "border-border bg-card hover:bg-muted"
      }`}
    >
      {label}
    </button>
  );
}

function RunDetailPanel({ id }: { id: string }) {
  const q = useAdminMetadataRun(id);
  if (q.isLoading) {
    return (
      <Card>
        <CardContent className="text-muted-foreground flex items-center gap-2 py-12 text-sm">
          <Loader2 className="h-4 w-4 animate-spin" /> Loading…
        </CardContent>
      </Card>
    );
  }
  if (!q.data) {
    return null;
  }
  const { run, candidates, query } = q.data;
  return (
    <Card>
      <CardContent className="space-y-3 p-4 text-sm">
        <div className="flex items-center gap-2">
          <StatusBadge status={run.status} />
          <span className="text-muted-foreground text-xs">
            {run.scope} · started{" "}
            {new Date(run.started_at).toLocaleString()}
          </span>
        </div>
        {query !== undefined && query !== null && (
          <pre className="bg-muted text-muted-foreground overflow-x-auto rounded p-2 text-xs">
            {JSON.stringify(query, null, 2)}
          </pre>
        )}
        {run.error_summary && (
          <p className="text-destructive text-xs">{run.error_summary}</p>
        )}
        <div className="text-muted-foreground grid grid-cols-3 gap-2 text-xs">
          <span>
            High: <strong>{run.items_matched_high}</strong>
          </span>
          <span>
            Medium: <strong>{run.items_matched_medium}</strong>
          </span>
          <span>
            Low: <strong>{run.items_matched_low}</strong>
          </span>
          <span>
            Applied: <strong>{run.items_applied}</strong>
          </span>
          <span>
            Skipped: <strong>{run.items_skipped}</strong>
          </span>
          <span>
            Total: <strong>{run.items_total}</strong>
          </span>
        </div>
        <hr className="border-border my-2" />
        <h4 className="text-xs font-medium">Candidates</h4>
        {candidates.length === 0 ? (
          <p className="text-muted-foreground text-xs">No candidates.</p>
        ) : (
          <ul className="space-y-1">
            {candidates.map((c) => (
              <li
                key={c.ordinal}
                className="border-border flex items-center justify-between gap-2 rounded border p-1.5 text-xs"
              >
                <div className="min-w-0">
                  <span className="font-medium capitalize">{c.source}</span>{" "}
                  <span className="text-muted-foreground">
                    #{c.external_id} · {c.bucket} · {c.score.toFixed(1)}
                  </span>
                </div>
                {c.applied_at && (
                  <Badge variant="default" className="text-[10px]">
                    Applied
                  </Badge>
                )}
                {c.dismissed_at && !c.applied_at && (
                  <Badge variant="outline" className="text-[10px]">
                    Dismissed
                  </Badge>
                )}
              </li>
            ))}
          </ul>
        )}
      </CardContent>
    </Card>
  );
}
