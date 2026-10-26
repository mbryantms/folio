"use client";

import * as React from "react";
import Link from "next/link";
import {
  ChevronDown,
  ChevronUp,
  MoreVertical,
  NotebookText,
} from "lucide-react";
import { toast } from "sonner";

import { Cover } from "@/components/Cover";
import { ClosingNoteEditor } from "@/components/library/ClosingNoteEditor";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/folio/dropdown-menu";
import { RatingStars } from "@/components/folio/rating-stars";
import { useSetIssueNote } from "@/lib/api/mutations";
import { UNDO_TOAST_DURATION_MS } from "@/lib/api/toast-strings";
import type { ClosingNoteEntryView } from "@/lib/api/types";
import { issueUrl } from "@/lib/urls";
import { cn } from "@/lib/utils";

/** A body this long (or multi-paragraph) gets the clamp + More toggle;
 *  shorter notes just render. */
export const CLAMP_THRESHOLD_CHARS = 280;

export function needsClamp(body: string): boolean {
  return body.length > CLAMP_THRESHOLD_CHARS || body.trim().includes("\n");
}

/** Body text that clamps to `lines` with an in-place More / Less when
 *  it is long or multi-paragraph; short text just renders. Shared by
 *  the closing-note and page-note rows. */
export function ClampedBody({
  text,
  lines = 4,
  className,
}: {
  text: string;
  lines?: 3 | 4;
  className?: string;
}) {
  const [expanded, setExpanded] = React.useState(false);
  const clamp = needsClamp(text);
  return (
    <>
      <p
        className={cn(
          "text-sm leading-relaxed whitespace-pre-line",
          clamp && !expanded && (lines === 3 ? "line-clamp-3" : "line-clamp-4"),
          className,
        )}
      >
        {text}
      </p>
      {clamp ? (
        <Button
          type="button"
          variant="link"
          size="sm"
          aria-expanded={expanded}
          onClick={() => setExpanded((e) => !e)}
          className="h-6 self-start px-0 text-xs"
        >
          {expanded ? "Less" : "More"}
          {expanded ? (
            <ChevronUp className="ml-1 size-3" aria-hidden="true" />
          ) : (
            <ChevronDown className="ml-1 size-3" aria-hidden="true" />
          )}
        </Button>
      ) : null}
    </>
  );
}

/** "Saga · #11" (or the title when there is no number). */
export function closingNoteIssueLabel(n: ClosingNoteEntryView): string {
  const parts: string[] = [];
  if (n.series_name) parts.push(n.series_name);
  if (n.issue_number) parts.push(`#${n.issue_number}`);
  else if (n.issue_title) parts.push(n.issue_title);
  return parts.length > 0 ? parts.join(" · ") : "Unknown issue";
}

function formatDate(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

/**
 * One issue-level closing note on the `/bookmarks` page: a full-width
 * text row (cover thumb, issue, your stars, date, body) rather than a
 * picture tile, since the text is the point. Long bodies clamp to four
 * lines with More / Less that expands in place. The kebab offers Edit
 * (inline, via `ClosingNoteEditor`), the issue page, Copy link and
 * Delete (Undo toast, the marker convention).
 */
export function ClosingNoteRow({
  note,
  className,
}: {
  note: ClosingNoteEntryView;
  className?: string;
}) {
  const [editing, setEditing] = React.useState(false);
  // Optimistic body: the feed re-fetches after the PUT, but the row
  // should not flash the old text in between.
  const [body, setBody] = React.useState(note.body);
  React.useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setBody(note.body);
  }, [note.body]);
  const mut = useSetIssueNote(note.series_slug ?? "", note.issue_slug ?? "");
  const canMutate = !!note.series_slug && !!note.issue_slug;
  const label = closingNoteIssueLabel(note);
  const issueHref =
    note.series_slug && note.issue_slug
      ? issueUrl({ series_slug: note.series_slug, slug: note.issue_slug })
      : null;

  const remove = () => {
    const prev = body;
    mut.mutate(
      { body: null },
      {
        onSuccess: () =>
          toast.success("Closing note removed", {
            duration: UNDO_TOAST_DURATION_MS,
            action: {
              label: "Undo",
              onClick: () => mut.mutate({ body: prev }),
            },
          }),
      },
    );
  };

  return (
    <article
      data-closing-note-row=""
      className={cn(
        "border-border bg-card flex gap-3.5 rounded-lg border p-3",
        className,
      )}
    >
      <div className="w-14 shrink-0">
        {issueHref ? (
          <Link href={issueHref} aria-label={`Open ${label}`}>
            <Cover src={note.cover_url ?? null} alt="" fallback={label} />
          </Link>
        ) : (
          <Cover src={note.cover_url ?? null} alt="" fallback={label} />
        )}
      </div>
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1">
          <span className="bg-muted text-foreground inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[10px] font-semibold tracking-wide uppercase">
            <NotebookText className="size-3" aria-hidden="true" />
            Closing note
          </span>
          <span
            className="text-muted-foreground truncate text-xs"
            title={label}
          >
            {label}
          </span>
          {note.rating != null ? (
            <RatingStars
              value={note.rating}
              readOnly
              size="sm"
              label={`Your rating of ${label}`}
            />
          ) : null}
          <span className="ml-auto flex shrink-0 items-center gap-1.5">
            <span className="text-muted-foreground text-xs whitespace-nowrap">
              {formatDate(note.updated_at)}
            </span>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button
                  type="button"
                  aria-label={`Actions for closing note on ${label}`}
                  className="folio-tap text-muted-foreground hover:text-foreground hover:bg-muted/50 inline-flex h-7 w-7 shrink-0 items-center justify-center rounded transition-colors"
                >
                  <MoreVertical className="size-4" aria-hidden="true" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem
                  disabled={!canMutate}
                  onSelect={() => setEditing(true)}
                >
                  Edit
                </DropdownMenuItem>
                {issueHref ? (
                  <DropdownMenuItem asChild>
                    <Link href={issueHref}>Open issue page</Link>
                  </DropdownMenuItem>
                ) : null}
                <DropdownMenuItem
                  disabled={!issueHref}
                  onSelect={() => {
                    if (!issueHref) return;
                    const url = `${window.location.origin}${issueHref}`;
                    void navigator.clipboard?.writeText(url);
                    toast.success("Link copied");
                  }}
                >
                  Copy link
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  className="text-destructive focus:text-destructive"
                  disabled={!canMutate || mut.isPending}
                  onSelect={remove}
                >
                  Delete
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </span>
        </div>

        {editing ? (
          <ClosingNoteEditor
            id={`closing-note-${note.issue_id}`}
            value={body}
            saving={mut.isPending}
            variant="card"
            onSave={(next) => {
              const prev = body;
              setBody(next ?? "");
              setEditing(false);
              mut.mutate({ body: next }, { onError: () => setBody(prev) });
            }}
          />
        ) : (
          <ClampedBody text={body} lines={4} />
        )}
      </div>
    </article>
  );
}
