"use client";

import * as React from "react";
import { ChevronRight, Folder, FolderOpen, Home } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useAdminFsList } from "@/lib/api/queries";
import { cn } from "@/lib/utils";

/**
 * Click-to-open directory picker for filesystem paths inside the
 * configured library root (`COMIC_LIBRARY_PATH`). The trigger renders
 * inline as a faux input — same height + border as `<Input>` — so it
 * slots into a form alongside other text fields. Clicking it opens a
 * sub-dialog with a single-column folder browser:
 *
 *   - The breadcrumb at the top shows the current path; clicking a
 *     segment jumps directly there.
 *   - The body lists the immediate-child directories of the current
 *     path. Click a folder to drill in.
 *   - The "Select this folder" button at the bottom commits the
 *     currently-displayed path back to the parent form.
 *
 * Browse scope is enforced server-side: `GET /admin/fs/list` rejects
 * anything outside the canonicalised library root with 403. Even if the
 * UI lets a path through, the server won't.
 */
export function DirectoryPicker({
  value,
  onChange,
  placeholder = "Choose a folder…",
  disabled = false,
}: {
  /** Current value as shown in the trigger. Empty string means "no
   *  selection yet". */
  value: string;
  onChange: (path: string) => void;
  placeholder?: string;
  disabled?: boolean;
}) {
  const [open, setOpen] = React.useState(false);
  // The path the browser is currently showing. Snapped back to
  // `undefined` (= library root) on each open via the trigger handler
  // rather than an effect — stale browse state from a previous session
  // would be confusing, and avoiding `useEffect` keeps the React 19
  // setState-in-effect rule clean.
  const [browsing, setBrowsing] = React.useState<string | undefined>(undefined);

  const handleOpenChange = (next: boolean) => {
    if (next) setBrowsing(undefined);
    setOpen(next);
  };

  return (
    <>
      <button
        type="button"
        disabled={disabled}
        onClick={() => handleOpenChange(true)}
        className={cn(
          "border-input bg-background flex h-9 w-full items-center justify-between gap-2 rounded-md border px-3 text-left text-sm font-mono",
          "hover:bg-accent/40 focus-visible:ring-ring focus-visible:ring-2 focus-visible:outline-none",
          "disabled:cursor-not-allowed disabled:opacity-50",
        )}
        aria-haspopup="dialog"
      >
        <span
          className={cn(
            "min-w-0 truncate",
            value ? "text-foreground" : "text-muted-foreground",
          )}
        >
          {value || placeholder}
        </span>
        <FolderOpen
          className="text-muted-foreground h-4 w-4 shrink-0"
          aria-hidden="true"
        />
      </button>
      <BrowserDialog
        open={open}
        onOpenChange={handleOpenChange}
        browsing={browsing}
        onBrowse={setBrowsing}
        onSelect={(path) => {
          onChange(path);
          handleOpenChange(false);
        }}
      />
    </>
  );
}

function BrowserDialog({
  open,
  onOpenChange,
  browsing,
  onBrowse,
  onSelect,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  browsing: string | undefined;
  onBrowse: (path: string | undefined) => void;
  onSelect: (path: string) => void;
}) {
  const query = useAdminFsList(browsing, open);

  // Once we've landed on a path the server has confirmed canonical, the
  // breadcrumb derives from `query.data.path` (not `browsing`) — this
  // means symlink resolution shows up to the user.
  const currentPath = query.data?.path ?? browsing ?? "";
  const root = query.data?.root ?? "";
  const segments = breadcrumbSegments(root, currentPath);
  const atRoot = !root || currentPath === root;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>Choose a library folder</DialogTitle>
          <DialogDescription>
            Browse the server&apos;s filesystem inside the configured library
            root. Click a folder to drill in, or pick the current folder
            with the button below.
          </DialogDescription>
        </DialogHeader>

        {/* Breadcrumb */}
        <nav
          aria-label="Path"
          className="text-muted-foreground flex flex-wrap items-center gap-1 font-mono text-xs"
        >
          <button
            type="button"
            onClick={() => onBrowse(undefined)}
            className="hover:text-foreground inline-flex items-center gap-1 rounded px-1 py-0.5"
            disabled={atRoot}
            title="Library root"
          >
            <Home aria-hidden="true" className="h-3 w-3" />
            <span>root</span>
          </button>
          {segments.map((seg) => (
            <React.Fragment key={seg.path}>
              <ChevronRight aria-hidden="true" className="h-3 w-3 shrink-0" />
              <button
                type="button"
                onClick={() => onBrowse(seg.path)}
                className="hover:text-foreground rounded px-1 py-0.5"
              >
                {seg.name}
              </button>
            </React.Fragment>
          ))}
        </nav>

        {/* Listing */}
        <div className="border-border rounded-md border">
          <ScrollArea className="h-72">
            {query.isLoading && (
              <div className="text-muted-foreground p-4 text-sm">
                Loading…
              </div>
            )}
            {query.error && (
              <div className="text-destructive space-y-2 p-4 text-sm">
                <p>{describeFsError(query.error)}</p>
              </div>
            )}
            {query.data && query.data.entries.length === 0 && (
              <div className="text-muted-foreground p-4 text-sm">
                This folder is empty. Use &ldquo;Select this folder&rdquo;
                to pick it anyway, or step back up to choose a different
                one.
              </div>
            )}
            {query.data && query.data.entries.length > 0 && (
              <ul className="p-1">
                {query.data.entries.map((entry) => (
                  <li key={entry.path}>
                    <button
                      type="button"
                      onClick={() => onBrowse(entry.path)}
                      className="hover:bg-accent flex w-full items-center gap-2 rounded-md px-2 py-2 text-left text-sm"
                    >
                      <Folder
                        aria-hidden="true"
                        className="text-muted-foreground h-4 w-4 shrink-0"
                      />
                      <span className="truncate">{entry.name}</span>
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </ScrollArea>
        </div>

        <div className="text-muted-foreground font-mono text-xs">
          {currentPath || "—"}
        </div>

        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            onClick={() => onOpenChange(false)}
          >
            Cancel
          </Button>
          <Button
            type="button"
            onClick={() => currentPath && onSelect(currentPath)}
            disabled={!currentPath}
          >
            Select this folder
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/**
 * Pick a user-facing string out of whatever `useAdminFsList` threw.
 * `jsonFetch` raises `HttpError` with the server's `error.message`
 * already extracted, so prefer that — it carries the specific
 * `library_root_missing` / outside-root / not-found copy the server
 * picked. Falls back to a generic message if the error isn't shaped
 * like an HttpError (e.g. a network failure with no JSON envelope).
 */
function describeFsError(err: unknown): string {
  if (err instanceof Error && err.message && err.message.length > 0) {
    return err.message;
  }
  return "Couldn't load this folder. The server may have rejected the path, or the path no longer exists.";
}

/**
 * Split an absolute `path` into named breadcrumb segments **relative to
 * `root`**. Returns the chain from the first child of `root` to `path`
 * itself. `root` is rendered separately by the dialog as the "Home"
 * icon, so it's omitted from this list.
 */
function breadcrumbSegments(
  root: string,
  path: string,
): { name: string; path: string }[] {
  if (!root || !path || !path.startsWith(root)) return [];
  const tail = path.slice(root.length).replace(/^\/+/, "");
  if (!tail) return [];
  const parts = tail.split("/").filter(Boolean);
  const out: { name: string; path: string }[] = [];
  let cursor = root;
  for (const p of parts) {
    cursor = cursor.endsWith("/") ? `${cursor}${p}` : `${cursor}/${p}`;
    out.push({ name: p, path: cursor });
  }
  return out;
}
