"use client";

/**
 * Linked reading lists (PR 3b): "Find events and parts…". Asks the
 * server what the catalog says about this list — for a CBRO master, the
 * events its with-events version inserts and where; for any numbered
 * part, the previous / next part you already have — and lets the person
 * pick which to link. Event lists they don't have yet are imported on
 * apply (one import, however many lists).
 */
import * as React from "react";
import { Loader2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  useApplyCblDerive,
  useDeriveCblLinksPreview,
} from "@/lib/api/mutations/reading-list-links";
import type {
  ApplyDeriveReq,
  ApplyDeriveResp,
  DerivePreview,
  ProposedEventLink,
  ProposedPartLink,
} from "@/lib/api/types";

const eventKey = (e: ProposedEventLink) =>
  `${e.catalog_path}@${e.after_entry_id}`;
const partKey = (p: ProposedPartLink) => `${p.direction}:${p.list_id}`;

/** The apply body for the ticked proposals, in reading order. Pure. */
export function applyBody(
  preview: DerivePreview,
  picked: ReadonlySet<string>,
): Required<ApplyDeriveReq> {
  return {
    events: preview.events
      .filter((e) => !e.already_linked && picked.has(eventKey(e)))
      .sort(
        (a, b) =>
          a.after_position - b.after_position ||
          a.order_at_anchor - b.order_at_anchor,
      )
      .map((e) => ({
        catalog_path: e.catalog_path,
        after_entry_id: e.after_entry_id,
      })),
    parts: preview.parts
      .filter((p) => !p.already_linked && picked.has(partKey(p)))
      .map((p) => ({ direction: p.direction, list_id: p.list_id })),
  };
}

/** Everything not yet linked starts ticked. */
function initialPicks(preview: DerivePreview): Set<string> {
  return new Set([
    ...preview.events.filter((e) => !e.already_linked).map(eventKey),
    ...preview.parts.filter((p) => !p.already_linked).map(partKey),
  ]);
}

/** What an apply did when part of it failed. */
function ApplyResult({ result }: { result: ApplyDeriveResp }) {
  return (
    <div className="space-y-3 py-2" data-testid="derive-result">
      <p className="text-sm">
        {result.linked === 0
          ? "Nothing new was linked."
          : `Linked ${result.linked} ${result.linked === 1 ? "list" : "lists"}`}
        {result.imported > 0
          ? ` (${result.imported} imported).`
          : result.linked > 0
            ? "."
            : ""}
      </p>
      <section className="space-y-2">
        <h3 className="text-sm font-medium">
          {result.failed.length === 1
            ? "1 list wasn\u2019t linked"
            : `${result.failed.length} lists weren\u2019t linked`}
        </h3>
        <ul className="space-y-1.5 text-sm">
          {result.failed.map((f, i) => (
            <li key={i}>
              <span className="line-clamp-2 font-medium" title={f.name}>
                {f.name}
              </span>
              <span className="text-muted-foreground block text-xs">
                {f.message}
              </span>
            </li>
          ))}
        </ul>
        <p className="text-muted-foreground text-xs">
          Open Find events and parts again to retry them; lists already imported
          are reused.
        </p>
      </section>
    </div>
  );
}

