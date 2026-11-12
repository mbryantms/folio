/**
 * "Undo instead of confirm" (folio-flair-1.0).
 *
 * Recoverable removals happen immediately and raise one of these toasts
 * instead of asking "Are you sure?" first. The toast carries an **Undo**
 * action that runs the caller's restore; a success / failure toast
 * follows so the user knows whether the restore landed.
 *
 * Only actions the client can actually reverse through existing
 * endpoints (re-create from a snapshot, re-add entries, re-PUT a layout)
 * use this. Truly unrecoverable actions (deleting a library, wiping
 * downloaded files, deleting a CBL list with its manual matches) keep
 * their confirm dialog.
 *
 * Every call site shares the wording ("Undo" / "Restored" / "Couldn't
 * undo …") and the shared Undo window, `UNDO_TOAST_DURATION_MS`.
 */
import { toast } from "sonner";

import { UNDO_TOAST_DURATION_MS } from "@/lib/api/toast-strings";

export { UNDO_TOAST_DURATION_MS };

export type UndoToastOptions = {
  /** The toast title — what just happened, e.g. `Deleted "Fables"`. */
  message: string;
  /** Optional secondary line under the title. */
  description?: string;
  /** Reverses the action. May be async; a throw / rejection surfaces
   *  `failedMessage` as an error toast. */
  onUndo: () => unknown;
  /** Toast shown once the undo has landed. Defaults to `"Restored"`.
   *  `null` skips it — for undos whose result is already on screen
   *  (e.g. re-opening an editor with the discarded draft). */
  restoredMessage?: string | null;
  /** Error toast when `onUndo` fails. */
  failedMessage?: string;
  /** Reuse an existing sonner toast id — lets the Undo toast replace a
   *  success toast a mutation hook has already raised under that id,
   *  instead of stacking a second one. */
  id?: string | number;
  /** Override the Undo window. Every site should leave this alone. */
  duration?: number;
  /** `"success"` (default) shows sonner's success icon; `"neutral"` a
   *  plain toast — for actions that aren't an accomplishment, like
   *  discarding a draft. */
  tone?: "success" | "neutral";
};

/** Thrown by an `onUndo` that failed in a way worth saying more about
 *  than `failedMessage` (e.g. what was cleaned up): its message is the
 *  error toast. */
export class UndoFailedError extends Error {}

/** Pop the shared "done — Undo" toast. Returns the sonner toast id. */
export function showUndoToast({
  message,
  description,
  onUndo,
  restoredMessage = "Restored",
  failedMessage = "Couldn't undo that",
  id,
  duration = UNDO_TOAST_DURATION_MS,
  tone = "success",
}: UndoToastOptions): string | number {
  // Sonner dismisses the toast when the action is clicked, but a fast
  // double-click (or a re-render racing the dismiss) must not replay the
  // restore twice — that would, e.g., recreate a collection twice.
  let used = false;
  const show = tone === "neutral" ? toast : toast.success;
  return show(message, {
    ...(id !== undefined ? { id } : {}),
    ...(description ? { description } : {}),
    duration,
    action: {
      label: "Undo",
      onClick: () => {
        if (used) return;
        used = true;
        void (async () => {
          try {
            await onUndo();
            if (restoredMessage !== null) toast.success(restoredMessage);
          } catch (err) {
            toast.error(
              err instanceof UndoFailedError ? err.message : failedMessage,
            );
          }
        })();
      },
    },
  });
}

/**
 * Order `current` to match `before` — the arrangement captured before a
 * removal — so an undone item returns to its old slot rather than the
 * tail it was re-added at. Items `before` doesn't know (added since) keep
 * their relative order after the known ones. `keyOf` maps an item to the
 * key used in `before` (default: the item itself).
 *
 *   restoreOrder(["a", "c", "b2"], ["a", "b2", "c"]) // ["a", "b2", "c"]
 */
export function restoreOrder<T, K = T>(
  current: readonly T[],
  before: readonly K[],
  keyOf: (item: T) => K | null = (item) => item as unknown as K,
): T[] {
  const rank = new Map(before.map((k, i) => [k, i]));
  return current
    .map((item, i) => {
      const k = keyOf(item);
      const r = k === null ? undefined : rank.get(k);
      return { item, rank: r ?? before.length + i };
    })
    .sort((a, b) => a.rank - b.rank)
    .map((e) => e.item);
}
