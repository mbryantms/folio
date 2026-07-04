"use client";

import * as React from "react";
import {
  DndContext,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  rectSortingStrategy,
  sortableKeyboardCoordinates,
  useSortable,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import {
  GripVertical,
  ImageUp,
  Loader2,
  RotateCw,
  SlidersHorizontal,
  Trash2,
  Undo2,
} from "lucide-react";
import { useRouter } from "next/navigation";
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
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { PageAdjustDialog } from "@/components/library/PageAdjustDialog";
import { useArchiveEditMutation, stageImageUpload } from "@/lib/api/mutations";
import { useArchivePageCount } from "@/lib/api/queries";
import {
  buildOps,
  hasChanges,
  initialSlots,
  rotateBy,
  summarizeOps,
  type PageSlot,
} from "@/lib/archive-edit";
import { cn } from "@/lib/utils";
import type { IssueDetailView, TransformStep } from "@/lib/api/types";

/**
 * Page-byte editor (`archive-rewrite-1.0` M3). A grid of the issue's
 * pages — drag to reorder, rotate, replace, or remove — that lowers to a
 * `PageOp[]` and enqueues an `ArchiveEditJob`. Admin-only; the caller
 * gates rendering on the parent library's `allow_archive_writeback`.
 */
export function PageEditor({
  issue,
  open,
  onOpenChange,
}: {
  issue: IssueDetailView;
  open: boolean;
  onOpenChange: (next: boolean) => void;
}) {
  const router = useRouter();
  const edit = useArchiveEditMutation(issue.id);
  // Build tiles from the archive's *real* page count (read live while the
  // dialog is open), not the DB `issue.page_count` — that can drift (stale
  // scan, or sourced from a ComicInfo `<PageCount>`) and would otherwise
  // render a phantom trailing page the archive doesn't have (deleting which
  // 422s as "ordinal out of range"). Fall back to the DB value if the live
  // read fails so the editor still opens.
  const countQuery = useArchivePageCount(issue.id, open);
  const resolvedCount: number | null =
    countQuery.data?.page_count ??
    (countQuery.isError ? (issue.page_count ?? 0) : null);
  const loadingCount = open && resolvedCount === null;
  const displayCount = resolvedCount ?? 0;

  const [slots, setSlots] = React.useState<PageSlot[]>([]);
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  const [uploadingOrig, setUploadingOrig] = React.useState<number | null>(null);
  const [adjustOrig, setAdjustOrig] = React.useState<number | null>(null);
  // Cache-buster minted per dialog open. The tile URLs are stable across
  // archive rewrites, so without this a reopen right after an apply shows
  // whatever the browser cached — including pre-edit thumbs pinned by the
  // pre-fix `immutable` policy. A fresh `?v=` forces a real fetch, and the
  // server regenerates wiped thumbs inline from the rewritten archive.
  const [openNonce, setOpenNonce] = React.useState(0);

  // Render-phase reconciliation (this codebase avoids set-state-in-effect):
  // (re)build the working list once the dialog is open and the real count
  // resolves, and clear it on close so a reopen re-fetches fresh.
  const [builtFor, setBuiltFor] = React.useState<number | null>(null);
  if (open && resolvedCount !== null && resolvedCount !== builtFor) {
    setBuiltFor(resolvedCount);
    setSlots(initialSlots(resolvedCount));
    setOpenNonce((n) => n + 1);
  } else if (!open && builtFor !== null) {
    setBuiltFor(null);
    setSlots([]);
  }

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    }),
  );

  const onDragEnd = (e: DragEndEvent) => {
    const { active, over } = e;
    if (!over || active.id === over.id) return;
    const oldIdx = slots.findIndex((s) => s.orig === Number(active.id));
    const newIdx = slots.findIndex((s) => s.orig === Number(over.id));
    if (oldIdx < 0 || newIdx < 0) return;
    setSlots((s) => arrayMove(s, oldIdx, newIdx));
  };

  const mutateSlot = (orig: number, patch: Partial<PageSlot>) =>
    setSlots((s) =>
      s.map((sl) => (sl.orig === orig ? { ...sl, ...patch } : sl)),
    );

  const onReplace = async (orig: number, file: File) => {
    setUploadingOrig(orig);
    try {
      const res = await stageImageUpload(file);
      mutateSlot(orig, { replaceId: res.id });
    } catch {
      toast.error("Upload failed");
    } finally {
      setUploadingOrig(null);
    }
  };

  const ops = buildOps(slots);
  const dirty = hasChanges(slots);
  const survivors = slots.filter((s) => !s.removed).length;

  // CBR archives can't be written in place — editing converts them to
  // CBZ. Explain the one-time-per-library extension change in the confirm.
  const isCbr = issue.file_path.toLowerCase().endsWith(".cbr");
  const showConversionNote = isCbr && !issue.library_cbr_convert_confirmed;

  const apply = () => {
    edit.mutate(
      { ops },
      {
        onSuccess: () => {
          setConfirmOpen(false);
          onOpenChange(false);
          // Radix sets `pointer-events: none` on <body> while a dialog is
          // open and only restores it once the last one closes. Closing the
          // confirm AlertDialog and the editor Dialog together, in the same
          // tick as the RSC `router.refresh()` below, races that restore —
          // and because a soft refresh doesn't remount MainShell (whose mount
          // effect clears the lock), the body can stay unclickable until a
          // hard reload. Defer the refresh past the close and clear any
          // residual lock ourselves. Mirrors the workaround in MainShell.
          setTimeout(() => {
            if (typeof document !== "undefined") {
              document.body.style.pointerEvents = "";
            }
            // The rewrite + rescan run in the background; refresh so the page
            // re-fetches when the scan lands.
            router.refresh();
          }, 0);
        },
      },
    );
  };

  return (
    <>
      <Dialog open={open} onOpenChange={onOpenChange}>
        <DialogContent className="flex max-h-[90vh] w-full flex-col gap-0 p-0 sm:max-w-5xl">
          <DialogHeader className="border-border border-b px-6 py-4">
            <DialogTitle>Edit archive</DialogTitle>
            <DialogDescription>
              Reorder, rotate, replace, or remove pages. Changes rewrite the
              archive file; a backup of the previous version is kept. Closes and
              re-scans when applied.
            </DialogDescription>
          </DialogHeader>

          <div className="flex-1 overflow-y-auto px-6 py-4">
            {loadingCount ? (
              <div className="text-muted-foreground flex items-center gap-2 py-12 text-sm">
                <Loader2 className="h-4 w-4 animate-spin" /> Loading pages…
              </div>
            ) : displayCount === 0 ? (
              <p className="text-muted-foreground text-sm">
                This issue has no pages to edit.
              </p>
            ) : (
              <DndContext
                sensors={sensors}
                collisionDetection={closestCenter}
                onDragEnd={onDragEnd}
              >
                <SortableContext
                  items={slots.map((s) => s.orig)}
                  strategy={rectSortingStrategy}
                >
                  <ul className="grid grid-cols-2 gap-3 sm:grid-cols-3 md:grid-cols-4">
                    {slots.map((slot, idx) => (
                      <PageCard
                        key={slot.orig}
                        issueId={issue.id}
                        cacheBust={openNonce}
                        slot={slot}
                        position={idx + 1}
                        uploading={uploadingOrig === slot.orig}
                        onRotate={() =>
                          mutateSlot(slot.orig, {
                            rotation: rotateBy(slot.rotation, 90),
                          })
                        }
                        onToggleRemove={() =>
                          mutateSlot(slot.orig, { removed: !slot.removed })
                        }
                        onReplace={(file) => onReplace(slot.orig, file)}
                        onAdjust={() => setAdjustOrig(slot.orig)}
                      />
                    ))}
                  </ul>
                </SortableContext>
              </DndContext>
            )}
          </div>

          <DialogFooter className="border-border flex items-center justify-between gap-2 border-t px-6 py-4 sm:justify-between">
            <span className="text-muted-foreground text-xs">
              {survivors} of {displayCount} pages kept
            </span>
            <div className="flex items-center gap-2">
              <Button
                type="button"
                variant="ghost"
                onClick={() => onOpenChange(false)}
                disabled={edit.isPending}
              >
                Cancel
              </Button>
              <Button
                type="button"
                disabled={!dirty || edit.isPending || loadingCount}
                onClick={() => setConfirmOpen(true)}
              >
                Apply changes
              </Button>
            </div>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Rewrite this archive?</AlertDialogTitle>
            <AlertDialogDescription>
              The following changes will be written to the archive file. A
              backup of the current version is kept for one-click restore.
            </AlertDialogDescription>
          </AlertDialogHeader>
          {showConversionNote && (
            <p className="border-border bg-muted text-muted-foreground rounded-md border px-3 py-2 text-xs">
              This is a <span className="font-medium">.cbr</span> (RAR) archive,
              which can&rsquo;t be edited in place. It will be converted to{" "}
              <span className="font-medium">.cbz</span> — the original{" "}
              <span className="font-mono">.cbr</span> is kept as a backup. This
              note won&rsquo;t show again for this library.
            </p>
          )}
          <ul className="text-muted-foreground max-h-48 list-disc space-y-1 overflow-y-auto pl-5 text-sm">
            {summarizeOps(ops).map((line, i) => (
              <li key={i}>{line}</li>
            ))}
          </ul>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={edit.isPending}>
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              onClick={(e) => {
                e.preventDefault();
                apply();
              }}
              disabled={edit.isPending}
            >
              {edit.isPending && (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              )}
              Rewrite archive
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {adjustOrig !== null &&
        (() => {
          const slot = slots.find((s) => s.orig === adjustOrig);
          if (!slot) return null;
          const position = slots.findIndex((s) => s.orig === adjustOrig) + 1;
          return (
            <PageAdjustDialog
              issueId={issue.id}
              orig={adjustOrig}
              position={position}
              open
              onOpenChange={(next) => {
                if (!next) setAdjustOrig(null);
              }}
              initial={slot.transform}
              onApply={(chain: TransformStep[] | null) =>
                mutateSlot(adjustOrig, { transform: chain })
              }
            />
          );
        })()}
    </>
  );
}

