"use client";

/**
 * Reading-list updates — the bulk check / review / update surface for the
 * reading-lists board, kept out of the way until asked for.
 *
 * Three pieces share one provider (`CblUpdatesProvider`):
 *
 *   - `CblUpdatesMenu` sits in the board toolbar as a small "Updates"
 *     button. Its menu runs **Check for updates**, **Update all lists…**,
 *     or shows / hides the panel. The button carries a count badge once a
 *     check has found lists with changes and spins while anything runs,
 *     so the state is visible without the panel taking space.
 *   - `CblUpdatesPanel` renders between the toolbar and the board only
 *     while open. Every action that produces something to look at (a
 *     check, any batch) opens it; the user closes it from its ✕ or the
 *     menu. It holds the status row, the review list and the batch
 *     progress.
 *   - The provider owns the check results and selection, and wraps the
 *     shared client-side batch runner (`useBatchRefresh`) so the group
 *     menu's "Update these lists" surfaces in the same panel.
 *
 * Workflow inside the panel:
 *   1. **Check for updates** dry-runs every list (nothing is applied) and
 *      returns which ones have upstream changes with their diff counts.
 *   2. The review list shows each changed list with its diff as tone
 *      badges (+added / −removed / reordered / newly matched) and a
 *      checkbox. Update them one at a time, pick a subset and **Update
 *      selected**, or **Update all**.
 *   3. Updates run sequentially through the batch runner, which shows a
 *      real progress bar, the list currently being fetched, per-row
 *      outcomes, and a Cancel.
 */

import * as React from "react";
import Link from "next/link";
import {
  AlertTriangle,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  CloudDownload,
  Loader2,
  PanelTopClose,
  PanelTopOpen,
  RefreshCw,
  X,
} from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/folio/dropdown-menu";
import { Progress } from "@/components/ui/progress";
import { apiMutate } from "@/lib/api/mutations/_core";
import { useCheckAllCblLists } from "@/lib/api/mutations";
import type { CblBulkItemView, ImportSummary } from "@/lib/api/types";
import {
  summarizeBatch,
  summaryChanged,
  useBatchRefresh,
  type BatchRow,
  type BatchTarget,
} from "@/lib/cbl/use-batch-refresh";
import { formatRelativeDate } from "@/lib/format";
import { statusTone, statusToneText } from "@/lib/ui/status-tone";
import { cn } from "@/lib/utils";

export type DashboardList = { id: string; name: string; viewId: string };

type CblUpdates = {
  lists: DashboardList[];
  /** `null` until the user runs a check; then the per-list outcomes. */
  results: CblBulkItemView[] | null;
  checking: boolean;
  /** A check or a batch is in flight. */
  busy: boolean;
  /** Checked lists with changes that haven't been updated since. */
  pending: CblBulkItemView[];
  upToDate: CblBulkItemView[];
  failed: CblBulkItemView[];
  selected: ReadonlySet<string>;
  retrying: ReadonlySet<string>;
  /** One-line status for the panel header and the menu. */
  statusLine: string;
  viewHref: (id: string) => string | undefined;
  check: () => void;
  updateAll: () => void;
  updateSelected: () => void;
  updateOne: (item: CblBulkItemView) => void;
  retryCheck: (item: CblBulkItemView) => Promise<void>;
  setSelected: React.Dispatch<React.SetStateAction<ReadonlySet<string>>>;
  /** Run a batch over arbitrary lists (the group menu) — opens the panel. */
  startBatch: (targets: BatchTarget[]) => void;
  panelOpen: boolean;
  setPanelOpen: (open: boolean) => void;
};

const CblUpdatesContext = React.createContext<CblUpdates | null>(null);

export function useCblUpdates(): CblUpdates {
  const ctx = React.useContext(CblUpdatesContext);
  if (!ctx) {
    throw new Error("useCblUpdates must be used inside <CblUpdatesProvider>");
  }
  return ctx;
}

