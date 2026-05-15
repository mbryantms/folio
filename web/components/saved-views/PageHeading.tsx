"use client";

import * as React from "react";
import { Pencil } from "lucide-react";

import { Input } from "@/components/ui/input";
import { useUpdatePage } from "@/lib/api/mutations";

const MAX_NAME_LEN = 80;

/** Page-detail title block. The title is inline-editable for custom
 *  pages (click → input, Enter/blur to commit, Esc to cancel); system
 *  pages render plain. The description (if set) appears underneath.
 *
 *  Page-level actions (rename via dialog, description, delete, sidebar
 *  toggle, manage rails) live in the toolbar's
 *  [`PageActionsMenu`](./PageActionsMenu.tsx) so they sit alongside
 *  the search/density controls instead of crowding the title. */
export function PageHeading({
  pageId,
  pageName,
  pageDescription,
  isSystem,
}: {
  pageId: string;
  pageName: string;
  pageDescription: string | null;
  isSystem: boolean;
}) {
  const rename = useUpdatePage(pageId);
  const [editing, setEditing] = React.useState(false);
  const [draft, setDraft] = React.useState(pageName);
  // Reset the draft on external prop changes — the render-phase
  // setState idiom from https://react.dev/learn/you-might-not-need-an-effect.
  const [lastSeenName, setLastSeenName] = React.useState(pageName);
  if (lastSeenName !== pageName) {
    setLastSeenName(pageName);
    setDraft(pageName);
  }
  const inputRef = React.useRef<HTMLInputElement | null>(null);

  React.useEffect(() => {
    if (editing) inputRef.current?.select();
  }, [editing]);

  const commit = async () => {
    const trimmed = draft.trim();
    setEditing(false);
    if (trimmed.length === 0 || trimmed.length > MAX_NAME_LEN) {
      setDraft(pageName);
      return;
    }
    if (trimmed === pageName) return;
    try {
      await rename.mutateAsync({ name: trimmed });
    } catch {
      setDraft(pageName);
    }
  };

  const cancel = () => {
    setDraft(pageName);
    setEditing(false);
  };

  return (
    <div className="min-w-0">
      {isSystem ? (
        <h1 className="text-2xl font-semibold tracking-tight">{pageName}</h1>
      ) : editing ? (
        <Input
          ref={inputRef}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void commit();
            } else if (e.key === "Escape") {
              e.preventDefault();
              cancel();
            }
          }}
          maxLength={MAX_NAME_LEN + 8}
          className="h-9 text-2xl font-semibold tracking-tight md:h-10"
          aria-label="Page name"
        />
      ) : (
        <button
          type="button"
          onClick={() => setEditing(true)}
          className="group hover:bg-secondary/50 -mx-1 flex max-w-full items-center gap-2 rounded-md px-1 py-0.5 text-left"
          title="Click to rename"
        >
          <h1 className="truncate text-2xl font-semibold tracking-tight">
            {pageName}
          </h1>
          <Pencil className="text-muted-foreground/0 group-hover:text-muted-foreground h-4 w-4 shrink-0 transition-colors" />
        </button>
      )}
      <p className="text-muted-foreground mt-1 text-sm">
        {pageDescription && pageDescription.length > 0
          ? pageDescription
          : "Pinned saved views and reading lists."}
      </p>
    </div>
  );
}
