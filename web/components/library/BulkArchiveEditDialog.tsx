"use client";

import * as React from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { summarizeBulkArchiveSkips } from "@/lib/library/bulk-archive-skips";
import { useBulkArchiveEditMutation } from "@/lib/api/mutations";
import type { BulkArchiveOp, BulkEditResponse } from "@/lib/api/types";

/**
 * "Edit archives…" — `archive-rewrite-1.0` M7.
 *
 * Wired from the SelectionToolbar overflow on the issue-list surfaces.
 * Captures a single *relative* op (rotate cover/all, remove first/last N)
 * and fans it out to a per-issue edit job via `POST /archive/bulk-edit`.
 * Ops are relative because page counts differ per issue — "remove the last
 * page" lowers to the right ordinal on every selected archive.
 *
 * Admin-gated at the call site; the server additionally skips issues whose
 * library has writeback disabled or whose format isn't editable, and reports
 * them back as `skipped` — so the summary toast tells the operator exactly
 * how many ran versus were left alone, grouped by reason (audit B17).
 *
 * When `onResult` is supplied the caller owns the close + selection: it keeps
 * the skipped issues selected so the operator can fix the cause and retry,
 * instead of the whole selection being dropped. Without it the dialog falls
 * back to closing itself (standalone usage).
 */

type OpKind = "rotate_cover" | "rotate_all" | "remove_first" | "remove_last";
type Degrees = "r90" | "r180" | "r270";

const DEGREE_LABEL: Record<Degrees, string> = {
  r90: "90° clockwise",
  r180: "180°",
  r270: "270° clockwise",
};

export function BulkArchiveEditDialog({
  open,
  onOpenChange,
  issueIds,
  onResult,
}: {
  open: boolean;
  onOpenChange: (next: boolean) => void;
  issueIds: string[];
  /**
   * Fired after a successful enqueue (in place of `onOpenChange(false)`).
   * The caller closes the dialog and reconciles the selection — typically
   * keeping the skipped issues selected for retry. Omit for standalone use.
   */
  onResult?: (res: BulkEditResponse) => void;
}) {
  const [kind, setKind] = React.useState<OpKind>("rotate_cover");
  const [degrees, setDegrees] = React.useState<Degrees>("r180");
  const [count, setCount] = React.useState(1);
  const mut = useBulkArchiveEditMutation();

  const isRemove = kind === "remove_first" || kind === "remove_last";

  const buildOp = (): BulkArchiveOp => {
    switch (kind) {
      case "rotate_cover":
        return { kind, degrees };
      case "rotate_all":
        return { kind, degrees };
      case "remove_first":
        return { kind, count };
      case "remove_last":
        return { kind, count };
    }
  };

  const submit = () => {
    if (issueIds.length === 0) return;
    mut.mutate(
      { issue_ids: issueIds, op: buildOp() },
      {
        onSuccess: (res) => {
          if (!res) {
            onOpenChange(false);
            return;
          }
          const skippedCount = res.skipped.length;
          const reasons = summarizeBulkArchiveSkips(res.skipped);
          // The caller keeps the skipped issues selected for retry; only
          // hint at that when it actually does (onResult wired).
          const retryHint = onResult
            ? " They stay selected so you can fix the cause and retry."
            : "";

          if (res.queued > 0) {
            const title =
              skippedCount > 0
                ? `Archive edit: ${res.queued} queued, ${skippedCount} skipped`
                : `Archive edit: ${res.queued} issue${res.queued === 1 ? "" : "s"} queued`;
            toast.success(
              title,
              skippedCount > 0
                ? { description: `${reasons}.${retryHint}` }
                : undefined,
            );
          } else {
            // Nothing ran — usually writeback disabled / unsupported format.
            toast.info("No archives edited", {
              description: `${reasons}.${retryHint}`,
            });
          }

          if (onResult) {
            onResult(res);
          } else {
            onOpenChange(false);
          }
        },
      },
    );
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Edit archives</DialogTitle>
          <DialogDescription>
            Apply one operation to all {issueIds.length} selected issue
            {issueIds.length === 1 ? "" : "s"}. Each archive file is rewritten
            in place and a <code>.bak</code> backup is kept.
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-2">
          <RadioGroup
            value={kind}
            onValueChange={(v) => setKind(v as OpKind)}
            className="gap-2"
          >
            <OpRow value="rotate_cover" label="Rotate cover" current={kind} />
            <OpRow
              value="rotate_all"
              label="Rotate every page"
              current={kind}
            />
            <OpRow
              value="remove_first"
              label="Remove first pages"
              current={kind}
            />
            <OpRow
              value="remove_last"
              label="Remove last pages"
              current={kind}
            />
          </RadioGroup>

          {isRemove ? (
            <div className="flex items-center gap-2">
              <Label htmlFor="bulk-count" className="text-sm">
                How many pages
              </Label>
              <Input
                id="bulk-count"
                type="number"
                min={1}
                max={50}
                value={count}
                onChange={(e) =>
                  setCount(
                    Math.max(1, Math.min(50, Number(e.target.value) || 1)),
                  )
                }
                className="w-20"
              />
              <span className="text-muted-foreground text-xs">
                (always leaves at least one page)
              </span>
            </div>
          ) : (
            <div className="flex items-center gap-2">
              <Label htmlFor="bulk-degrees" className="text-sm">
                Rotation
              </Label>
              <Select
                value={degrees}
                onValueChange={(v) => setDegrees(v as Degrees)}
              >
                <SelectTrigger id="bulk-degrees" className="w-44">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="r90">{DEGREE_LABEL.r90}</SelectItem>
                  <SelectItem value="r180">{DEGREE_LABEL.r180}</SelectItem>
                  <SelectItem value="r270">{DEGREE_LABEL.r270}</SelectItem>
                </SelectContent>
              </Select>
            </div>
          )}
        </div>

        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            onClick={submit}
            disabled={mut.isPending || issueIds.length === 0}
          >
            {mut.isPending
              ? "Queuing…"
              : `Apply to ${issueIds.length} issue${issueIds.length === 1 ? "" : "s"}`}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function OpRow({
  value,
  label,
  current,
}: {
  value: OpKind;
  label: string;
  current: OpKind;
}) {
  return (
    <label
      className={`flex cursor-pointer items-center gap-2 rounded-md border px-3 py-2 text-sm transition-colors ${
        current === value ? "border-ring bg-accent/40" : "border-border"
      }`}
    >
      <RadioGroupItem value={value} />
      {label}
    </label>
  );
}