export function CblUpdatesProvider({
  lists,
  children,
}: {
  lists: DashboardList[];
  children: React.ReactNode;
}) {
  const checkAll = useCheckAllCblLists();
  const batch = useBatchRefresh();
  const [results, setResults] = React.useState<CblBulkItemView[] | null>(null);
  const [checkedAt, setCheckedAt] = React.useState<string | null>(null);
  const [selected, setSelected] = React.useState<ReadonlySet<string>>(
    new Set(),
  );
  // Lists updated since the last check (by any path) drop out of the
  // pending review set.
  const [appliedIds, setAppliedIds] = React.useState<ReadonlySet<string>>(
    new Set(),
  );
  const [retrying, setRetrying] = React.useState<ReadonlySet<string>>(
    new Set(),
  );
  const [panelOpen, setPanelOpen] = React.useState(false);

  const count = lists.length;
  const viewHref = React.useMemo(() => {
    const m = new Map(lists.map((l) => [l.id, `/views/${l.viewId}`]));
    return (id: string) => m.get(id);
  }, [lists]);

  // When a batch finishes (from here or from a group menu), fold its
  // successes into `appliedIds` so the review list reflects what's left.
  const { subscribe } = batch;
  React.useEffect(
    () =>
      subscribe((rows) => {
        const done = rows.filter((r) => r.status === "done").map((r) => r.id);
        if (done.length === 0) return;
        setAppliedIds((s) => new Set([...s, ...done]));
        setSelected((s) => {
          const next = new Set(s);
          for (const id of done) next.delete(id);
          return next;
        });
      }),
    [subscribe],
  );

  const checking = checkAll.isPending;
  const busy = checking || batch.state.running;
  const pending = React.useMemo(
    () =>
      (results ?? []).filter((r) => r.ok && r.changed && !appliedIds.has(r.id)),
    [results, appliedIds],
  );
  const upToDate = React.useMemo(
    () =>
      (results ?? []).filter(
        (r) => r.ok && (!r.changed || appliedIds.has(r.id)),
      ),
    [results, appliedIds],
  );
  const failed = React.useMemo(
    () => (results ?? []).filter((r) => !r.ok),
    [results],
  );

  const { mutate: runCheck } = checkAll;
  const check = React.useCallback(() => {
    setPanelOpen(true);
    setAppliedIds(new Set());
    setSelected(new Set());
    runCheck(undefined, {
      onSuccess: (d) => {
        const items = d?.items ?? [];
        setResults(items);
        setCheckedAt(new Date().toISOString());
        // Pre-select everything with changes: the common case is "yes,
        // update them", and unticking is one click per exception.
        setSelected(
          new Set(items.filter((i) => i.ok && i.changed).map((i) => i.id)),
        );
      },
    });
  }, [runCheck]);

  const { clear: clearBatch, start: startRaw } = batch;
  const startBatch = React.useCallback(
    (targets: BatchTarget[]) => {
      if (targets.length === 0) return;
      setPanelOpen(true);
      clearBatch();
      startRaw(targets);
    },
    [clearBatch, startRaw],
  );
  const updateAll = React.useCallback(
    () => startBatch(lists.map((l) => ({ id: l.id, name: l.name }))),
    [lists, startBatch],
  );
  const updateSelected = React.useCallback(
    () =>
      startBatch(
        pending
          .filter((p) => selected.has(p.id))
          .map((p) => ({ id: p.id, name: p.name })),
      ),
    [pending, selected, startBatch],
  );
  const updateOne = React.useCallback(
    (item: CblBulkItemView) => startBatch([{ id: item.id, name: item.name }]),
    [startBatch],
  );

  const retryCheck = React.useCallback(async (item: CblBulkItemView) => {
    setRetrying((s) => new Set(s).add(item.id));
    try {
      const fresh = await apiMutate<CblBulkItemView>({
        path: `/me/cbl-lists/${item.id}/check`,
        method: "POST",
      });
      if (fresh) {
        setResults((rs) =>
          (rs ?? []).map((r) => (r.id === item.id ? fresh : r)),
        );
        if (fresh.ok && fresh.changed)
          setSelected((s) => new Set(s).add(item.id));
      }
    } catch (e) {
      const message = e instanceof Error ? e.message : "check failed";
      setResults((rs) =>
        (rs ?? []).map((r) =>
          r.id === item.id ? { ...r, ok: false, error: message } : r,
        ),
      );
    } finally {
      setRetrying((s) => {
        const next = new Set(s);
        next.delete(item.id);
        return next;
      });
    }
  }, []);

  const statusLine = (() => {
    if (checking)
      return `Checking ${count} ${count === 1 ? "list" : "lists"} for upstream changes…`;
    if (results === null)
      return `${count} ${count === 1 ? "list" : "lists"} · not checked yet this session`;
    const when = checkedAt ? formatRelativeDate(checkedAt) : null;
    const base = when ? `Checked ${when}` : "Checked";
    if (pending.length === 0 && failed.length === 0)
      return `${base} · everything is up to date`;
    const parts: string[] = [];
    if (pending.length) parts.push(`${pending.length} with updates`);
    if (failed.length) parts.push(`${failed.length} couldn't be checked`);
    return `${base} · ${parts.join(" · ")}`;
  })();

  const value = React.useMemo<CblUpdates>(
    () => ({
      lists,
      results,
      checking,
      busy,
      pending,
      upToDate,
      failed,
      selected,
      retrying,
      statusLine,
      viewHref,
      check,
      updateAll,
      updateSelected,
      updateOne,
      retryCheck,
      setSelected,
      startBatch,
      panelOpen,
      setPanelOpen,
    }),
    [
      lists,
      results,
      checking,
      busy,
      pending,
      upToDate,
      failed,
      selected,
      retrying,
      statusLine,
      viewHref,
      check,
      updateAll,
      updateSelected,
      updateOne,
      retryCheck,
      startBatch,
      panelOpen,
    ],
  );

  return (
    <CblUpdatesContext.Provider value={value}>
      {children}
    </CblUpdatesContext.Provider>
  );
}