export function DeriveLinksDialog({
  listId,
  open,
  onOpenChange,
}: {
  listId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const previewM = useDeriveCblLinksPreview(listId);
  const applyM = useApplyCblDerive(listId);
  const [preview, setPreview] = React.useState<DerivePreview | null>(null);
  const [picked, setPicked] = React.useState<Set<string>>(new Set());
  const [failed, setFailed] = React.useState(false);
  // What the last apply did, when some of it failed: the dialog stays
  // open on this instead of closing with one toast for the lot.
  const [result, setResult] = React.useState<ApplyDeriveResp | null>(null);
  const { mutate: runPreview } = previewM;

  const fetchPreview = React.useCallback(() => {
    runPreview(undefined, {
      onSuccess: (p) => {
        if (!p) return;
        setPreview(p);
        setPicked(initialPicks(p));
      },
      onError: () => setFailed(true),
    });
  }, [runPreview]);
  React.useEffect(() => {
    if (open) fetchPreview();
  }, [open, fetchPreview]);
  const retry = () => {
    setFailed(false);
    fetchPreview();
  };

  const toggle = (key: string, on: boolean) =>
    setPicked((prev) => {
      const next = new Set(prev);
      if (on) next.add(key);
      else next.delete(key);
      return next;
    });

  const body = preview ? applyBody(preview, picked) : null;
  const count = body ? body.events.length + body.parts.length : 0;
  const imports = preview
    ? preview.events.filter(
        (e) =>
          !e.already_linked &&
          e.existing_list_id == null &&
          picked.has(eventKey(e)),
      ).length
    : 0;
  // The event rows that can be ticked, for "Select all / none".
  const selectable = preview
    ? preview.events.filter((e) => !e.already_linked).map(eventKey)
    : [];
  const allPicked = selectable.every((k) => picked.has(k));
  const nothing =
    preview != null &&
    preview.events.length === 0 &&
    preview.parts.length === 0 &&
    preview.unplaced.length === 0;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[85dvh] flex-col sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Find events and parts</DialogTitle>
          <DialogDescription>
            From the catalog this list came from. Each event stays its own list
            with its own progress.
          </DialogDescription>
        </DialogHeader>

        <div className="-mx-6 min-h-0 flex-1 overflow-y-auto px-6">
          {preview == null ? (
            failed ? (
              <div className="space-y-3 py-6">
                <p className="text-muted-foreground text-sm">
                  Couldn&rsquo;t read the catalog.
                </p>
                <Button variant="outline" size="sm" onClick={retry}>
                  Try again
                </Button>
              </div>
            ) : (
              <div className="space-y-3 py-6" aria-busy="true">
                <p className="text-muted-foreground flex items-center gap-2 text-sm">
                  <Loader2 className="size-4 animate-spin" /> Reading the
                  catalog… this can take up to half a minute.
                </p>
                {[0, 1, 2].map((i) => (
                  <div
                    key={i}
                    className="bg-muted h-9 animate-pulse rounded"
                    aria-hidden
                  />
                ))}
              </div>
            )
          ) : result ? (
            <ApplyResult result={result} />
          ) : nothing && preview.twin_mismatch ? (
            <p className="text-muted-foreground py-6 text-sm">
              The catalog&rsquo;s with-events version doesn&rsquo;t line up with
              this list, so events can&rsquo;t be placed. Refresh the list from
              the catalog and try again.
            </p>
          ) : nothing ? (
            <p className="text-muted-foreground py-6 text-sm">
              {preview.twin_found
                ? "The catalog has no events or parts to link here."
                : "The catalog has no events for this list, and you don't have its other parts yet."}
            </p>
          ) : (
            <div className="space-y-5 py-1">
              {preview.events.length > 0 && (
                <section className="space-y-2">
                  <div className="flex items-center justify-between gap-2">
                    <h3 className="text-sm font-medium">Events</h3>
                    {selectable.length > 1 && (
                      <Button
                        variant="ghost"
                        size="sm"
                        disabled={applyM.isPending}
                        onClick={() =>
                          setPicked((prev) => {
                            const next = new Set(prev);
                            for (const k of selectable) {
                              if (allPicked) next.delete(k);
                              else next.add(k);
                            }
                            return next;
                          })
                        }
                      >
                        {allPicked ? "Select none" : "Select all"}
                      </Button>
                    )}
                  </div>
                  <ul className="space-y-1">
                    {preview.events.map((e) => {
                      const key = eventKey(e);
                      const id = `derive-${key}`;
                      return (
                        <li key={key} className="flex items-start gap-3 py-1">
                          <Checkbox
                            id={id}
                            className="mt-0.5"
                            checked={e.already_linked || picked.has(key)}
                            disabled={e.already_linked || applyM.isPending}
                            onCheckedChange={(v) => toggle(key, v === true)}
                          />
                          <label htmlFor={id} className="min-w-0 text-sm">
                            <span
                              className="line-clamp-2 font-medium"
                              title={e.name}
                            >
                              {e.name}
                            </span>
                            <span className="text-muted-foreground block text-xs">
                              After {e.after_label} · {e.total} issues
                              {e.already_linked
                                ? " · already linked"
                                : e.existing_list_id == null
                                  ? " · will be imported"
                                  : ""}
                            </span>
                          </label>
                        </li>
                      );
                    })}
                  </ul>
                </section>
              )}
              {preview.parts.length > 0 && (
                <section className="space-y-2">
                  <h3 className="text-sm font-medium">Parts</h3>
                  <ul className="space-y-1">
                    {preview.parts.map((p) => {
                      const key = partKey(p);
                      const id = `derive-${key}`;
                      return (
                        <li key={key} className="flex items-start gap-3 py-1">
                          <Checkbox
                            id={id}
                            className="mt-0.5"
                            checked={p.already_linked || picked.has(key)}
                            disabled={p.already_linked || applyM.isPending}
                            onCheckedChange={(v) => toggle(key, v === true)}
                          />
                          <label htmlFor={id} className="min-w-0 text-sm">
                            <span
                              className="line-clamp-2 font-medium"
                              title={p.name}
                            >
                              {p.name}
                            </span>
                            <span className="text-muted-foreground block text-xs">
                              {p.direction === "next"
                                ? "Read it after this list"
                                : "This list continues it"}
                              {p.already_linked ? " · already linked" : ""}
                            </span>
                          </label>
                        </li>
                      );
                    })}
                  </ul>
                </section>
              )}
              {preview.unplaced.length > 0 && (
                <section className="space-y-2">
                  <h3 className="text-sm font-medium">
                    Not matched to an event
                  </h3>
                  <p className="text-muted-foreground text-xs">
                    The with-events version adds these issues, but no event list
                    holds them. Add them yourself if you want them.
                  </p>
                  <ul className="text-muted-foreground space-y-1 text-xs">
                    {preview.unplaced.map((u, i) => (
                      <li key={i}>
                        {u.first_label}
                        {u.issues > 1 ? ` – ${u.last_label}` : ""} ({u.issues}{" "}
                        {u.issues === 1 ? "issue" : "issues"})
                        {u.after_label
                          ? `, after ${u.after_label}`
                          : ", at the start"}
                      </li>
                    ))}
                  </ul>
                </section>
              )}
            </div>
          )}
        </div>

        <DialogFooter className="items-center">
          {imports > 0 && (
            <p className="text-muted-foreground mr-auto text-xs">
              Imports {imports} {imports === 1 ? "list" : "lists"} into your
              reading lists.
            </p>
          )}
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            {result ? "Done" : count === 0 ? "Close" : "Cancel"}
          </Button>
          {count > 0 && body && !result && (
            <Button
              disabled={applyM.isPending}
              onClick={() =>
                applyM.mutate(body, {
                  onSuccess: (resp) => {
                    // Anything that failed keeps the dialog open on what
                    // happened; a clean run closes it.
                    if (resp && resp.failed.length > 0) setResult(resp);
                    else onOpenChange(false);
                  },
                })
              }
            >
              {applyM.isPending && <Loader2 className="size-4 animate-spin" />}
              Link {count}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
