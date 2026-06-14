"use client";

import { Check, Pencil, Plus, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { Textarea } from "@/components/ui/textarea";
import { useUpdateIssue } from "@/lib/api/mutations";

/** Inline notes editor for the issue detail page's Notes tab.
 *
 *  Two states:
 *  - **View**: renders the saved note as paragraph text with a small
 *    "Edit" pencil button. Empty-notes case offers an "Add a note"
 *    CTA so the user can start writing without leaving the page.
 *  - **Edit**: replaces the paragraph with a `<Textarea>` plus Save
 *    / Cancel buttons. Dirty check disables Save until the value
 *    differs from the snapshot. Mod+Enter saves; Escape cancels.
 *
 *  Persists via the existing `PATCH /series/{slug}/issues/{slug}`
 *  endpoint — same surface the Edit Issue sheet uses — so the
 *  `user_edited` set is updated and the scanner won't blow over the
 *  edit on a rescan. Empty save clears the column (server treats
 *  `notes: ""` as a clear-with-flag).
 */
export function InlineNotesEditor({
  seriesSlug,
  issueSlug,
  initial,
}: {
  seriesSlug: string;
  issueSlug: string;
  /** Current saved value. `null` when the issue has no note yet. */
  initial: string | null;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(initial ?? "");
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const update = useUpdateIssue(seriesSlug, issueSlug);

  // Re-sync when the upstream value changes — e.g. someone else edits
  // through the Edit sheet while this surface is mounted. Skip while
  // the user is actively editing so we don't stomp in-flight changes.
  useEffect(() => {
    if (!editing) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setDraft(initial ?? "");
    }
  }, [initial, editing]);

  // Focus the textarea + place caret at the end when entering edit
  // mode. `useEffect` (not layout) so the textarea has actually
  // mounted before we call focus.
  useEffect(() => {
    if (!editing) return;
    const ta = textareaRef.current;
    if (!ta) return;
    ta.focus();
    const end = ta.value.length;
    ta.setSelectionRange(end, end);
  }, [editing]);

  const dirty = draft.trim() !== (initial ?? "").trim();

  function save() {
    if (!dirty) {
      setEditing(false);
      return;
    }
    // Server accepts `notes: ""` as an explicit clear. Trim trailing
    // whitespace so a user typing then deleting back to empty doesn't
    // persist a row of spaces.
    const next = draft.trim();
    update.mutate(
      { notes: next.length === 0 ? null : next },
      {
        onSuccess: () => {
          setEditing(false);
        },
      },
    );
  }

  function cancel() {
    setDraft(initial ?? "");
    setEditing(false);
  }

  if (!editing) {
    if (!initial || initial.trim().length === 0) {
      return (
        <div className="flex max-w-prose flex-col items-start gap-3">
          <p className="text-muted-foreground text-sm">No notes for this issue yet.</p>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => setEditing(true)}
            className="h-9"
          >
            <Plus className="mr-1.5 size-4" aria-hidden="true" />
            Add a note
          </Button>
        </div>
      );
    }
    return (
      <div className="group max-w-prose space-y-2">
        <div className="flex items-start justify-between gap-3">
          <p className="text-foreground/90 flex-1 text-sm leading-6 whitespace-pre-wrap">
            {initial}
          </p>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={() => setEditing(true)}
            className="text-muted-foreground hover:text-foreground h-9 shrink-0 opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100"
            aria-label="Edit notes"
            title="Edit notes"
          >
            <Pencil className="size-3.5" aria-hidden="true" />
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="max-w-prose space-y-2">
      <Textarea
        ref={textareaRef}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            save();
          } else if (e.key === "Escape") {
            e.preventDefault();
            cancel();
          }
        }}
        placeholder="Write a note for this issue…"
        aria-label="Issue notes"
        rows={Math.max(4, Math.min(12, draft.split("\n").length + 1))}
        className="text-sm leading-6"
        disabled={update.isPending}
      />
      <div className="flex items-center justify-between gap-2">
        <p className="text-muted-foreground text-[11px]">
          <Kbd className="mx-1">
            ⌘
          </Kbd>
          <Kbd>
            ↵
          </Kbd>{" "}
          save · <Kbd className="mx-1">esc</Kbd> cancel
        </p>
        <div className="flex items-center gap-2">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={cancel}
            disabled={update.isPending}
            className="h-9"
          >
            <X className="mr-1 size-3.5" aria-hidden="true" />
            Cancel
          </Button>
          <Button
            type="button"
            size="sm"
            onClick={save}
            disabled={!dirty || update.isPending}
            className="h-9"
          >
            <Check className="mr-1 size-3.5" aria-hidden="true" />
            Save
          </Button>
        </div>
      </div>
    </div>
  );
}