/** Toolbar entry point: a quiet "Updates" button whose menu runs a check,
 *  updates everything, or toggles the panel. Shows a count badge once a
 *  check has found lists with changes and spins while work is running. */
export function CblUpdatesMenu() {
  const u = useCblUpdates();
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  if (u.lists.length === 0) return null;
  const count = u.lists.length;
  const n = u.pending.length;

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="outline"
            className="h-9"
            aria-label={
              n > 0
                ? `Updates — ${n} ${n === 1 ? "list has" : "lists have"} changes`
                : "Updates"
            }
            title={u.statusLine}
          >
            {u.busy ? (
              <Loader2 className="text-primary h-4 w-4 animate-spin" />
            ) : (
              <RefreshCw className="h-4 w-4" />
            )}
            <span className="hidden sm:inline">Updates</span>
            {n > 0 ? (
              <Badge
                variant="secondary"
                className="h-5 min-w-5 justify-center px-1.5 tabular-nums"
              >
                {n}
              </Badge>
            ) : null}
            <ChevronDown
              className="h-3.5 w-3.5 opacity-60"
              aria-hidden="true"
            />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-64">
          <DropdownMenuLabel className="text-muted-foreground text-xs font-normal">
            {u.statusLine}
          </DropdownMenuLabel>
          <DropdownMenuSeparator />
          <DropdownMenuItem disabled={u.busy} onSelect={u.check}>
            <RefreshCw className="h-4 w-4" /> Check for updates
          </DropdownMenuItem>
          <DropdownMenuItem
            disabled={u.busy}
            onSelect={() => setConfirmOpen(true)}
          >
            <CloudDownload className="h-4 w-4" /> Update all lists…
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onSelect={() => u.setPanelOpen(!u.panelOpen)}>
            {u.panelOpen ? (
              <>
                <PanelTopClose className="h-4 w-4" /> Hide updates panel
              </>
            ) : (
              <>
                <PanelTopOpen className="h-4 w-4" /> Show updates panel
              </>
            )}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <UpdateAllDialog
        open={confirmOpen}
        onOpenChange={setConfirmOpen}
        count={count}
        onConfirm={u.updateAll}
      />
    </>
  );
}

