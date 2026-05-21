"use client";

import { StickyNote } from "lucide-react";

import { WidgetCard } from "../WidgetCard";
import type { LogWidgetProps, NoteConfig } from "./types";

/** Free-form text widget — a place for the user to pin reading
 *  goals, current arc notes, or anything else relevant to their log.
 *
 *  Body is rendered as plain text with `whitespace-pre-wrap` to
 *  preserve line breaks. Markdown / inline formatting is a deferred
 *  polish (M6) — pulling in `react-markdown` for one widget didn't
 *  earn its bundle slot at v1. */
export function Note({ widget }: LogWidgetProps<NoteConfig>) {
  const body = widget.config.body ?? "";
  return (
    <WidgetCard widget={widget} title="Note" Icon={StickyNote}>
      {body.length === 0 ? (
        <p className="text-muted-foreground text-xs">
          Empty note. Open “Configure…” to add a body.
        </p>
      ) : (
        <p className="text-sm whitespace-pre-wrap">{body}</p>
      )}
    </WidgetCard>
  );
}
