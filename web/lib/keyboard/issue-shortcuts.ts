/**
 * The issue page's bare-key shortcuts — data only, so the `?` shortcuts
 * sheet can list them without pulling in the hook's mutations. The
 * handler lives in [`useIssueShortcuts`](./use-issue-shortcuts.ts).
 */
export type IssueShortcutAction =
  "markRead" | "markUnread" | "toggleBookmark" | "readIncognito" | "editIssue";

/**
 * The single source for both the key handler and the shortcuts sheet's
 * "Issue page" section. Not rebindable (see `useIssueShortcuts`).
 */
export const ISSUE_SHORTCUTS: ReadonlyArray<{
  action: IssueShortcutAction;
  key: string;
  label: string;
  adminOnly?: boolean;
}> = [
  { action: "markRead", key: "r", label: "Mark as read" },
  { action: "markUnread", key: "u", label: "Mark as unread" },
  { action: "toggleBookmark", key: "b", label: "Bookmark / unbookmark" },
  { action: "readIncognito", key: "i", label: "Read in incognito" },
  { action: "editIssue", key: "e", label: "Edit issue", adminOnly: true },
];
