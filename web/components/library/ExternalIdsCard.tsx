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
} from "@/components/ui/select";
import {
  useAddExternalIdIssue,
  useAddExternalIdSeries,
  useDeleteExternalIdIssue,
  useDeleteExternalIdSeries,
} from "@/lib/api/mutations";
import { useExternalIdsIssue, useExternalIdsSeries } from "@/lib/api/queries";
import { statusTone } from "@/lib/ui/status-tone";
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

export type ExternalIdsCardProps = (
  | { entityType: "series"; seriesSlug: string }
  | { entityType: "issue"; seriesSlug: string; issueSlug: string }
) & {
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
};

export function ExternalIdsCard(props: ExternalIdsCardProps) {
  const seriesList = useExternalIdsSeries(
    props.entityType === "series" ? props.seriesSlug : "",
  );
  const issueList = useExternalIdsIssue(
    props.entityType === "issue" ? props.seriesSlug : "",
    props.entityType === "issue" ? props.issueSlug : "",
  );
  const addSeries = useAddExternalIdSeries(
    props.entityType === "series" ? props.seriesSlug : "",
  );
  const deleteSeries = useDeleteExternalIdSeries(
    props.entityType === "series" ? props.seriesSlug : "",
  );
  const addIssue = useAddExternalIdIssue(
    props.entityType === "issue" ? props.seriesSlug : "",
    props.entityType === "issue" ? props.issueSlug : "",
  );
  const deleteIssue = useDeleteExternalIdIssue(
    props.entityType === "issue" ? props.seriesSlug : "",
    props.entityType === "issue" ? props.issueSlug : "",
  );

  const query = props.entityType === "series" ? seriesList : issueList;
  const add = props.entityType === "series" ? addSeries : addIssue;
  const remove = props.entityType === "series" ? deleteSeries : deleteIssue;

  const rows = query.data?.rows ?? [];
  const [adding, setAdding] = React.useState(false);
  const [source, setSource] = React.useState("comicvine");
  const [extId, setExtId] = React.useState("");
  const [confirmRemove, setConfirmRemove] =
    React.useState<ExternalIdRow | null>(null);

  const onAdd = (e: React.FormEvent) => {
    e.preventDefault();
    if (!extId.trim()) return;
    add.mutate(
      { source, external_id: extId.trim() },
      {
        onSuccess: () => {
          setAdding(false);
          setExtId("");
        },
      },
    );
  };

  const onConfirmRemove = () => {
    if (!confirmRemove) return;
    remove.mutate(
      { source: confirmRemove.source },
      {
        onSuccess: () => setConfirmRemove(null),
      },
    );
  };

  const chrome = props.chrome ?? "card";
  const header =
    chrome === "card" ? (
      <CardHeader className="flex flex-row items-center justify-between pb-2">
        <CardTitle className="text-sm font-medium">External IDs</CardTitle>
        {!adding && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setAdding(true)}
            aria-label="Add identifier"
          >
            <Plus className="h-3.5 w-3.5" />
          </Button>
        )}
      </CardHeader>
    ) : (
      // Bare mode: tab label is the title; surface the "+ Add" affordance as
      // a thin right-aligned row rendered *below* the list (see the return
      // block), so it doesn't offset the content from the sibling tabs.
      !adding && (
        <div className="flex justify-end">
          <Button variant="ghost" size="sm" onClick={() => setAdding(true)}>
            <Plus className="mr-1 h-3.5 w-3.5" /> Add ID
          </Button>
        </div>
      )
    );

  const body =
    chrome === "card" ? (
      // Card layout: the original divided-list rendering. Used by the
      // series-page panel + any other caller that wants the Card chrome.
      <>
        {query.isLoading ? (
          <div className="text-muted-foreground flex items-center gap-2 py-3">
            <Loader2 className="h-4 w-4 animate-spin" /> Loading…
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
                    {r.set_by === "user" && (
                      <span
                        className={`rounded px-1.5 py-0.5 text-xs ${statusTone("warning")}`}
                      >
                        User-set
                      </span>
                    )}
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
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setConfirmRemove(r)}
                  aria-label={`Remove ${r.source_label} link`}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </li>
            ))}
          </ul>
        )}
        {adding &&
          renderAddForm(
            source,
            setSource,
            extId,
            setExtId,
            onAdd,
            add.isPending,
            () => {
              setAdding(false);
              setExtId("");
            },
          )}
      </>
    ) : (
      // Bare layout: ChipList-style grid matching the Credits / Cast
      // & Setting tabs. Each row renders as
      //   LABEL (uppercase muted)
      //   [chip]   (clickable, opens external_url in a new tab)
      // The trash-can affordance becomes a tiny ghost button beside
      // the chip; the User-set marker becomes a small outline badge
      // sitting alongside.
      <>
        {query.isLoading ? (
          <div className="text-muted-foreground flex items-center gap-2 py-3">
            <Loader2 className="h-4 w-4 animate-spin" /> Loading…
          </div>
        ) : rows.length === 0 && !adding ? (
          <p className="text-muted-foreground text-sm">
            No identifiers linked yet.
          </p>
        ) : (
          <div className="grid gap-6 sm:grid-cols-2 md:grid-cols-3">
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
                  {r.set_by === "user" && (
                    <Badge variant="outline" className="font-normal">
                      User-set
                    </Badge>
                  )}
                  <Button
                    variant="ghost"
                    size="icon"
                    className="text-muted-foreground/60 hover:text-foreground h-6 w-6"
                    onClick={() => setConfirmRemove(r)}
                    aria-label={`Remove ${r.source_label} link`}
                  >
                    <Trash2 className="h-3 w-3" />
                  </Button>
                </div>
              </div>
            ))}
          </div>
        )}
        {adding &&
          renderAddForm(
            source,
            setSource,
            extId,
            setExtId,
            onAdd,
            add.isPending,
            () => {
              setAdding(false);
              setExtId("");
            },
          )}
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
                  <Loader2 className="mr-1 h-3 w-3 animate-spin" /> Unlinking
                </>
              ) : (
                "Unlink"
              )}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

/**
 * Inline "+ Add ID" form. Same shape in both card + bare layouts so the
 * "Source / ID / Add / Cancel" UX is identical; only the outer chrome
 * differs.
 */
function renderAddForm(
  source: string,
  setSource: (v: string) => void,
  extId: string,
  setExtId: (v: string) => void,
  onAdd: (e: React.FormEvent) => void,
  isPending: boolean,
  onCancel: () => void,
) {
  return (
    <form
      onSubmit={onAdd}
      className="flex flex-col gap-2 border-t pt-3 sm:flex-row sm:items-end"
    >
      <div className="grid flex-1 gap-1.5">
        <Label htmlFor="eic-source" className="text-xs">
          Source
        </Label>
        <Select value={source} onValueChange={setSource}>
          <SelectTrigger id="eic-source">
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
      </div>
      <div className="grid flex-[2] gap-1.5">
        <Label htmlFor="eic-id" className="text-xs">
          ID
        </Label>
        <Input
          id="eic-id"
          value={extId}
          onChange={(e) => setExtId(e.target.value)}
          placeholder="e.g. 12345"
          autoFocus
        />
      </div>
      <div className="flex gap-1">
        <Button type="submit" size="sm" disabled={isPending || !extId.trim()}>
          Add
        </Button>
        <Button
          type="button"
          size="sm"
          variant="ghost"
          onClick={onCancel}
          disabled={isPending}
        >
          Cancel
        </Button>
      </div>
    </form>
  );
}
