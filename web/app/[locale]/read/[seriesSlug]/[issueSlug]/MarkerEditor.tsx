"use client";

import * as React from "react";
import { Sparkles, Star, X } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Textarea } from "@/components/ui/textarea";
import { useMarkerTags } from "@/lib/api/queries";
import {
  useCreateMarker,
  useDeleteMarker,
  useUpdateMarker,
} from "@/lib/api/mutations";
import { useReaderStore } from "@/lib/reader/store";
import { cn } from "@/lib/utils";

import { ocrCroppedRegion } from "./marker-selection";

/** Editor sheet for the pending marker. Shared between create flows
 *  (bookmark, note, highlight) and the edit-existing path (clicking a
 *  rendered marker in the overlay opens this with `editingMarkerId`
 *  set). The store owns the open state via `pendingMarker !== null`,
 *  so all paths converge through `beginMarkerEdit`. */
export function MarkerEditor({
  issueId,
  pageNaturalSize,
}: {
  issueId: string;
  /** Lookup-by-page natural pixel dimensions, threaded through from
   *  the parent so the "Detect text" button can re-run OCR against an
   *  existing region without re-fetching the source image's metadata. */
  pageNaturalSize: React.RefObject<
    Map<number, { width: number; height: number }>
  >;
}) {
  const pendingMarker = useReaderStore((s) => s.pendingMarker);
  const editingMarkerId = useReaderStore((s) => s.editingMarkerId);
  const beginMarkerEdit = useReaderStore((s) => s.beginMarkerEdit);
  const setChromePinned = useReaderStore((s) => s.setChromePinned);

  const open = pendingMarker !== null;

  // Pin chrome while the sheet is open so auto-hide doesn't yank the
  // header out from under the user mid-edit. Released when the sheet
  // closes (success, cancel, or page-flip cleanup in the parent).
  React.useEffect(() => {
    setChromePinned(open);
    return () => setChromePinned(false);
  }, [open, setChromePinned]);

  const create = useCreateMarker();
  const update = useUpdateMarker(editingMarkerId ?? "", issueId);
  const del = useDeleteMarker(editingMarkerId ?? "", issueId, {
    silent: true,
  });

  const [body, setBody] = React.useState("");
  const [detectedText, setDetectedText] = React.useState<string | null>(null);
  const [ocrPending, setOcrPending] = React.useState(false);
  const [isFavorite, setIsFavorite] = React.useState(false);
  const [tags, setTags] = React.useState<string[]>([]);
  const [tagInput, setTagInput] = React.useState("");
  // Autocomplete source — the user's existing tag set. Cached 60s so
  // typing doesn't churn the network.
  const tagSuggestionsQuery = useMarkerTags();
  React.useEffect(() => {
    if (pendingMarker) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setBody(pendingMarker.body ?? "");
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setDetectedText(pendingMarker.selection?.text ?? null);
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setIsFavorite(pendingMarker.is_favorite);
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setTags(pendingMarker.tags);
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setTagInput("");
    }
  }, [pendingMarker]);

  function addTag(raw: string) {
    const normalized = raw.trim().toLowerCase();
    if (!normalized) return;
    if (tags.includes(normalized)) {
      setTagInput("");
      return;
    }
    setTags((prev) => [...prev, normalized]);
    setTagInput("");
  }
  function removeTag(t: string) {
    setTags((prev) => prev.filter((x) => x !== t));
  }

  // Autocomplete suggestions: user's existing tags that match the
  // current input and aren't already on this marker. Capped at 6 to
  // keep the dropdown compact.
  const suggestions = React.useMemo(() => {
    const needle = tagInput.trim().toLowerCase();
    const all = tagSuggestionsQuery.data?.items ?? [];
    return all
      .map((t) => t.tag)
      .filter((t) => !tags.includes(t))
      .filter((t) => (needle ? t.includes(needle) : true))
      .slice(0, 6);
  }, [tagSuggestionsQuery.data, tagInput, tags]);

  function close() {
    beginMarkerEdit(null, null);
  }

  async function handleSave() {
    if (!pendingMarker) return;
    const trimmedBody = body.trim();
    if (pendingMarker.kind === "note" && !trimmedBody) {
      toast.error("Notes need a body — type something or pick another kind.");
      return;
    }

    if (editingMarkerId) {
      update.mutate(
        {
          body: trimmedBody || null,
          is_favorite: isFavorite,
          tags,
        },
        { onSuccess: () => close() },
      );
      return;
    }

    // If OCR ran inside this editor (not at drag time) the detected
    // text is in local state. Merge it into the saved selection so a
    // user-triggered "Detect text" persists alongside whatever the
    // overlay's drag-time pass produced.
    const mergedSelection = detectedText
      ? {
          ...(pendingMarker.selection ?? {}),
          text: detectedText,
        }
      : (pendingMarker.selection ?? null);

    create.mutate(
      {
        issue_id: issueId,
        page_index: pendingMarker.page_index,
        kind: pendingMarker.kind,
        region: pendingMarker.region ?? null,
        selection: mergedSelection,
        body: trimmedBody || null,
        is_favorite: isFavorite,
        tags,
      },
      {
        onSuccess: () => {
          const label =
            pendingMarker.kind === "bookmark"
              ? `Bookmarked page ${pendingMarker.page_index + 1}`
              : pendingMarker.kind === "highlight"
                ? `Highlight saved`
                : `Note saved`;
          toast.success(label);
          close();
        },
      },
    );
  }

  function handleDelete() {
    if (!editingMarkerId || !pendingMarker) return;
    // Snapshot the marker before delete so Undo can recreate it. We
    // capture the *editor's current view* of the marker (which mirrors
    // the persisted row at open time — unsaved edits would have gone
    // through `update.mutate` first if the user clicked Save).
    const snapshot = {
      issue_id: issueId,
      page_index: pendingMarker.page_index,
      kind: pendingMarker.kind,
      region: pendingMarker.region,
      selection: pendingMarker.selection,
      body: pendingMarker.body || null,
      color: null,
      is_favorite: pendingMarker.is_favorite,
      tags: pendingMarker.tags,
    };
    del.mutate(undefined, {
      onSuccess: () => {
        toast.success("Removed", {
          action: {
            label: "Undo",
            onClick: () => create.mutate(snapshot),
          },
        });
        close();
      },
    });
  }

  /** Re-run OCR against the pending marker's region. For new
   *  highlights this populates the local detected-text preview which
   *  gets saved through the regular `selection` payload. For existing
   *  highlights it patches the row in place so the global feed picks
   *  up the new text without re-creating the marker. */
  async function handleDetectText() {
    if (!pendingMarker?.region) {
      toast.error("Detect text needs a region — try Highlight a region first.");
      return;
    }
    const natural = pageNaturalSize.current?.get(pendingMarker.page_index);
    if (!natural) {
      toast.error("Image still loading — try again in a moment.");
      return;
    }
    setOcrPending(true);
    const toastId = toast.loading("Reading text…");
    try {
      const ocr = await ocrCroppedRegion({
        issueId,
        pageIndex: pendingMarker.page_index,
        region: pendingMarker.region,
        naturalSize: natural,
      });
      toast.dismiss(toastId);
      if (!ocr || !ocr.text.trim()) {
        toast.message("Couldn't read any text in that region.");
        return;
      }
      setDetectedText(ocr.text);
      if (editingMarkerId) {
        update.mutate(
          {
            selection: {
              text: ocr.text,
              ocr_confidence: ocr.confidence,
            },
          },
          {
            onSuccess: () => toast.success("Text captured"),
          },
        );
      } else {
        toast.success("Text captured — save the marker to keep it.");
      }
    } catch (err) {
      toast.dismiss(toastId);
      console.warn("markers: detect text failed", err);
      toast.error("OCR failed — see console for details.");
    } finally {
      setOcrPending(false);
    }
  }

  if (!pendingMarker) return null;

  const title =
    pendingMarker.kind === "note"
      ? editingMarkerId
        ? "Edit note"
        : "Add note"
      : pendingMarker.kind === "highlight"
        ? editingMarkerId
          ? "Edit highlight"
          : "Save highlight"
        : "Save bookmark";

  const description =
    pendingMarker.kind === "note"
      ? "Markdown-friendly. Saved to this user only."
      : pendingMarker.kind === "highlight"
        ? "Optional caption. The region is preserved as you drew it."
        : `Page ${pendingMarker.page_index + 1}.`;

  const selectionPreview = detectedText ?? pendingMarker.selection?.text;
  // The OCR button shows whenever there's a region to OCR — so users
  // can run it on a plain rect highlight after the fact, or re-run on
  // an existing highlight whose text needs a refresh.
  const canDetectText = !!pendingMarker.region;

  return (
    <Sheet
      open={open}
      onOpenChange={(next) => {
        if (!next) close();
      }}
    >
      <SheetContent
        side="right"
        className="flex w-full flex-col gap-0 sm:max-w-md"
      >
        <SheetHeader>
          <SheetTitle>{title}</SheetTitle>
          <SheetDescription>{description}</SheetDescription>
        </SheetHeader>
        <div className="flex flex-1 flex-col gap-4 px-4 py-4">
          {selectionPreview ? (
            <div className="space-y-1">
              <Label className="text-muted-foreground text-xs">
                Detected text
              </Label>
              <div className="border-border/60 bg-muted/30 max-h-32 overflow-y-auto rounded-md border p-2 text-sm whitespace-pre-wrap">
                {selectionPreview}
              </div>
            </div>
          ) : null}
          {canDetectText ? (
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={handleDetectText}
              disabled={ocrPending}
              className="self-start"
            >
              <Sparkles className="mr-2 h-4 w-4" />
              {selectionPreview ? "Re-detect text" : "Detect text (OCR)"}
            </Button>
          ) : null}
          <div className="space-y-1">
            <Label htmlFor="marker-body">
              {pendingMarker.kind === "note" ? "Note" : "Caption"}
            </Label>
            <Textarea
              id="marker-body"
              value={body}
              onChange={(e) => setBody(e.target.value)}
              rows={6}
              placeholder={
                pendingMarker.kind === "note"
                  ? "What did you want to remember?"
                  : "Optional"
              }
              autoFocus
            />
          </div>

          <div className="flex items-center justify-between gap-3">
            <div className="space-y-0.5">
              <Label htmlFor="marker-favorite" className="cursor-pointer">
                Favorite
              </Label>
              <p className="text-muted-foreground text-xs">
                Star this marker so you can find it under the Favorites filter.
              </p>
            </div>
            <button
              id="marker-favorite"
              type="button"
              aria-pressed={isFavorite}
              onClick={() => setIsFavorite((v) => !v)}
              className={cn(
                "focus-visible:ring-ring inline-flex h-9 w-9 items-center justify-center rounded-full border transition-colors focus-visible:ring-2 focus-visible:outline-none",
                isFavorite
                  ? "border-rose-500/60 bg-rose-500/20 text-rose-500"
                  : "border-border/60 text-muted-foreground hover:bg-accent/40",
              )}
            >
              <Star
                className={cn("h-4 w-4", isFavorite ? "fill-current" : null)}
                aria-hidden="true"
              />
            </button>
          </div>

          <div className="space-y-1">
            <Label htmlFor="marker-tag-input">Tags</Label>
            {tags.length > 0 ? (
              <div className="flex flex-wrap gap-1">
                {tags.map((t) => (
                  <span
                    key={t}
                    className="bg-muted text-foreground inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs"
                  >
                    {t}
                    <button
                      type="button"
                      onClick={() => removeTag(t)}
                      aria-label={`Remove ${t}`}
                      className="text-muted-foreground hover:text-foreground"
                    >
                      <X className="h-3 w-3" />
                    </button>
                  </span>
                ))}
              </div>
            ) : null}
            <div className="relative">
              <Input
                id="marker-tag-input"
                value={tagInput}
                onChange={(e) => setTagInput(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === ",") {
                    e.preventDefault();
                    addTag(tagInput);
                  } else if (
                    e.key === "Backspace" &&
                    tagInput === "" &&
                    tags.length > 0
                  ) {
                    e.preventDefault();
                    removeTag(tags[tags.length - 1]!);
                  }
                }}
                placeholder="Add a tag and press Enter…"
              />
              {/* Suggestions surface only while the input is non-empty
               *  or focused with prior tags to pick from. The list is
               *  capped at 6 in `suggestions` so the dropdown stays
               *  compact. */}
              {suggestions.length > 0 && tagInput.trim().length > 0 ? (
                <div className="bg-popover absolute top-full left-0 z-10 mt-1 w-full rounded-md border shadow-md">
                  {suggestions.map((t) => (
                    <button
                      key={t}
                      type="button"
                      onClick={() => addTag(t)}
                      className="hover:bg-accent/60 block w-full px-2 py-1 text-left text-xs"
                    >
                      {t}
                    </button>
                  ))}
                </div>
              ) : null}
            </div>
          </div>
        </div>
        <div className="border-border/60 flex flex-row items-center justify-between gap-2 border-t px-4 py-3">
          {editingMarkerId ? (
            <Button
              type="button"
              variant="ghost"
              className="text-destructive hover:text-destructive"
              onClick={handleDelete}
              disabled={del.isPending}
            >
              Delete
            </Button>
          ) : (
            <span />
          )}
          <div className="flex items-center gap-2">
            <Button type="button" variant="ghost" onClick={close}>
              Cancel
            </Button>
            <Button
              type="button"
              onClick={handleSave}
              disabled={create.isPending || update.isPending}
            >
              {editingMarkerId ? "Save" : "Save marker"}
            </Button>
          </div>
        </div>
      </SheetContent>
    </Sheet>
  );
}