function UpdateAllDialog({
  open,
  onOpenChange,
  count,
  onConfirm,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  count: number;
  onConfirm: () => void;
}) {
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Update all reading lists?</AlertDialogTitle>
          <AlertDialogDescription>
            Re-fetches every reading list from its source and applies any
            changes — new, removed, or reordered issues — across {count}{" "}
            {count === 1 ? "list" : "lists"}, one at a time with progress shown
            in the updates panel. Your manual matches are preserved. Prefer to
            look first? Use{" "}
            <span className="font-medium">Check for updates</span> and pick
            which lists to update.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction onClick={onConfirm}>Update all</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

/** The panel proper. Mounted only while `panelOpen` — the provider keeps
 *  the results, so hiding and re-showing it loses nothing. */
export function CblUpdatesPanel() {
  const u = useCblUpdates();
  const batch = useBatchRefresh();
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  if (!u.panelOpen || u.lists.length === 0) return null;

  const {
    busy,
    checking,
    results,
    pending,
    upToDate,
    failed,
    selected,
    retrying,
    viewHref,
  } = u;
  const count = u.lists.length;
  const selectedPending = pending.filter((p) => selected.has(p.id));
  const allPendingSelected =
    pending.length > 0 && selectedPending.length === pending.length;

  return (
    <section
      aria-label="Reading list updates"
      className="border-border bg-card rounded-xl border"
    >
      {/* Status row */}
      <div className="flex flex-wrap items-center justify-between gap-3 p-3 sm:p-4">
        <div className="flex min-w-0 items-center gap-3">
          <span
            className={cn(
              "bg-muted text-muted-foreground inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-full",
              busy && "text-primary",
            )}
            aria-hidden="true"
          >
            <RefreshCw className={cn("h-4 w-4", busy && "animate-spin")} />
          </span>
          <div className="min-w-0">
            <h3 className="text-foreground text-sm font-semibold">
              Reading list updates
            </h3>
            <p
              className="text-muted-foreground truncate text-xs"
              aria-live="polite"
            >
              {u.statusLine}
            </p>
          </div>
        </div>
        <div className="flex shrink-0 flex-wrap items-center gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={busy}
            onClick={u.check}
          >
            {checking ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <RefreshCw className="h-4 w-4" />
            )}
            Check for updates
          </Button>
          <Button
            type="button"
            size="sm"
            disabled={busy}
            onClick={() => setConfirmOpen(true)}
          >
            Update all
          </Button>
          <Button
            type="button"
            size="icon"
            variant="ghost"
            className="h-8 w-8"
            aria-label="Hide updates panel"
            onClick={() => u.setPanelOpen(false)}
          >
            <X className="h-4 w-4" />
          </Button>
        </div>
      </div>
      <UpdateAllDialog
        open={confirmOpen}
        onOpenChange={setConfirmOpen}
        count={count}
        onConfirm={u.updateAll}
      />

      {/* Batch progress / outcome */}
      {batch.state.rows.length > 0 ? <BatchPanel viewHref={viewHref} /> : null}

      {/* Review list (after a check) */}
      {results !== null && !checking ? (
        <div className="border-border/60 border-t p-3 sm:p-4">
          {pending.length === 0 && failed.length === 0 ? (
            <p className="text-muted-foreground flex items-center gap-2 text-sm">
              <CheckCircle2
                className={cn("h-4 w-4", statusToneText("success"))}
              />
              Everything is up to date.
              {upToDate.length > 0 ? (
                <span className="text-xs">
                  ({upToDate.length} {upToDate.length === 1 ? "list" : "lists"}{" "}
                  checked)
                </span>
              ) : null}
            </p>
          ) : (
            <div className="space-y-3">
              {pending.length > 0 ? (
                <>
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <label className="flex items-center gap-2 text-sm">
                      <Checkbox
                        checked={
                          allPendingSelected
                            ? true
                            : selectedPending.length > 0
                              ? "indeterminate"
                              : false
                        }
                        onCheckedChange={(v) =>
                          u.setSelected(
                            v === true
                              ? new Set(pending.map((p) => p.id))
                              : new Set(),
                          )
                        }
                        aria-label="Select all lists with updates"
                        disabled={busy}
                      />
                      <span className="font-medium">
                        {pending.length}{" "}
                        {pending.length === 1 ? "list has" : "lists have"}{" "}
                        updates
                      </span>
                    </label>
                    <Button
                      type="button"
                      size="sm"
                      disabled={busy || selectedPending.length === 0}
                      onClick={u.updateSelected}
                    >
                      Update selected
                      {selectedPending.length > 0
                        ? ` (${selectedPending.length})`
                        : ""}
                    </Button>
                  </div>
                  <ul className="divide-border/60 border-border/60 divide-y rounded-lg border">
                    {pending.map((item) => (
                      <ReviewRow
                        key={item.id}
                        item={item}
                        href={viewHref(item.id)}
                        checked={selected.has(item.id)}
                        disabled={busy}
                        onCheckedChange={(v) =>
                          u.setSelected((s) => {
                            const next = new Set(s);
                            if (v) next.add(item.id);
                            else next.delete(item.id);
                            return next;
                          })
                        }
                        onUpdate={() => u.updateOne(item)}
                      />
                    ))}
                  </ul>
                </>
              ) : null}

              {failed.length > 0 ? (
                <ul className="space-y-1.5">
                  {failed.map((item) => (
                    <li
                      key={item.id}
                      className="text-muted-foreground flex flex-wrap items-center gap-2 text-sm"
                    >
                      <AlertTriangle
                        className={cn(
                          "h-4 w-4 shrink-0",
                          statusToneText("warning"),
                        )}
                        aria-hidden="true"
                      />
                      <span className="text-foreground truncate font-medium">
                        {item.name}
                      </span>
                      <span
                        className="min-w-0 flex-1 truncate text-xs"
                        title={item.error ?? undefined}
                      >
                        couldn&rsquo;t be checked
                        {item.error ? ` — ${item.error}` : ""}
                      </span>
                      <Button
                        type="button"
                        size="sm"
                        variant="ghost"
                        className="h-7"
                        disabled={retrying.has(item.id)}
                        onClick={() => void u.retryCheck(item)}
                      >
                        {retrying.has(item.id) ? (
                          <Loader2 className="h-3.5 w-3.5 animate-spin" />
                        ) : null}
                        Retry
                      </Button>
                    </li>
                  ))}
                </ul>
              ) : null}

              {upToDate.length > 0 ? (
                <Collapsible>
                  <CollapsibleTrigger className="text-muted-foreground hover:text-foreground group/c inline-flex items-center gap-1 text-xs">
                    <ChevronRight className="h-3.5 w-3.5 transition-transform group-data-[state=open]/c:rotate-90" />
                    {upToDate.length} up to date
                  </CollapsibleTrigger>
                  <CollapsibleContent>
                    <ul className="text-muted-foreground mt-1.5 flex flex-wrap gap-1.5 text-xs">
                      {upToDate.map((item) => (
                        <li key={item.id}>
                          <Badge
                            variant="outline"
                            className="gap-1 font-normal"
                          >
                            <Check className="h-3 w-3" aria-hidden="true" />
                            {item.name}
                          </Badge>
                        </li>
                      ))}
                    </ul>
                  </CollapsibleContent>
                </Collapsible>
              ) : null}
            </div>
          )}
        </div>
      ) : null}
    </section>
  );
}

