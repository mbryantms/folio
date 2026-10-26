"use client";

import * as React from "react";
import { Pencil } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";

/**
 * Issue-level closing note — the textarea + Save pair used by the
 * reader's end-of-issue Wrap up pane and by the issue page.
 *
 * Controlled from the outside: `value` is the stored note, `onSave`
 * receives the trimmed body (or `null` to clear). The component owns
 * only the draft. Save stays disabled until the draft differs from the
 * stored value so a stray click never fires a no-op PUT. Mod+Enter
 * saves; the textarea is never auto-focused (in the reader the Enter
 * key must keep meaning "read next").
 *
 * `variant="page"` renders a view state first — the saved note as a
 * paragraph with an Edit pencil, or an "Add a closing note" link when
 * empty — and only swaps in the textarea on demand, so the issue page
 * header stays quiet when there is nothing to show.
 */
export function ClosingNoteEditor({
  value,
  onSave,
  saving = false,
  variant = "card",
  id = "closing-note",
  className,
}: {
  value: string | null;
  onSave: (body: string | null) => void;
  saving?: boolean;
  variant?: "card" | "page";
  id?: string;
  className?: string;
}) {
  const [draft, setDraft] = React.useState(value ?? "");
  const [editing, setEditing] = React.useState(false);
  // Re-sync when the stored value changes under us (save round-trip,
  // another surface's write).
  React.useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setDraft(value ?? "");
  }, [value]);

  const trimmed = draft.trim();
  const dirty = trimmed !== (value ?? "").trim();

  const commit = () => {
    if (!dirty || saving) return;
    onSave(trimmed === "" ? null : trimmed);
    setEditing(false);
  };

  if (variant === "page" && !editing) {
    return value ? (
      <div className={cn("space-y-1", className)}>
        <div className="flex items-center justify-between gap-2">
          <p className="text-muted-foreground text-xs font-medium">
            Your closing note
          </p>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="text-muted-foreground h-7 px-2"
            onClick={() => setEditing(true)}
          >
            <Pencil className="mr-1 size-3.5" aria-hidden="true" />
            Edit
          </Button>
        </div>
        <p className="text-sm whitespace-pre-wrap">{value}</p>
      </div>
    ) : (
      <Button
        type="button"
        variant="ghost"
        size="sm"
        className={cn("text-muted-foreground h-7 px-2", className)}
        onClick={() => setEditing(true)}
      >
        <Pencil className="mr-1 size-3.5" aria-hidden="true" />
        Add a closing note
      </Button>
    );
  }

  return (
    <div className={cn("space-y-2", className)}>
      <label
        htmlFor={id}
        className="text-muted-foreground block text-xs font-medium"
      >
        {variant === "page" ? "Your closing note" : "Closing note"}
      </label>
      <Textarea
        id={id}
        rows={3}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            commit();
          }
        }}
        placeholder="What stuck with you? Saved to this issue, not a page."
        className="min-h-0 resize-none text-sm"
        disabled={saving}
      />
      <div className="flex justify-end gap-2">
        {variant === "page" ? (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={() => {
              setDraft(value ?? "");
              setEditing(false);
            }}
            disabled={saving}
          >
            Cancel
          </Button>
        ) : null}
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={commit}
          disabled={!dirty || saving}
        >
          {trimmed === "" && value ? "Clear note" : "Save note"}
        </Button>
      </div>
    </div>
  );
}
