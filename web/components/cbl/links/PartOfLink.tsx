"use client";

/**
 * Linked reading lists (PR 4): the back link on a list that is linked
 * into others — "Part of Marvel Master Part #07 · read after Excalibur
 * #12". The list the reader came from (`?via=`) wins, else the one they
 * read most recently (the server orders `parents` that way, the same
 * rule the reader uses to return at an event's end). The rest follow
 * under "Also in".
 */
import * as React from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { CornerUpLeft } from "lucide-react";

import { useOpenCblListView } from "@/lib/api/mutations/reading-list-links";
import type { ReadingListParentView } from "@/lib/api/types";
import { displayListName } from "@/lib/reader/list-context";

/** The parent the back link names, then the rest. Pure, for tests. */
export function orderParents(
  parents: readonly ReadingListParentView[],
  viaViewId: string | null,
): ReadingListParentView[] {
  const i = viaViewId ? parents.findIndex((p) => p.view_id === viaViewId) : -1;
  if (i <= 0) return [...parents];
  return [parents[i]!, ...parents.slice(0, i), ...parents.slice(i + 1)];
}

/** Where a parent link goes: its view, scrolled to the event's band. */
export function parentHref(p: ReadingListParentView, viewId: string): string {
  return p.kind === "event"
    ? `/views/${viewId}?event=${encodeURIComponent(p.link_id)}`
    : `/views/${viewId}`;
}

export function PartOfLink({
  listId,
  parents,
  viaViewId,
}: {
  listId: string;
  parents: readonly ReadingListParentView[];
  viaViewId: string | null;
}) {
  const router = useRouter();
  const open = useOpenCblListView(listId);
  if (parents.length === 0) return null;
  const [main, ...rest] = orderParents(parents, viaViewId);
  const go = (p: ReadingListParentView) => (e: React.MouseEvent) => {
    if (p.view_id) return;
    e.preventDefault();
    open.mutate(
      { listId: p.list_id },
      { onSuccess: (d) => d && router.push(parentHref(p, d.view_id)) },
    );
  };
  const linkTo = (p: ReadingListParentView, className?: string) => (
    <Link
      href={p.view_id ? parentHref(p, p.view_id) : "#"}
      onClick={go(p)}
      className={className}
    >
      {displayListName(p.name)}
    </Link>
  );
  return (
    <p className="text-muted-foreground flex flex-wrap items-center gap-x-1.5 gap-y-1 text-sm">
      <CornerUpLeft className="size-4 shrink-0" aria-hidden />
      <span>
        {main!.kind === "continues" ? "Continues " : "Part of "}
        {linkTo(main!, "text-foreground font-medium hover:underline")}
        {main!.anchor_label ? ` · read after ${main!.anchor_label}` : ""}
      </span>
      {rest.length > 0 && (
        <span>
          · Also in{" "}
          {rest.map((p, i) => (
            <React.Fragment key={p.link_id}>
              {i > 0 ? ", " : ""}
              {linkTo(p, "hover:text-foreground hover:underline")}
            </React.Fragment>
          ))}
        </span>
      )}
    </p>
  );
}
