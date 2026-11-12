"use client";

import * as React from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Copy, Pencil, Sparkles, Star, X } from "lucide-react";
import { toast } from "sonner";

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
import { Button } from "@/components/ui/button";
import { useCopyToClipboard } from "@/components/folio/copy-button";
import { NativeSelect } from "@/components/folio/native-select";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/folio/sheet";
import { Textarea } from "@/components/ui/textarea";
import { useIssueMarkers, useMarkerTags } from "@/lib/api/queries";
import { queryKeys } from "@/lib/api/query-keys";
import { showUndoToast } from "@/lib/ui/undo-toast";
import {
  useCreateMarker,
  useDeleteMarker,
  usePatchMarkerSelection,
  useRestoreMarkers,
  useUpdateMarker,
} from "@/lib/api/mutations/markers";
import type { MarkerSelection, TextRegionsView } from "@/lib/api/types";
import { toOcrRequestError } from "@/lib/reader/ocr-errors";
import { resolvePageSize, type PageSize } from "@/lib/reader/page-size";
import { useReaderStore, type PendingMarker } from "@/lib/reader/store";
import { statusToneText } from "@/lib/ui/status-tone";
import { cn } from "@/lib/utils";

import { ocrCroppedRegion } from "./marker-selection";

/** Server cap on `selection.text` (WP-5.3): OCR output or the user's edit
 *  of it, in UTF-8 bytes. */
export const MAX_CAPTURED_TEXT_BYTES = 8 * 1024;

/** Editor sheet for the pending marker. Shared between create flows
 *  (bookmark, note, highlight) and the edit-existing path (clicking a
 *  rendered marker in the overlay opens this with `editingMarkerId`
 *  set). The store owns the open state via `pendingMarker !== null`,
 *  so all paths converge through `beginMarkerEdit`. */