/** One changed list in the review panel: checkbox + name + diff badges +
 *  a per-row Update. */
function ReviewRow({
  item,
  href,
  checked,
  disabled,
  onCheckedChange,
  onUpdate,
}: {
  item: CblBulkItemView;
  href: string | undefined;
  checked: boolean;
  disabled: boolean;
  onCheckedChange: (checked: boolean) => void;
  onUpdate: () => void;
}) {
  const id = `review-${item.id}`;
  return (
    <li className="flex flex-wrap items-center gap-2 px-3 py-2 text-sm">
      <Checkbox
        id={id}
        checked={checked}
        disabled={disabled}
        onCheckedChange={(v) => onCheckedChange(v === true)}
        aria-label={`Select ${item.name}`}
      />
      {href ? (
        <Link
          href={href}
          className="text-foreground min-w-0 truncate font-medium hover:underline"
        >
          {item.name}
        </Link>
      ) : (
        <label
          htmlFor={id}
          className="text-foreground min-w-0 truncate font-medium"
        >
          {item.name}
        </label>
      )}
      <DiffBadges counts={item} />
      <Button
        type="button"
        size="sm"
        variant="outline"
        className="ml-auto h-7"
        disabled={disabled}
        onClick={onUpdate}
      >
        Update
      </Button>
    </li>
  );
}

