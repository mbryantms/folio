"use client";

/**
 * `<ExternalIdsCard>` (metadata-providers-1.0 M5).
 *
 * Per-entity list of `external_ids` rows with add / unlink actions.
 * Used on both the series page and the issue page —
 * `entityType="series"|"issue"` picks the right endpoints + query keys.
 *
 * User-added rows land with `set_by='user'` and are sacred against
 * future Apply jobs (the M4 layer's `should_apply` matrix checks
 * provenance before overwriting).
 *
 * The data + mutations + the two pieces of UI every surface needs (the
 * inline add form and the unlink confirm) live in
 * [`useExternalIdsEditor`], so a host that lays the ids out its own way
 * — the series Metadata tab's provider-mapping card shows each
 * provider's id in that provider's header — shares one editor with
 * this card instead of a second add/unlink implementation.
 *
 * Adding and unlinking an id is admin-only (audit 2026-11 E13: the
 * server's add / unlink routes take `RequireAdmin`, like the issue and
 * series `PATCH`). Readers see the ids; the host passes `canEdit` and
 * the card drops the add and remove controls without it.
 */

import { Loader2, Plus, Trash2 } from "lucide-react";
import * as React from "react";

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
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/folio/select";
import { collectServerErrors } from "@/lib/api/form-errors";
import {
  useAddExternalIdIssue,
  useAddExternalIdSeries,
  useDeleteExternalIdIssue,
  useDeleteExternalIdSeries,
} from "@/lib/api/mutations";
import { useExternalIdsIssue, useExternalIdsSeries } from "@/lib/api/queries";
import type { ExternalIdRow } from "@/lib/api/types";

const SOURCES: Array<{ value: string; label: string }> = [
  { value: "comicvine", label: "ComicVine" },
  { value: "metron", label: "Metron" },
  { value: "gcd", label: "Grand Comics Database" },
  { value: "marvel", label: "Marvel" },
  { value: "locg", label: "League of Comic Geeks" },
  { value: "mal", label: "MyAnimeList" },
  { value: "anilist", label: "AniList" },
  { value: "mangaupdates", label: "MangaUpdates" },
  { value: "isbn", label: "ISBN" },
  { value: "upc", label: "UPC" },
  { value: "asin", label: "ASIN" },
];

export type ExternalIdsTarget =
  | { entityType: "series"; seriesSlug: string }
  | { entityType: "issue"; seriesSlug: string; issueSlug: string };

export type ExternalIdsCardProps = ExternalIdsTarget & {
  /**
   * `"card"` (default): wraps content in a [`<Card>`] with a "External IDs"
   * header — matches the series-page panel + legacy issue-page layout.
   *
   * `"bare"`: drops the `<Card>` chrome entirely (no header, no border).
   * Caller is responsible for the section title. Used by the issue page
   * tabs where the tab label IS the title and a nested card would
   * visually clash with the other tabs' content (MetadataGrid, ChipList,
   * etc. — none of which use Cards).
   */
  chrome?: "card" | "bare";
  /**
   * Show the add / unlink controls. Pass the viewer's admin role: the
   * write routes answer 403 to everyone else.
   */
  canEdit: boolean;
};

/**
 * The external-ids editor for one entity: rows, the inline "+ Add ID"
 * form and the unlink confirm dialog. The host decides where the rows
 * and the add affordance go; it renders `addForm` where the form should
 * appear while `adding`, and `confirmDialog` once anywhere in its tree.
 *
 * `idPrefix` keeps the form's input ids unique when two editors share a
 * page (the series card and an issue card never do today, but the
 * mapping card and a legacy `<ExternalIdsCard>` could).
 */