function PageCard({
  issueId,
  cacheBust,
  slot,
  position,
  uploading,
  onRotate,
  onToggleRemove,
  onReplace,
  onAdjust,
}: {
  issueId: string;
  /** Per-dialog-open nonce appended to the thumb URL (see PageEditor). */
  cacheBust: number;
  slot: PageSlot;
  position: number;
  uploading: boolean;
  onRotate: () => void;
  onToggleRemove: () => void;
  onReplace: (file: File) => void;
  onAdjust: () => void;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id: slot.orig });
  const fileRef = React.useRef<HTMLInputElement | null>(null);
  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.6 : undefined,
  };

  return (
    <li
      ref={setNodeRef}
      style={style}
      className={cn(
        "border-border bg-card relative flex flex-col overflow-hidden rounded-md border",
        slot.removed && "opacity-50",
      )}
    >
      <div className="bg-muted relative aspect-[2/3] overflow-hidden">
        {/* Thumbnail addresses the *original* page index; rotation is a
            client-side preview until the rewrite re-encodes it. */}
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img
          src={`/issues/${issueId}/pages/${slot.orig}/thumb?v=e${cacheBust}`}
          alt={`Page ${position}`}
          className="h-full w-full object-contain transition-transform"
          style={{ transform: `rotate(${slot.rotation}deg)` }}
          draggable={false}
        />
        {slot.replaceId && (
          <Badge className="absolute top-1 right-1 text-[10px]">Replaced</Badge>
        )}
        {!slot.replaceId && slot.transform && slot.transform.length > 0 && (
          <Badge
            variant="secondary"
            className="absolute top-1 right-1 text-[10px]"
          >
            Adjusted
          </Badge>
        )}
        {slot.removed && (
          <div className="bg-destructive/10 absolute inset-0 grid place-items-center">
            <Badge variant="destructive">Removed</Badge>
          </div>
        )}
        <button
          type="button"
          aria-label={`Drag page ${position}`}
          className="bg-background/70 text-muted-foreground hover:text-foreground absolute top-1 left-1 grid h-7 w-7 cursor-grab place-items-center rounded-md active:cursor-grabbing"
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
          {...(attributes as any)}
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
          {...(listeners as any)}
        >
          <GripVertical className="h-4 w-4" />
        </button>
      </div>
      <div className="flex items-center justify-between gap-1 px-2 py-1.5">
        <span className="text-muted-foreground text-xs tabular-nums">
          {position}
        </span>
        <div className="flex items-center gap-0.5">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7"
            onClick={onRotate}
            disabled={slot.removed}
            aria-label={`Rotate page ${position}`}
            title="Rotate 90°"
          >
            <RotateCw className="h-3.5 w-3.5" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className={cn(
              "h-7 w-7",
              slot.transform && slot.transform.length > 0 && "text-primary",
            )}
            onClick={onAdjust}
            disabled={slot.removed}
            aria-label={`Adjust page ${position}`}
            title="Adjust image (brightness, levels, sharpen, crop)"
          >
            <SlidersHorizontal className="h-3.5 w-3.5" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7"
            onClick={() => fileRef.current?.click()}
            disabled={slot.removed || uploading}
            aria-label={`Replace page ${position}`}
            title="Replace with an image"
          >
            {uploading ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <ImageUp className="h-3.5 w-3.5" />
            )}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className={cn(
              "h-7 w-7",
              !slot.removed && "text-muted-foreground hover:text-destructive",
            )}
            onClick={onToggleRemove}
            aria-label={
              slot.removed ? `Keep page ${position}` : `Remove page ${position}`
            }
            title={slot.removed ? "Keep page" : "Remove page"}
          >
            {slot.removed ? (
              <Undo2 className="h-3.5 w-3.5" />
            ) : (
              <Trash2 className="h-3.5 w-3.5" />
            )}
          </Button>
        </div>
      </div>
      <input
        ref={fileRef}
        type="file"
        accept="image/*"
        className="hidden"
        onChange={(e) => {
          const file = e.target.files?.[0];
          if (file) onReplace(file);
          e.target.value = "";
        }}
      />
    </li>
  );
}