type DiffCounts = Pick<
  ImportSummary,
  "added" | "removed" | "reordered" | "rematched"
>;

/** Tone-coded diff chips. Success for growth, error for removals, info
 *  for reorders, success for newly matched entries. */
function DiffBadges({ counts }: { counts: DiffCounts }) {
  const chips: Array<{
    key: string;
    tone: "success" | "error" | "info";
    label: string;
  }> = [];
  if (counts.added)
    chips.push({
      key: "added",
      tone: "success",
      label: `+${counts.added} added`,
    });
  if (counts.removed)
    chips.push({
      key: "removed",
      tone: "error",
      label: `−${counts.removed} removed`,
    });
  if (counts.reordered)
    chips.push({
      key: "reordered",
      tone: "info",
      label: `${counts.reordered} reordered`,
    });
  if (counts.rematched)
    chips.push({
      key: "rematched",
      tone: "success",
      label: `${counts.rematched} newly matched`,
    });
  if (chips.length === 0) {
    return (
      <span className="text-muted-foreground text-xs">changes available</span>
    );
  }
  return (
    <span className="flex flex-wrap items-center gap-1">
      {chips.map((c) => (
        <Badge
          key={c.key}
          variant="outline"
          className={cn("font-normal tabular-nums", statusTone(c.tone))}
        >
          {c.label}
        </Badge>
      ))}
    </span>
  );
}

