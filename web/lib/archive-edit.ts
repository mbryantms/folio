/**
 * Pure helpers for the page editor (`archive-rewrite-1.0` M3).
 *
 * The editor keeps an ordered list of {@link PageSlot}s — one per original
 * page, reordered in place by drag, with per-slot rotation / replacement /
 * removal flags. {@link buildOps} lowers that display state into the
 * sequential `PageOp[]` the server applies; {@link summarizeOps} renders a
 * human-readable confirm-dialog summary.
 *
 * Kept framework-free so it's unit-testable without a DOM.
 */
import type { PageOp, Rot } from "@/lib/api/types";

/** One page in the editor's working list. `orig` is the 0-based index of
 *  the page in the *original* archive (drives the thumbnail URL); the
 *  array position is the desired display order. */
export type PageSlot = {
  orig: number;
  /** Net clockwise rotation in degrees: 0 | 90 | 180 | 270. */
  rotation: number;
  removed: boolean;
  /** Staged-upload id when the page is being replaced, else null. */
  replaceId: string | null;
};

/** Initial slot list for an `n`-page archive: identity order, no edits. */
export function initialSlots(n: number): PageSlot[] {
  return Array.from({ length: n }, (_, i) => ({
    orig: i,
    rotation: 0,
    removed: false,
    replaceId: null,
  }));
}

/** Add `deg` (a positive multiple of 90) to a slot's rotation, wrapping at
 *  360. Used by the rotate buttons. */
export function rotateBy(rotation: number, deg: number): number {
  return (((rotation + deg) % 360) + 360) % 360;
}

function rotToEnum(deg: number): Rot | null {
  switch (deg) {
    case 90:
      return "r90";
    case 180:
      return "r180";
    case 270:
      return "r270";
    default:
      return null;
  }
}

/** True when the slot list differs from a pristine archive. */
export function hasChanges(slots: PageSlot[]): boolean {
  return buildOps(slots).length > 0;
}

/**
 * Lower the display state into a sequential `PageOp[]`.
 *
 * Server semantics are sequential + positional (each op addresses the
 * current list), so we emit in a fixed order that reproduces the desired
 * final state from `[0..originalCount)`:
 *
 *   1. **Removes**, highest original index first — going high→low means
 *      each removed index still equals its current position.
 *   2. **Reorder** — one permutation over the survivors (now in original
 *      order) into the desired display order. Skipped if already identity.
 *   3. **Rotate** — one op per rotated survivor, addressing final position.
 *   4. **Replace** — one op per replaced survivor, addressing final
 *      position.
 *
 * The slot list always holds every original page (removed flagged in
 * place), so the original count is implicit and not a separate argument.
 */
export function buildOps(slots: PageSlot[]): PageOp[] {
  const ops: PageOp[] = [];

  // 1. Removes (descending original index).
  const removedOrigs = slots
    .filter((s) => s.removed)
    .map((s) => s.orig)
    .sort((a, b) => b - a);
  for (const orig of removedOrigs) {
    ops.push({ kind: "remove", ordinal: orig });
  }

  // Survivors in display order.
  const survivors = slots.filter((s) => !s.removed);

  // 2. Reorder. After the removes, the working list is the survivors in
  // ascending-original order; map that to the desired display order.
  const ascByOrig = [...survivors].sort((a, b) => a.orig - b.orig);
  const newOrder = survivors.map((s) =>
    ascByOrig.findIndex((c) => c.orig === s.orig),
  );
  const isIdentity = newOrder.every((v, i) => v === i);
  if (!isIdentity && newOrder.length > 0) {
    ops.push({ kind: "reorder", new_order: newOrder });
  }

  // 3 + 4. Rotations and replacements address the final (post-reorder)
  // positions, i.e. the display index of each survivor.
  survivors.forEach((s, i) => {
    const rot = rotToEnum(s.rotation % 360);
    if (rot) ops.push({ kind: "rotate", ordinal: i, degrees: rot });
  });
  survivors.forEach((s, i) => {
    if (s.replaceId) {
      ops.push({ kind: "replace", ordinal: i, image_id: s.replaceId });
    }
  });

  return ops;
}

/** Human-readable, one-line-per-op summary for the confirm dialog. Ops
 *  reference 1-based page numbers in the *current* list for readability. */
export function summarizeOps(ops: PageOp[]): string[] {
  return ops.map((op) => {
    switch (op.kind) {
      case "remove":
        return `Remove page ${op.ordinal + 1}`;
      case "reorder":
        return `Reorder pages (${op.new_order.length} pages)`;
      case "rotate": {
        const deg = op.degrees.replace("r", "");
        return `Rotate page ${op.ordinal + 1} by ${deg}°`;
      }
      case "replace":
        return `Replace page ${op.ordinal + 1}`;
    }
  });
}