export function MarkerEditor({
  issueId,
  pageSizeSources,
}: {
  issueId: string;
  /** The reader's sources for a page's real pixel size: the server's page
   *  list and the `<img>`'s size when it loaded the original bytes. The
   *  editor adds the detection's size when the text-regions query has it
   *  and resolves them with `resolvePageSize`, the same as capture, so
   *  Detect text sends its rectangle in original page pixels. */
  pageSizeSources: (page: number) => {
    server: PageSize | null;
    measured: PageSize | null;
  };
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

  const { copy: copyText } = useCopyToClipboard();
  const queryClient = useQueryClient();
  const create = useCreateMarker();
  const restore = useRestoreMarkers();
  // The overlay already holds this query; reading it here costs no
  // request and gives Undo the persisted row (id, colour, page hash).
  const issueMarkers = useIssueMarkers(issueId);
  const update = useUpdateMarker(editingMarkerId ?? "", issueId);
  // Detect text / Clear patch the marker they were started on, by id —
  // never the hook-bound id above, which follows whatever marker the
  // editor shows by the time a slow read lands.
  const patchSelection = usePatchMarkerSelection(issueId);
  const del = useDeleteMarker(editingMarkerId ?? "", issueId, {
    silent: true,
  });

  const [body, setBody] = React.useState("");
  const [detectedText, setDetectedText] = React.useState<string | null>(null);
  const [detectedConfidence, setDetectedConfidence] = React.useState<
    number | null
  >(null);
  // OCR language override for the Detect/Re-detect button. Empty
  // string = Auto (server resolves series text_language → reading
  // direction → western).
  const [ocrLang, setOcrLang] = React.useState("");
  const [ocrPending, setOcrPending] = React.useState(false);
  // The captured text as the server has it for this marker (what Cancel
  // and Save compare against). Starts as the text the editor opened
  // with; Detect text / Clear on an existing marker save at once, so they
  // move it when their patch succeeds (D7).
  const [baselineText, setBaselineText] = React.useState("");
  // The text exactly as recognition produced it, or null when the current
  // text was typed or corrected by hand (stored text without a confidence
  // is hand-written: a hand edit drops the confidence). Replacing text
  // that differs from this asks first.
  const [recognizedText, setRecognizedText] = React.useState<string | null>(
    null,
  );
  const [confirmReplace, setConfirmReplace] = React.useState<
    "detect" | "clear" | null
  >(null);
  // The marker a Detect text read belongs to. Opening another marker, or
  // closing the editor, drops it, and a read that lands after that is
  // ignored: it must not fill another marker's field or patch it.
  const detectForRef = React.useRef<{
    pending: PendingMarker;
    markerId: string | null;
  } | null>(null);
  // Whether the editable captured-text field is shown: on open when the
  // marker already carries text, after an OCR pass, or when the user
  // chooses to type the text by hand (WP-5.3).
  const [capturedOpen, setCapturedOpen] = React.useState(false);
  const [isFavorite, setIsFavorite] = React.useState(false);
  const [tags, setTags] = React.useState<string[]>([]);
  const [tagInput, setTagInput] = React.useState("");
  // Autocomplete source — the user's existing tag set. Cached 60s so
  // typing doesn't churn the network.
  const tagSuggestionsQuery = useMarkerTags();
  React.useEffect(() => {
    detectForRef.current = null;
    if (pendingMarker) {
      // Snapshot the incoming marker into local form state once. React
      // Compiler can't see this is bounded by `if (pendingMarker)` and
      // would loop without the guard.
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setBody(pendingMarker.body ?? "");
      setDetectedText(pendingMarker.selection?.text ?? null);
      setDetectedConfidence(pendingMarker.selection?.ocr_confidence ?? null);
      setBaselineText(pendingMarker.selection?.text ?? "");
      setRecognizedText(
        pendingMarker.selection?.ocr_confidence != null
          ? (pendingMarker.selection?.text ?? null)
          : null,
      );
      setConfirmReplace(null);
      setCapturedOpen(!!pendingMarker.selection?.text);
      setOcrLang("");
      setIsFavorite(pendingMarker.is_favorite);
      setTags(pendingMarker.tags);
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

  /** Whether the form has unsaved edits vs. the marker it opened with. */
  function isDirty(): boolean {
    if (!pendingMarker) return false;
    if (body.trim() !== (pendingMarker.body?.trim() ?? "")) return true;
    if (isFavorite !== pendingMarker.is_favorite) return true;
    if (tags.join(" ") !== pendingMarker.tags.join(" ")) return true;
    if (detectedText !== null && detectedText !== baselineText) return true;
    return false;
  }

  /** Cancel / Esc / overlay-click path (audit C7): if the user typed
   *  something, don't drop it silently — close, then offer an Undo toast
   *  that re-opens the editor with the draft restored. A pristine draft
   *  (e.g. a highlight drawn but never captioned) just closes. */
  function discardAndClose() {
    if (!pendingMarker || !isDirty()) {
      close();
      return;
    }
    const draftSelection = currentSelection();
    const draft = {
      ...pendingMarker,
      body: body.trim(),
      is_favorite: isFavorite,
      tags,
      selection: draftSelection,
    };
    const editId = editingMarkerId;
    close();
    // The re-opened editor is the confirmation — no "Restored" toast.
    showUndoToast({
      message: "Discarded unsaved changes",
      tone: "neutral",
      restoredMessage: null,
      onUndo: () => beginMarkerEdit(draft, editId),
    });
  }

  /** The selection to save. `detectedText === null` means the captured
   *  text was never touched in this editor, so the marker's own selection
   *  stands. Otherwise the local text (OCR'd here, or typed / corrected by
   *  hand) replaces it; blank text drops the text (and its confidence)
   *  while keeping any other selection fields. */
  function currentSelection(): MarkerSelection | null {
    if (!pendingMarker) return null;
    if (detectedText === null) return pendingMarker.selection ?? null;
    if (!detectedText.trim())
      return selectionWithoutText(pendingMarker.selection);
    return withCapturedText(
      pendingMarker.selection,
      detectedText,
      detectedConfidence,
    );
  }

  async function handleSave() {
    if (!pendingMarker) return;
    const trimmedBody = body.trim();
    if (pendingMarker.kind === "note" && !trimmedBody) {
      toast.error("Notes need a body — type something or pick another kind.");
      return;
    }
    if (
      detectedText !== null &&
      new TextEncoder().encode(detectedText).length > MAX_CAPTURED_TEXT_BYTES
    ) {
      toast.error("Captured text is too long (max 8 KB).");
      return;
    }
    const mergedSelection = currentSelection();

    if (editingMarkerId) {
      // Only send `selection` when the captured text was edited here, so
      // a plain caption/tag edit never rewrites it.
      const textChanged =
        detectedText !== null && detectedText !== baselineText;
      update.mutate(
        {
          body: trimmedBody || null,
          is_favorite: isFavorite,
          tags,
          ...(textChanged ? { selection: mergedSelection } : {}),
        },
        { onSuccess: () => close() },
      );
      return;
    }

    // Stamp the page's pixel size onto the region so the saved-markers grid
    // renders the crop at its true aspect (no decode, no reflow). Only
    // region-bearing markers (highlights) need it; page-level markers
    // already crop with object-cover. Unknown size → omit and fall back to
    // the 2:3 assumption.
    const natural = pageSizeFor(pendingMarker.page_index);
    const regionWithDims =
      pendingMarker.region && natural
        ? {
            ...pendingMarker.region,
            page_w: natural.width,
            page_h: natural.height,
          }
        : (pendingMarker.region ?? null);

    create.mutate(
      {
        issue_id: issueId,
        page_index: pendingMarker.page_index,
        kind: pendingMarker.kind,
        region: regionWithDims,
        selection: mergedSelection,
        body: trimmedBody || null,
        is_favorite: isFavorite,
        tags,
      },
      { onSuccess: () => close() },
    );
  }

  function handleDelete() {
    if (!editingMarkerId || !pendingMarker) return;
    // Snapshot the persisted row before delete so Undo restores it in
    // one request with its id, colour and page hash (WP-8.4). Unsaved
    // edits would have gone through `update.mutate` first if the user
    // clicked Save, so the cached row is the current state.
    const persisted = issueMarkers.data?.items.find(
      (m) => m.id === editingMarkerId,
    );
    // Fallback when the cache doesn't hold the row: re-create from the
    // editor's view of the marker.
    const draft = {
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
        showUndoToast({
          message: "Removed",
          onUndo: () =>
            persisted
              ? restore.mutateAsync([persisted])
              : create.mutateAsync(draft),
        });
        close();
      },
    });
  }

  /** The page's real pixel size, resolved like capture does it. */
  function pageSizeFor(page: number): PageSize | null {
    const detected = queryClient.getQueryData<TextRegionsView>(
      queryKeys.issuePageTextRegions(issueId, page),
    );
    return resolvePageSize({
      detected: detected
        ? { width: detected.page_w, height: detected.page_h }
        : null,
      ...pageSizeSources(page),
    });
  }

  /** Whether the captured text in the field was typed or corrected by
   *  hand (it is not exactly what recognition produced). */
  function textIsHandEdited(): boolean {
    const current = detectedText ?? pendingMarker?.selection?.text ?? "";
    return current.trim() !== "" && current !== recognizedText;
  }

  /** Re-run OCR against the pending marker's region. For new highlights
   *  this fills the captured-text field, saved with the marker. For an
   *  existing highlight it patches the row at once, and the editor then
   *  counts that text as saved. */
  async function handleDetectText() {
    if (!pendingMarker?.region) {
      toast.error("Detect text needs a region — try Highlight a region first.");
      return;
    }
    // The region goes as percent; a known size only adds the pixel
    // fallback for a server that predates percent regions.
    const size = pageSizeFor(pendingMarker.page_index);
    const opened = pendingMarker;
    const forMarker = { pending: opened, markerId: editingMarkerId };
    detectForRef.current = forMarker;
    // Same marker still in the editor (checked again when the patch lands,
    // after this read has finished).
    const sameMarker = () => {
      const st = useReaderStore.getState();
      return (
        st.pendingMarker === forMarker.pending &&
        st.editingMarkerId === forMarker.markerId
      );
    };
    const stillOpen = () => detectForRef.current === forMarker && sameMarker();
    setOcrPending(true);
    const toastId = toast.loading("Reading text…");
    try {
      const ocr = await ocrCroppedRegion(
        {
          issueId,
          pageIndex: opened.page_index,
          region: opened.region!,
          pageSize: size,
        },
        {
          lang:
            ocrLang === "western" || ocrLang === "manga" ? ocrLang : undefined,
        },
      );
      toast.dismiss(toastId);
      // The editor moved on (another marker, or closed): drop the result.
      if (!stillOpen()) return;
      if (!ocr) {
        toast.message("No text found in that area.");
        return;
      }
      setDetectedText(ocr.text);
      setDetectedConfidence(ocr.confidence);
      setRecognizedText(ocr.text);
      setCapturedOpen(true);
      if (forMarker.markerId) {
        // The hook toasts "Text captured" for a selection-only patch.
        patchSelection.mutate(
          {
            id: forMarker.markerId,
            selection: withCapturedText(
              opened.selection,
              ocr.text,
              ocr.confidence,
            ),
          },
          {
            onSuccess: () => {
              if (sameMarker()) setBaselineText(ocr.text);
            },
          },
        );
      } else {
        toast.success("Text captured — save the marker to keep it.");
      }
    } catch (err) {
      toast.dismiss(toastId);
      if (stillOpen()) {
        toast.error(
          `Couldn't read the text. ${toOcrRequestError(err).message}`,
        );
      }
    } finally {
      if (detectForRef.current === forMarker) detectForRef.current = null;
      setOcrPending(false);
    }
  }

  /** Drop the OCR'd text from the marker. `""` is the local "cleared"
   *  sentinel that hides the preview and (for new markers) keeps the text
   *  out of the saved selection. For an existing marker it patches the row
   *  immediately, preserving any non-text selection fields. */
  function handleClearText() {
    setDetectedText("");
    setDetectedConfidence(null);
    setRecognizedText(null);
    setCapturedOpen(false);
    if (editingMarkerId && pendingMarker) {
      const opened = pendingMarker;
      const markerId = editingMarkerId;
      // The hook toasts "Detected text cleared" for this patch.
      patchSelection.mutate(
        { id: markerId, selection: selectionWithoutText(opened.selection) },
        {
          onSuccess: () => {
            const st = useReaderStore.getState();
            if (st.pendingMarker === opened && st.editingMarkerId === markerId)
              setBaselineText("");
          },
        },
      );
    }
  }

  /** Detect / Clear replace the captured text: ask first when the text in
   *  the field was written or corrected by hand, never when it is still
   *  exactly what recognition produced. */
  function replaceCapturedText(action: "detect" | "clear") {
    if (textIsHandEdited()) {
      setConfirmReplace(action);
      return;
    }
    if (action === "detect") void handleDetectText();
    else handleClearText();
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
        ? editingMarkerId
          ? "Optional caption."
          : pendingMarker.regionFitted
            ? "Optional caption. The highlight covers the detected text box."
            : "Optional caption. The highlight covers the area you drew."
        : `Page ${pendingMarker.page_index + 1}.`;

  const capturedText = detectedText ?? pendingMarker.selection?.text ?? "";
  // The OCR button shows whenever there's a region to OCR — so users
  // can run it on a plain rect highlight after the fact, or re-run on
  // an existing highlight whose text needs a refresh.
  const canDetectText = !!pendingMarker.region;
  // Manga-ocr reports a synthetic 1.0, so this hint effectively only
  // fires for western recognitions — exactly the engine whose
  // confidence is meaningful.
  const lowConfidence =
    !!capturedText && detectedConfidence != null && detectedConfidence < 0.6;

  async function handleCopyText() {
    if (!capturedText) return;
    if (await copyText(capturedText)) {
      toast.success("Copied");
    } else {
      toast.error("Couldn't copy to clipboard.");
    }
  }

  return (
    <Sheet
      open={open}
      onOpenChange={(next) => {
        if (!next) discardAndClose();
      }}
    >
      <SheetContent
        size="md"
        side="right"
        className="flex w-full flex-col gap-0"
      >
        <SheetHeader>
          <SheetTitle>{title}</SheetTitle>
          <SheetDescription>{description}</SheetDescription>
        </SheetHeader>
        <div className="flex flex-1 flex-col gap-4 px-4 py-4">
          {capturedOpen ? (
            <div className="space-y-1">
              <div className="flex items-center justify-between gap-2">
                <Label
                  htmlFor="marker-captured-text"
                  className="text-muted-foreground text-xs"
                >
                  Captured text
                </Label>
                <span className="flex items-center gap-1">
                  {/* Small visible boxes; on touch each carries the
                      invisible 44px tap area (`folio-tap` / the button
                      data-slot rule), widened to 44px across. */}
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="size-6 [--folio-tap-x:10px]"
                    onClick={handleCopyText}
                    aria-label="Copy detected text"
                  >
                    <Copy className="size-3.5" />
                  </Button>
                  <button
                    type="button"
                    onClick={() => replaceCapturedText("clear")}
                    disabled={ocrPending || patchSelection.isPending}
                    className="folio-tap text-muted-foreground hover:text-foreground inline-flex items-center gap-1 text-xs disabled:opacity-50"
                  >
                    <X className="size-3" />
                    Clear
                  </button>
                </span>
              </div>
              {/* Editable (WP-5.3): fix an OCR typo or type the text by
                  hand. A hand edit drops the OCR confidence — it no
                  longer describes the text. */}
              <Textarea
                id="marker-captured-text"
                value={capturedText}
                onChange={(e) => {
                  setDetectedText(e.target.value);
                  setDetectedConfidence(null);
                }}
                rows={3}
                className="max-h-40 text-sm"
                aria-describedby="marker-captured-text-hint"
              />
              <p
                id="marker-captured-text-hint"
                className="text-muted-foreground text-xs"
              >
                Searchable, and exported with your notes. Edit freely.
              </p>
              {lowConfidence ? (
                <p className={`text-xs ${statusToneText("warning")}`}>
                  Low confidence — text may be inaccurate.
                </p>
              ) : null}
            </div>
          ) : null}
          {canDetectText ? (
            <div className="flex items-center gap-2 self-start">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => replaceCapturedText("detect")}
                disabled={ocrPending}
              >
                <Sparkles className="mr-2 size-4" />
                {capturedText ? "Re-detect text" : "Detect text (OCR)"}
              </Button>
              {!capturedOpen ? (
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={() => setCapturedOpen(true)}
                >
                  <Pencil className="mr-2 size-4" />
                  Type text
                </Button>
              ) : null}
              <NativeSelect
                size="sm"
                aria-label="OCR language"
                value={ocrLang}
                onChange={setOcrLang}
                options={[
                  { value: "", label: "Auto" },
                  { value: "western", label: "Western (English text)" },
                  { value: "manga", label: "Japanese" },
                ]}
              />
            </div>
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
                className={cn("size-4", isFavorite ? "fill-current" : null)}
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
                      className="folio-tap text-muted-foreground hover:text-foreground [--folio-tap-x:16px]"
                    >
                      <X className="size-3" />
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
            <Button type="button" variant="ghost" onClick={discardAndClose}>
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
      <AlertDialog
        open={confirmReplace !== null}
        onOpenChange={(next) => {
          if (!next) setConfirmReplace(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {confirmReplace === "clear"
                ? "Clear the text you edited?"
                : "Replace the text you edited?"}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {confirmReplace === "clear"
                ? "The captured text was typed or corrected by hand. Clearing it can't be undone here."
                : "The captured text was typed or corrected by hand. Detecting again replaces it with what the recognizer reads."}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Keep my text</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                const action = confirmReplace;
                setConfirmReplace(null);
                if (action === "detect") void handleDetectText();
                else if (action === "clear") handleClearText();
              }}
            >
              {confirmReplace === "clear" ? "Clear text" : "Replace text"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Sheet>
  );
}

/** Selection with `text` replaced. `confidence` is the OCR confidence for
 *  that text, or `null` for hand-typed / hand-corrected text — then any
 *  stale `ocr_confidence` is dropped. Other fields (`image_hash`) are kept. */
export function withCapturedText(
  selection: MarkerSelection | null | undefined,
  text: string,
  confidence: number | null,
): MarkerSelection {
  const next: MarkerSelection = { ...(selection ?? {}), text };
  if (confidence != null) {
    next.ocr_confidence = confidence;
  } else {
    delete next.ocr_confidence;
  }
  return next;
}

/** Returns the selection with the OCR text (and its confidence) removed,
 *  preserving any other fields (e.g. `image_hash`). Collapses to `null` when
 *  nothing else remains so the row's whole selection is cleared. */
function selectionWithoutText(
  selection: MarkerSelection | null | undefined,
): MarkerSelection | null {
  if (!selection) return null;
  const kept: MarkerSelection = {};
  if (selection.image_hash != null) kept.image_hash = selection.image_hash;
  return Object.keys(kept).length > 0 ? kept : null;
}