/** Progress for the running / just-finished batch, with per-row outcomes. */
function BatchPanel({
  viewHref,
}: {
  viewHref: (id: string) => string | undefined;
}) {
  const batch = useBatchRefresh();
  const { rows, running } = batch.state;
  const total = rows.length;
  const done = batch.completed;
  const current = rows.find((r) => r.status === "running");
  const percent = total > 0 ? Math.round((done / total) * 100) : 0;
  const summary = summarizeBatch(rows);
  const [showRows, setShowRows] = React.useState(false);
  // Expand the row list automatically while running (so the user sees
  // movement) and when anything failed.
  const open = running || summary.failed > 0 || showRows;

  return (
    <div className="border-border/60 space-y-2 border-t p-3 sm:p-4">
      <div className="flex flex-wrap items-center justify-between gap-2 text-sm">
        <div className="flex min-w-0 items-center gap-2">
          {running ? (
            <Loader2
              className="text-primary h-4 w-4 shrink-0 animate-spin"
              aria-hidden="true"
            />
          ) : summary.failed > 0 ? (
            <AlertTriangle
              className={cn("h-4 w-4 shrink-0", statusToneText("warning"))}
              aria-hidden="true"
            />
          ) : (
            <CheckCircle2
              className={cn("h-4 w-4 shrink-0", statusToneText("success"))}
              aria-hidden="true"
            />
          )}
          <span className="min-w-0 truncate" aria-live="polite">
            {running ? (
              <>
                Updating {Math.min(done + 1, total)} of {total}
                {current ? (
                  <span className="text-muted-foreground">
                    {" "}
                    · {current.name}
                  </span>
                ) : null}
              </>
            ) : (
              <>
                Refreshed {total} {total === 1 ? "list" : "lists"}
                <span className="text-muted-foreground">
                  {" "}
                  ·{" "}
                  {[
                    summary.updated ? `${summary.updated} changed` : null,
                    summary.unchanged
                      ? `${summary.unchanged} already current`
                      : null,
                    summary.failed ? `${summary.failed} failed` : null,
                    summary.skipped ? `${summary.skipped} skipped` : null,
                  ]
                    .filter(Boolean)
                    .join(" · ")}
                </span>
              </>
            )}
          </span>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {running ? (
            <Button
              type="button"
              size="sm"
              variant="ghost"
              className="h-7"
              onClick={batch.cancel}
            >
              <X className="h-3.5 w-3.5" /> Stop after this one
            </Button>
          ) : (
            <>
              <Button
                type="button"
                size="sm"
                variant="ghost"
                className="h-7"
                onClick={() => setShowRows((v) => !v)}
              >
                {open ? "Hide details" : "Show details"}
              </Button>
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className="h-7 w-7"
                aria-label="Dismiss"
                onClick={batch.clear}
              >
                <X className="h-4 w-4" />
              </Button>
            </>
          )}
        </div>
      </div>
      <Progress
        value={percent}
        className="h-1.5"
        aria-label={`${percent}% complete`}
      />
      {open ? (
        <ul className="divide-border/60 border-border/60 divide-y rounded-lg border text-sm">
          {rows.map((r) => (
            <BatchRowItem key={r.id} row={r} href={viewHref(r.id)} />
          ))}
        </ul>
      ) : null}
    </div>
  );
}

function BatchRowItem({
  row,
  href,
}: {
  row: BatchRow;
  href: string | undefined;
}) {
  const icon = (() => {
    switch (row.status) {
      case "running":
        return (
          <Loader2
            className="text-primary h-4 w-4 animate-spin"
            aria-label="Updating"
          />
        );
      case "done":
        return summaryChanged(row.summary) ? (
          <CheckCircle2
            className={cn("h-4 w-4", statusToneText("success"))}
            aria-label="Updated"
          />
        ) : (
          <Check
            className="text-muted-foreground h-4 w-4"
            aria-label="Already up to date"
          />
        );
      case "failed":
        return (
          <AlertTriangle
            className={cn("h-4 w-4", statusToneText("error"))}
            aria-label="Failed"
          />
        );
      case "skipped":
        return (
          <X className="text-muted-foreground h-4 w-4" aria-label="Skipped" />
        );
      default:
        return (
          <span
            className="bg-border inline-block h-2 w-2 rounded-full"
            aria-label="Queued"
          />
        );
    }
  })();
  return (
    <li
      className={cn(
        "flex flex-wrap items-center gap-2 px-3 py-1.5",
        row.status === "queued" && "text-muted-foreground",
      )}
    >
      <span className="inline-flex w-4 shrink-0 items-center justify-center">
        {icon}
      </span>
      {href ? (
        <Link
          href={href}
          className="min-w-0 truncate font-medium hover:underline"
        >
          {row.name}
        </Link>
      ) : (
        <span className="min-w-0 truncate font-medium">{row.name}</span>
      )}
      {row.status === "done" && row.summary ? (
        summaryChanged(row.summary) ? (
          <DiffBadges counts={row.summary} />
        ) : (
          <span className="text-muted-foreground text-xs">no changes</span>
        )
      ) : null}
      {row.status === "failed" ? (
        <span
          className={cn(
            "min-w-0 flex-1 truncate text-xs",
            statusToneText("error"),
          )}
          title={row.error}
        >
          {row.error}
        </span>
      ) : null}
    </li>
  );
}
