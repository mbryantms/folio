/**
 * The reading-list page's bare-key shortcuts (linked reading lists,
 * PR 3a) — data only, so the `?` shortcuts sheet can list them without
 * pulling in the page. The handler lives in `CblViewDetail`; not
 * rebindable, like the issue page's (`./issue-shortcuts.ts`).
 */
export type ListShortcutAction = "nextChapter" | "prevChapter";

export const LIST_SHORTCUTS: ReadonlyArray<{
  action: ListShortcutAction;
  key: string;
  label: string;
}> = [
  { action: "nextChapter", key: "]", label: "Next chapter" },
  {
    action: "prevChapter",
    key: "[",
    label: "Previous chapter (or the start of this one)",
  },
];

/** The action bound to a key press, if any. */
export function listShortcutFor(key: string): ListShortcutAction | null {
  return LIST_SHORTCUTS.find((s) => s.key === key)?.action ?? null;
}