export function useExternalIdsEditor(
  target: ExternalIdsTarget,
  { idPrefix = "eic" }: { idPrefix?: string } = {},
) {
  const seriesList = useExternalIdsSeries(
    target.entityType === "series" ? target.seriesSlug : "",
  );
  const issueList = useExternalIdsIssue(
    target.entityType === "issue" ? target.seriesSlug : "",
    target.entityType === "issue" ? target.issueSlug : "",
  );
  const addSeries = useAddExternalIdSeries(
    target.entityType === "series" ? target.seriesSlug : "",
  );
  const deleteSeries = useDeleteExternalIdSeries(
    target.entityType === "series" ? target.seriesSlug : "",
  );
  const addIssue = useAddExternalIdIssue(
    target.entityType === "issue" ? target.seriesSlug : "",
    target.entityType === "issue" ? target.issueSlug : "",
  );
  const deleteIssue = useDeleteExternalIdIssue(
    target.entityType === "issue" ? target.seriesSlug : "",
    target.entityType === "issue" ? target.issueSlug : "",
  );

  const query = target.entityType === "series" ? seriesList : issueList;
  const add = target.entityType === "series" ? addSeries : addIssue;
  const remove = target.entityType === "series" ? deleteSeries : deleteIssue;

  const rows = React.useMemo(() => query.data?.rows ?? [], [query.data]);
  const [adding, setAdding] = React.useState(false);
  const [source, setSource] = React.useState("comicvine");
  const [extId, setExtId] = React.useState("");
  const [confirmRemove, setConfirmRemove] =
    React.useState<ExternalIdRow | null>(null);
  // Server 422 field errors (`error.details`) bound under their inputs;
  // the mutation hook still toasts the summary.
  const [errors, setErrors] = React.useState<Record<string, string>>({});

  const cancelAdding = () => {
    setAdding(false);
    setExtId("");
    setErrors({});
  };
  const onAdd = (e: React.FormEvent) => {
    e.preventDefault();
    if (!extId.trim()) return;
    setErrors({});
    add.mutate(
      { source, external_id: extId.trim() },
      {
        onSuccess: () => cancelAdding(),
        onError: (err) =>
          setErrors(collectServerErrors(err, ["source", "external_id"])),
      },
    );
  };
  const onConfirmRemove = () => {
    if (!confirmRemove) return;
    remove.mutate(
      { source: confirmRemove.source },
      { onSuccess: () => setConfirmRemove(null) },
    );
  };

  const addForm = adding ? (
    <form
      onSubmit={onAdd}
      className="flex flex-col gap-2 border-t pt-3 sm:flex-row sm:items-end"
      data-testid="external-ids-add-form"
    >
      <div className="grid flex-1 gap-1.5">
        <Label htmlFor={`${idPrefix}-source`} className="text-xs">
          Source
        </Label>
        <Select value={source} onValueChange={setSource}>
          <SelectTrigger
            id={`${idPrefix}-source`}
            aria-invalid={errors.source ? true : undefined}
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {SOURCES.map((s) => (
              <SelectItem key={s.value} value={s.value}>
                {s.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {errors.source ? (
          <p className="text-destructive text-xs">{errors.source}</p>
        ) : null}
      </div>
      <div className="grid flex-[2] gap-1.5">
        <Label htmlFor={`${idPrefix}-id`} className="text-xs">
          ID
        </Label>
        <Input
          id={`${idPrefix}-id`}
          value={extId}
          onChange={(e) => setExtId(e.target.value)}
          placeholder="e.g. 12345"
          autoFocus
          aria-invalid={errors.external_id ? true : undefined}
          aria-describedby={
            errors.external_id ? `${idPrefix}-id-error` : undefined
          }
        />
        {errors.external_id ? (
          <p id={`${idPrefix}-id-error`} className="text-destructive text-xs">
            {errors.external_id}
          </p>
        ) : null}
      </div>
      <div className="flex gap-1">
        <Button
          type="submit"
          size="sm"
          disabled={add.isPending || !extId.trim()}
        >
          Add
        </Button>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          onClick={cancelAdding}
          disabled={add.isPending}
        >
          Cancel
        </Button>
      </div>
    </form>
  ) : null;

  const confirmDialog = (
    <AlertDialog
      open={confirmRemove !== null}
      onOpenChange={(o) => {
        if (!o) setConfirmRemove(null);
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            Unlink {confirmRemove?.source_label}?
          </AlertDialogTitle>
          <AlertDialogDescription>
            Removes the link to <code>{confirmRemove?.external_id}</code>.
            Subsequent metadata fetches won&apos;t auto-re-add it.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={remove.isPending}>
            Cancel
          </AlertDialogCancel>
          <AlertDialogAction
            onClick={onConfirmRemove}
            disabled={remove.isPending}
          >
            {remove.isPending ? (
              <>
                <Loader2 className="mr-1 size-3 animate-spin" /> Unlinking
              </>
            ) : (
              "Unlink"
            )}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );

  return {
    rows,
    isLoading: query.isLoading,
    adding,
    startAdding: () => setAdding(true),
    requestRemove: (row: ExternalIdRow) => setConfirmRemove(row),
    addForm,
    confirmDialog,
  };
}

export function ExternalIdsCard(props: ExternalIdsCardProps) {
  const { chrome: chromeProp, canEdit, ...target } = props;
  const editor = useExternalIdsEditor(target);
  const { rows, adding, startAdding, requestRemove } = editor;

  const chrome = chromeProp ?? "card";

  const header =
    chrome === "card" ? (
      <CardHeader className="flex flex-row items-center justify-between pb-2">
        <CardTitle className="text-sm font-medium">External IDs</CardTitle>
        {canEdit && !adding && (
          <Button
            variant="ghost"
            size="sm"
            onClick={startAdding}
            aria-label="Add identifier"
          >
            <Plus className="size-3.5" />
          </Button>
        )}
      </CardHeader>
    ) : (
      // Bare mode: tab label is the title; surface the "+ Add" affordance as
      // a thin right-aligned row rendered *below* the list (see the return
      // block), so it doesn't offset the content from the sibling tabs.
      canEdit &&
      !adding && (
        <div className="flex justify-end">
          <Button variant="ghost" size="sm" onClick={startAdding}>
            <Plus className="mr-1 size-3.5" /> Add ID
          </Button>
        </div>
      )
    );

  const body =
    chrome === "card" ? (
      // Card layout: the original divided-list rendering. Used by the
      // series-page panel + any other caller that wants the Card chrome.
      <>
        {editor.isLoading ? (
          <div className="text-muted-foreground flex items-center gap-2 py-3">
            <Loader2 className="size-4 animate-spin" /> Loading…
          </div>
        ) : rows.length === 0 && !adding ? (
          <div className="text-muted-foreground py-3">
            No identifiers linked yet.
          </div>
        ) : (
          <ul className="divide-y">
            {rows.map((r) => (
              <li
                key={r.source}
                className="flex items-center justify-between py-2"
              >
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <span className="font-medium">{r.source_label}</span>
                  </div>
                  {r.external_url ? (
                    <a
                      href={r.external_url}
                      target="_blank"
                      rel="noreferrer"
                      className="text-muted-foreground block truncate text-xs hover:underline"
                    >
                      {r.external_id} ↗
                    </a>
                  ) : (
                    <code className="text-muted-foreground block truncate text-xs">
                      {r.external_id}
                    </code>
                  )}
                </div>
                {canEdit && (
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => requestRemove(r)}
                    aria-label={`Remove ${r.source_label} link`}
                  >
                    <Trash2 className="size-3.5" />
                  </Button>
                )}
              </li>
            ))}
          </ul>
        )}
        {editor.addForm}
      </>
    ) : (
      // Bare layout: ChipList-style grid matching the Credits / Cast
      // & Setting tabs. Each row renders as
      //   LABEL (uppercase muted)
      //   [chip]   (clickable, opens external_url in a new tab)
      // The trash-can affordance becomes a tiny ghost button beside
      // the chip. An id that is here is shown as-is: who linked it (you,
      // a provider match, the file) is not labelled, so no id reads as
      // less trustworthy than another. The precedence rule that keeps a
      // manually linked id from being replaced still applies server-side.
      <>
        {editor.isLoading ? (
          <div className="text-muted-foreground flex items-center gap-2 py-3">
            <Loader2 className="size-4 animate-spin" /> Loading…
          </div>
        ) : rows.length === 0 && !adding ? (
          <p className="text-muted-foreground text-sm">
            No identifiers linked yet.
          </p>
        ) : (
          <div className="grid grid-cols-1 gap-6 sm:grid-cols-2 md:grid-cols-3">
            {rows.map((r) => (
              <div key={r.source} className="space-y-2">
                <h3 className="text-muted-foreground text-xs font-semibold tracking-wider uppercase">
                  {r.source_label}
                </h3>
                <div className="flex flex-wrap items-center gap-1.5">
                  {r.external_url ? (
                    <a
                      href={r.external_url}
                      target="_blank"
                      rel="noreferrer"
                      title={`Open ${r.source_label} ${r.external_id}`}
                    >
                      <Badge
                        variant="secondary"
                        className="hover:bg-secondary/80 cursor-pointer font-normal"
                      >
                        {r.external_id}
                      </Badge>
                    </a>
                  ) : (
                    <Badge
                      variant="secondary"
                      className="cursor-default font-normal"
                    >
                      {r.external_id}
                    </Badge>
                  )}
                  {canEdit && (
                    <Button
                      variant="ghost"
                      size="icon"
                      className="text-muted-foreground/60 hover:text-foreground size-6"
                      onClick={() => requestRemove(r)}
                      aria-label={`Remove ${r.source_label} link`}
                    >
                      <Trash2 className="size-3" />
                    </Button>
                  )}
                </div>
              </div>
            ))}
          </div>
        )}
        {editor.addForm}
      </>
    );

  return (
    <>
      {chrome === "card" ? (
        <Card>
          {header}
          <CardContent className="space-y-2 text-sm">{body}</CardContent>
        </Card>
      ) : (
        // Bare mode renders the content first so the tab's top edge lines up
        // with every other tab (Credits, Cast, Details — all of which start
        // straight into their content). The "Add ID" affordance sits *below*
        // the list; placing it on its own row above left an empty band that
        // pushed the IDs down relative to the sibling tabs.
        <div className="space-y-3 text-sm">
          {body}
          {header}
        </div>
      )}
      {editor.confirmDialog}
    </>
  );
}
