"use client";

/**
 * Linked reading lists (PR 4): the events Up Next offers beside the main
 * line ("Start House of M · 0/64"). Starting one reads its first unread
 * issue in the event's own list, remembering the list it came from
 * (`?via=`) so the event's end returns there.
 */
import * as React from "react";
import { useRouter } from "next/navigation";
import { useQueryClient } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { apiMutate } from "@/lib/api/mutations/_core";
import { apiPath } from "@/lib/api/path";
import { queryKeys } from "@/lib/api/queries";
import type { NextUpDetour } from "@/lib/api/types";
import { displayListName, ensureListView } from "@/lib/reader/list-context";
import { noteReplace } from "@/lib/ui/back-flight";
import { readerUrl } from "@/lib/urls";

type Props = {
  detours: readonly NextUpDetour[] | undefined;
  /** The `?cbl=` view being read (the list to come back to). */
  viaViewId: string | null;
  /** Runs before navigating (closes the card). */
  onStart?: () => void;
};

export function DetourButtons(props: Props) {
  if (!props.detours || props.detours.length === 0) return null;
  return <DetourList {...props} detours={props.detours} />;
}

function DetourList({
  detours,
  viaViewId,
  onStart,
}: Props & { detours: readonly NextUpDetour[] }) {
  const router = useRouter();
  const queryClient = useQueryClient();
  const [pending, setPending] = React.useState<string | null>(null);
  // Events skipped from this card: hidden at once, the list page learns
  // of it through the links query.
  const [skipped, setSkipped] = React.useState<ReadonlySet<string>>(
    () => new Set(),
  );
  const setSkip = async (d: NextUpDetour, skip: boolean) => {
    if (!d.parent_list_id) return;
    const name = displayListName(d.name);
    try {
      await apiMutate<null>({
        path: apiPath`/me/cbl-lists/${d.parent_list_id}/links/${d.link_id}/skip`,
        method: skip ? "PUT" : "DELETE",
      });
    } catch {
      toast.error(`Couldn't ${skip ? "skip" : "unskip"} ${name}. Try again.`);
      return;
    }
    setSkipped((prev) => {
      const next = new Set(prev);
      if (skip) next.add(d.link_id);
      else next.delete(d.link_id);
      return next;
    });
    void queryClient.invalidateQueries({
      queryKey: queryKeys.cblListLinks(d.parent_list_id),
    });
    if (skip) {
      toast.message(`Skipped ${name}`, {
        action: { label: "Undo", onClick: () => void setSkip(d, false) },
      });
    }
  };
  const start = async (d: NextUpDetour) => {
    setPending(d.link_id);
    const view = await ensureListView(d.list_id, d.view_id);
    if (!view) {
      // Without the event's view the reader would open the issue in
      // series order and lose the event; say so and stay put.
      setPending(null);
      toast.error(`Couldn't open ${displayListName(d.name)}. Try again.`);
      return;
    }
    const href = readerUrl(
      d.first_unread.series_slug,
      d.first_unread.issue_slug,
      { cbl: view, via: viaViewId },
    );
    onStart?.();
    noteReplace(href);
    router.replace(href);
  };
  const shown = detours.filter((d) => !skipped.has(d.link_id));
  if (shown.length === 0) return null;
  return (
    <div className="flex flex-col gap-2 pt-1">
      <p className="text-muted-foreground text-xs">
        {shown.length === 1
          ? "Or read the event here first"
          : "Or read an event here first"}
      </p>
      {shown.map((d) => (
        <div key={d.link_id} className="flex items-center gap-1">
          <Button
            variant="outline"
            className="h-auto min-w-0 flex-1 justify-between gap-3 py-2"
            disabled={pending != null}
            onClick={() => void start(d)}
          >
            <span className="min-w-0 truncate text-left">
              {d.read > 0 ? "Resume" : "Start"} {displayListName(d.name)}
            </span>
            <span className="text-muted-foreground shrink-0 text-xs tabular-nums">
              {pending === d.link_id ? (
                <Loader2 className="size-3 animate-spin" aria-hidden />
              ) : (
                <span title={`${d.read} of ${d.in_library} read`}>
                  {d.read}/{d.in_library}
                </span>
              )}
            </span>
          </Button>
          {d.parent_list_id ? (
            <Button
              variant="ghost"
              size="sm"
              className="folio-tap shrink-0"
              disabled={pending != null}
              aria-label={`Skip ${displayListName(d.name)}`}
              onClick={() => void setSkip(d, true)}
            >
              Skip
            </Button>
          ) : null}
        </div>
      ))}
    </div>
  );
}
