"use client";

/**
 * `<ReviewQueueTab>` — /admin/metadata `?tab=review` (M6).
 *
 * Lists pending medium + low candidates (no `applied_at`, no
 * `dismissed_at`) so an operator can resolve them in bulk. Each row
 * carries the run_id + ordinal so a future "Review" action can
 * deep-link to the MetadataMatchDialog with the pre-selected
 * candidate; for v1 we surface a "Dismiss" action to clear noise.
 */

import { Loader2, X } from "lucide-react";
import { useState } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useDismissMetadataCandidate } from "@/lib/api/mutations";
import { useAdminMetadataReviewQueue } from "@/lib/api/queries";
import type { ReviewItem } from "@/lib/api/types";

export function ReviewQueueTab() {
  const [bucket, setBucket] = useState<string>("");
  const q = useAdminMetadataReviewQueue({ bucket: bucket || undefined });
  const items = q.data?.items ?? [];

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <span className="text-muted-foreground">Bucket:</span>
        <FilterChip
          label="All"
          active={!bucket}
          onClick={() => setBucket("")}
        />
        <FilterChip
          label="Medium"
          active={bucket === "medium"}
          onClick={() => setBucket("medium")}
        />
        <FilterChip
          label="Low"
          active={bucket === "low"}
          onClick={() => setBucket("low")}
        />
      </div>
      {q.isLoading ? (
        <div className="text-muted-foreground flex items-center gap-2 py-6 text-sm">
          <Loader2 className="h-4 w-4 animate-spin" /> Loading…
        </div>
      ) : items.length === 0 ? (
        <Card>
          <CardContent className="text-muted-foreground py-8 text-center text-sm">
            Nothing to review. Empty queues mean every candidate either
            applied automatically (HIGH bucket) or got dismissed.
          </CardContent>
        </Card>
      ) : (
        <ul className="space-y-1.5">
          {items.map((it) => (
            <ReviewRow key={`${it.run_id}-${it.ordinal}`} item={it} />
          ))}
        </ul>
      )}
    </div>
  );
}

function ReviewRow({ item }: { item: ReviewItem }) {
  const dismiss = useDismissMetadataCandidate();
  const parsed = parseCandidate(item.candidate);
  return (
    <li className="border-border bg-card rounded border p-2 text-sm">
      <div className="flex items-center justify-between gap-2">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <Badge
              variant={item.bucket === "medium" ? "secondary" : "outline"}
              className="text-[10px] uppercase"
            >
              {item.bucket}
            </Badge>
            <span className="text-muted-foreground text-xs">
              {item.score.toFixed(1)}
            </span>
            <span className="text-muted-foreground text-xs">·</span>
            <span className="text-muted-foreground text-xs uppercase">
              {item.scope}
            </span>
            {parsed.name && (
              <span className="truncate font-medium">{parsed.name}</span>
            )}
            <span className="text-muted-foreground text-xs">
              from {item.source}
            </span>
          </div>
          <p className="text-muted-foreground truncate text-xs">
            {parsed.year ? `${parsed.year} · ` : ""}
            {parsed.publisher ?? "—"} ·{" "}
            {new Date(item.run_started_at).toLocaleString()}
          </p>
        </div>
        <Button
          size="sm"
          variant="ghost"
          onClick={() =>
            dismiss.mutate({ runId: item.run_id, ordinal: item.ordinal })
          }
          disabled={dismiss.isPending}
          aria-label="Dismiss"
        >
          <X className="h-3.5 w-3.5" />
        </Button>
      </div>
    </li>
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

function parseCandidate(payload: unknown): {
  name: string | null;
  year: number | null;
  publisher: string | null;
} {
  if (!payload || typeof payload !== "object") {
    return { name: null, year: null, publisher: null };
  }
  const obj = payload as Record<string, unknown>;
  return {
    name: typeof obj.name === "string" ? obj.name : null,
    year: typeof obj.year === "number" ? obj.year : null,
    publisher: typeof obj.publisher === "string" ? obj.publisher : null,
  };
}
